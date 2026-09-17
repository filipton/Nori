package dev.flint.music.net

import android.annotation.SuppressLint
import android.content.Context
import android.net.ConnectivityManager
import dev.flint.music.settings.ServerProfile
import kotlinx.coroutines.suspendCancellableCoroutine
import okhttp3.Call
import okhttp3.Callback
import okhttp3.ConnectionPool
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.File
import java.io.IOException
import java.security.KeyStore
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509TrustManager
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** The server is only allowed on unmetered networks and this is not one. */
class MeteredNetworkException : IOException("This server is set to Wi-Fi only")

/**
 * One connection pool for everything: API calls, cover art and audio all ride
 * the same HTTP/2 connection to the server, so the radio wakes once, not three times.
 * The clients are rebuilt when the server profile changes (headers, TLS); callers
 * go through [callFactory] / [streamFactory] so they always use the current ones.
 */
class Http(private val context: Context) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val pool = ConnectionPool(4, 20, TimeUnit.SECONDS)
    @Volatile private var profile: ServerProfile? = null

    @Volatile var api: OkHttpClient = build(null)
        private set

    /**
     * octo-fiesta answers a stream request for a provider track only once the
     * whole file is downloaded on its side, so the first byte can take minutes.
     */
    @Volatile var stream: OkHttpClient = api.newBuilder().readTimeout(4, TimeUnit.MINUTES).build()
        private set

    val callFactory = Call.Factory { api.newCall(it) }
    val streamFactory = Call.Factory { stream.newCall(it) }

    /**
     * Asked once per track, when its quality is chosen. A registered network callback would be woken
     * for every signal-strength change for as long as the process lives; this costs one binder call.
     */
    val metered: Boolean get() = connectivity.isActiveNetworkMetered

    fun configure(next: ServerProfile?) {
        val old = profile
        profile = next
        // Only TLS settings need new clients; headers and the Wi-Fi rule are read per request.
        if (old?.allowSelfSigned != next?.allowSelfSigned || old?.clientCert != next?.clientCert || old?.clientCertPassword != next?.clientCertPassword) {
            api = build(next)
            stream = api.newBuilder().readTimeout(4, TimeUnit.MINUTES).build()
        }
    }

    private fun build(p: ServerProfile?): OkHttpClient {
        val b = OkHttpClient.Builder()
            // Idle connections close after 20 s, while the radio is still up from the request that used them. The default
            // five minutes means every track fetch is followed, minutes later, by a lone FIN that wakes the modem again.
            .connectionPool(pool)
            .connectTimeout(10, TimeUnit.SECONDS)
            .readTimeout(30, TimeUnit.SECONDS)
            .addInterceptor { chain ->
                val now = profile
                if (now?.wifiOnly == true && metered) throw MeteredNetworkException()
                val headers = now?.headers.orEmpty()
                chain.proceed(if (headers.isEmpty()) chain.request() else chain.request().newBuilder().apply { headers.forEach { (k, v) -> header(k, v) } }.build())
            }
        if (p != null && (p.allowSelfSigned || p.clientCert.isNotEmpty())) tls(b, p)
        return b.build()
    }

    /** Self-signed servers and client certificates. Both are per profile and opt-in. */
    private fun tls(b: OkHttpClient.Builder, p: ServerProfile) {
        val trust: X509TrustManager = if (p.allowSelfSigned) TrustAll else {
            TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).apply { init(null as KeyStore?) }.trustManagers.filterIsInstance<X509TrustManager>().first()
        }
        val keys = if (p.clientCert.isEmpty()) null else runCatching {
            val store = KeyStore.getInstance("PKCS12")
            File(context.filesDir, "certs/${p.clientCert}").inputStream().use { store.load(it, p.clientCertPassword.toCharArray()) }
            KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm()).apply { init(store, p.clientCertPassword.toCharArray()) }.keyManagers
        }.getOrNull()
        val ssl = SSLContext.getInstance("TLS").apply { init(keys, arrayOf(trust), null) }
        b.sslSocketFactory(ssl.socketFactory, trust)
        if (p.allowSelfSigned) b.hostnameVerifier { _, _ -> true }
    }

    /** The user ticked "accept any certificate" for this server: their own box with a self-signed cert. */
    @SuppressLint("CustomX509TrustManager", "TrustAllX509TrustManager")
    private object TrustAll : X509TrustManager {
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) {}
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {}
        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    /** Cancelling the coroutine cancels the call, which is what makes live search cheap. */
    suspend fun get(url: String, timeoutMs: Long = 0): ByteArray = suspendCancellableCoroutine { cont ->
        val client = if (timeoutMs > 0) api.newBuilder().callTimeout(timeoutMs, TimeUnit.MILLISECONDS).build() else api
        val call = client.newCall(Request.Builder().url(url).build())
        cont.invokeOnCancellation { call.cancel() }
        call.enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                if (cont.isActive) cont.resumeWithException(e)
            }

            override fun onResponse(call: Call, response: Response) {
                try {
                    // octo-fiesta reports auth failures as 401 with a normal Subsonic error body, so the body is read either way.
                    val body = response.use { it.body.bytes() }
                    if (body.isEmpty() && !response.isSuccessful) throw IOException("HTTP ${response.code}")
                    cont.resume(body)
                } catch (e: IOException) {
                    if (cont.isActive) cont.resumeWithException(e)
                }
            }
        })
    }
}

/** What went wrong, in words a person can act on. */
fun describeConnectionError(e: Throwable): String = when (e) {
    is MeteredNetworkException -> e.message!!
    is java.net.UnknownHostException -> "Server not found. Check the address."
    is java.net.ConnectException -> "Nothing is answering at that address. Is the port right, and is the server running?"
    is java.net.SocketTimeoutException -> "The server did not answer in time."
    is javax.net.ssl.SSLPeerUnverifiedException, is javax.net.ssl.SSLHandshakeException ->
        "The server's certificate was not accepted. If it is self-signed, turn on \"Accept self-signed certificate\"; if it needs a client certificate, import one."
    is java.net.UnknownServiceException -> "Cleartext HTTP was refused; use https://"
    is dev.flint.music.ffi.CoreException.Api -> when (e.code) {
        40 -> "Wrong user name or password."
        41 -> "This server does not support token authentication."
        50 -> "This user is not allowed to do that."
        else -> e.reason
    }
    is dev.flint.music.ffi.CoreException.Parse -> "That address answered, but not like a Subsonic server. Check the URL (and any reverse-proxy path)."
    else -> e.message ?: e.javaClass.simpleName
}
