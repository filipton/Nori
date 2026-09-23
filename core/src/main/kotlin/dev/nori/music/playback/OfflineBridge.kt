package dev.nori.music.playback

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import android.util.Log
import androidx.media3.common.C
import androidx.media3.common.PlaybackException
import androidx.media3.exoplayer.ExoPlayer
import dev.nori.music.ffi.QueueEdit
import java.net.ConnectException
import java.net.SocketTimeoutException
import java.net.UnknownHostException

/**
 * When the server is gone mid-evening and the next queued song is not on the phone, keep playing from
 * full downloads until the network is back, then return to the parked queue. Off by default; when off
 * this object never registers a network callback and costs nothing.
 *
 * Which downloads, where they go and what comes back is the core's (crates/core/src/bridge.rs over its
 * queue, nori_player::playlist): this watches the network, and the service makes each change the core
 * made to its player ([apply]).
 */
@androidx.media3.common.util.UnstableApi
class OfflineBridge(
    context: Context,
    private val player: ExoPlayer,
    private val core: () -> dev.nori.music.ffi.Core,
    private val main: Handler,
    private val apply: (QueueEdit) -> Unit,
) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private var networkCallback: ConnectivityManager.NetworkCallback? = null

    /** Network failure while playing: a downloaded song still queued plays, else a bridge starts. */
    fun onPlaybackError(error: PlaybackException): Boolean {
        if (!error.isNetworkish()) return false
        val next = runCatching { core().bridgeNextDownloaded() }.getOrDefault(-1)
        if (next >= 0) {
            player.seekTo(next, C.TIME_UNSET)
            player.prepare()
            player.play()
            return true
        }
        val edit = runCatching { core().bridgeStart() }.getOrNull() ?: return false
        apply(edit)
        watchNetwork()
        Log.i(TAG, "bridging with ${edit.songs.size} downloads")
        return true
    }

    /**
     * Every track change: once the bridge has played up to the parked song, the queue comes back if the
     * network has, or more downloads go in before it if it has not.
     */
    fun onTrack() {
        val s = dev.nori.music.ffi.playlistBridgeState()
        if (!s.bridging) return stopWatching()
        if (!s.nextIsParked) return
        if (networkUp()) resume() else runCatching { core().bridgeStart() }.getOrNull()?.let(apply)
    }

    /** Network came back: the bridge's songs go and the parked song plays. */
    fun resume() {
        dev.nori.music.ffi.playlistUnbridge()?.let { apply(it); Log.i(TAG, "resumed the parked queue") }
        stopWatching()
    }

    /** A new queue replaced the bridged one (the core dropped the bridge with it): stop watching. */
    fun abandon() = stopWatching()

    private fun watchNetwork() {
        if (networkCallback != null) return
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                main.post { resume() }
            }
        }
        runCatching { connectivity.registerDefaultNetworkCallback(cb) }
            .onSuccess { networkCallback = cb }
            .onFailure { Log.w(TAG, "could not watch network for bridge resume", it) }
    }

    private fun stopWatching() {
        networkCallback?.let { runCatching { connectivity.unregisterNetworkCallback(it) } }
        networkCallback = null
    }

    private fun networkUp(): Boolean =
        connectivity.activeNetwork != null &&
            connectivity.getNetworkCapabilities(connectivity.activeNetwork) != null

    private companion object {
        const val TAG = "nori.bridge"
    }
}

/** Network / unreachable server, not a bad file or a refused audio sink. */
fun PlaybackException.isNetworkish(): Boolean {
    when (errorCode) {
        PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED,
        PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT,
        PlaybackException.ERROR_CODE_TIMEOUT,
        -> return true
        PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS -> return true
    }
    return generateSequence(cause) { it.cause }.any {
        it is UnknownHostException || it is SocketTimeoutException || it is ConnectException ||
            it is java.net.NoRouteToHostException || it is java.io.InterruptedIOException ||
            (it is java.io.IOException && it.message?.contains("offline", ignoreCase = true) == true)
    }
}
