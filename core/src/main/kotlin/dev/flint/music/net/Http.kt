package dev.flint.music.net

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import kotlinx.coroutines.suspendCancellableCoroutine
import okhttp3.Call
import okhttp3.Callback
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
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

    private val network = MutableStateFlow(Net(online = true, metered = false))
    val net: StateFlow<Net> = network

    data class Net(val online: Boolean, val metered: Boolean)

    init {
        // A callback instead of asking ConnectivityManager on every request: no binder call on the hot path.
        val cm = context.getSystemService(ConnectivityManager::class.java)
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(n: Network, caps: NetworkCapabilities) {
                network.value = Net(true, !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED))
            }

            override fun onLost(n: Network) {
                network.value = network.value.copy(online = false)
            }
        })
    }

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
