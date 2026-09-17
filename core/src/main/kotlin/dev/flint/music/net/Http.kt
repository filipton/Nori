package dev.flint.music.net

import android.content.Context
import android.net.ConnectivityManager
import kotlinx.coroutines.suspendCancellableCoroutine
import okhttp3.Call
import okhttp3.Callback
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * One connection pool for everything: API calls, cover art and audio all ride
 * the same HTTP/2 connection to the server, so the radio wakes once, not three times.
 */
class Http(context: Context) {
    val api: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(30, TimeUnit.SECONDS)
        .build()

    /**
     * octo-fiesta answers a stream request for a provider track only once the
     * whole file is downloaded on its side, so the first byte can take minutes.
     */
    val stream: OkHttpClient = api.newBuilder().readTimeout(4, TimeUnit.MINUTES).build()

    private val connectivity = context.getSystemService(ConnectivityManager::class.java)

    /**
     * Asked once per track, when its quality is chosen. A registered network callback would be woken
     * for every signal-strength change for as long as the process lives; this costs one binder call.
     */
    val metered: Boolean get() = connectivity.isActiveNetworkMetered

    /** Cancelling the coroutine cancels the call, which is what makes live search cheap. */
    suspend fun get(url: String): ByteArray = suspendCancellableCoroutine { cont ->
        val call = api.newCall(Request.Builder().url(url).build())
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
