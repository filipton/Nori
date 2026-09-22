package dev.nori.music.playback

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.os.Bundle
import android.os.Handler
import android.util.Log
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import dev.nori.music.downloads.Downloads
import dev.nori.music.ffi.Song
import java.net.ConnectException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.util.concurrent.atomic.AtomicBoolean

internal const val BRIDGE_FLAG = "bridge"
internal const val PARKED_HEAD_FLAG = "parkedHead"

/**
 * When the server is gone mid-evening and the next queued song is not on the phone, keep playing from
 * full downloads until the network is back, then return to the parked queue. Off by default; when off
 * this object never registers a network callback and costs nothing.
 *
 * The user's queue is not rewritten: online songs from the failure point are kept after a labelled
 * bridge block, and the bridge songs are tagged so they can be pulled out again.
 */
@androidx.media3.common.util.UnstableApi
class OfflineBridge(
    private val context: Context,
    private val player: ExoPlayer,
    private val downloads: Downloads,
    private val sources: MediaSources,
    private val main: Handler,
    private val cover: (Song) -> String?,
    private val onChanged: () -> Unit,
) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private val watching = AtomicBoolean(false)

    /** True while bridge songs are in the queue ahead of the parked online ones. */
    @Volatile var active: Boolean = false
        private set

    /** Network failure while playing: prefer a local song still in the queue, else start a download bridge. */
    fun onPlaybackError(error: PlaybackException): Boolean {
        if (!error.isNetworkish()) return false
        nextDownloadedIndex()?.let { i ->
            player.seekTo(i, C.TIME_UNSET)
            player.prepare()
            player.play()
            return true
        }
        return startOrExtend()
    }

    /**
     * Called on every track change while active: if the next thing is the parked online head and we
     * are still offline, inject more downloads before it so the evening does not fall into the wall.
     */
    fun onTrack(item: MediaItem?) {
        if (!active || item == null) return
        val next = player.nextMediaItemIndex.takeIf { it != C.INDEX_UNSET }?.let(player::getMediaItemAt) ?: return
        if (!next.isParkedHead) return
        if (networkUp()) {
            resume()
            return
        }
        injectBeforeParked()
    }

    /** Network came back: drop bridge songs and continue from the parked head. */
    fun resume() {
        if (!active) return
        val items = (0 until player.mediaItemCount).map(player::getMediaItemAt)
        val kept = items.filterNot { it.isBridge }
        val resumeId = items.firstOrNull { it.isParkedHead }?.mediaId
            ?: kept.firstOrNull()?.mediaId
        val at = resumeId?.let { id -> kept.indexOfFirst { it.mediaId == id } }?.takeIf { it >= 0 } ?: 0
        mutating {
            if (kept.isEmpty()) {
                clearState()
                return
            }
            player.setMediaItems(kept.map { it.clearBridgeTags() }, at.coerceIn(0, kept.lastIndex), 0)
            player.prepare()
            player.play()
        }
        clearState()
        onChanged()
        Log.i(TAG, "resumed parked queue at $at (${kept.size} songs)")
    }

    /** User replaced the queue: drop bridge bookkeeping without touching the new list. */
    fun abandon() {
        if (!active && networkCallback == null) return
        clearState()
        onChanged()
    }

    private fun startOrExtend(): Boolean {
        val done = downloads.state.value.done
        if (done.isEmpty()) return false
        val at = player.currentMediaItemIndex.takeIf { it >= 0 } ?: return false
        val seed = player.currentMediaItem?.takeUnless { it.isRadio }?.toSong()
        val queued = (0 until player.mediaItemCount).mapTo(HashSet()) { player.getMediaItemAt(it).mediaId }
        val picks = pick(seed, done, queued, BRIDGE_BATCH)
        if (picks.isEmpty()) return false

        mutating {
            if (!active) {
                // [at] is the failed song; park it and everything after, play downloads in between.
                val parked = (at until player.mediaItemCount).map(player::getMediaItemAt)
                player.removeMediaItems(at, player.mediaItemCount)
                val bridge = picks.toMediaItems(cover).map { it.asBridge() }
                val rest = parked.mapIndexed { i, m -> if (i == 0) m.asParkedHead() else m.clearBridgeTags() }
                player.addMediaItems(bridge + rest)
                player.seekTo(at, C.TIME_UNSET)
            } else {
                injectBeforeParked(picks)
            }
            player.prepare()
            player.play()
        }
        active = true
        watchNetwork()
        onChanged()
        Log.i(TAG, "bridging with ${picks.size} downloads (seed=${seed?.id})")
        return true
    }

    private fun injectBeforeParked(extra: List<Song>? = null) {
        val head = (0 until player.mediaItemCount).indexOfFirst { player.getMediaItemAt(it).isParkedHead }
            .takeIf { it >= 0 } ?: return
        val seed = player.currentMediaItem?.takeUnless { it.isRadio }?.toSong()
        val queued = (0 until player.mediaItemCount).mapTo(HashSet()) { player.getMediaItemAt(it).mediaId }
        val picks = extra ?: pick(seed, downloads.state.value.done, queued, BRIDGE_BATCH)
        if (picks.isEmpty()) return
        player.addMediaItems(head, picks.toMediaItems(cover).map { it.asBridge() })
    }

    private fun nextDownloadedIndex(): Int? {
        val from = player.currentMediaItemIndex
        if (from < 0) return null
        val t = player.currentTimeline
        var i = t.getNextWindowIndex(from, Player.REPEAT_MODE_OFF, player.shuffleModeEnabled)
        while (i != C.INDEX_UNSET) {
            val id = player.getMediaItemAt(i).mediaId
            if (id in sources.downloaded) return i
            i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, player.shuffleModeEnabled)
        }
        return null
    }

    private fun watchNetwork() {
        if (!watching.compareAndSet(false, true)) return
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                main.post { if (active) resume() }
            }
        }
        runCatching { connectivity.registerDefaultNetworkCallback(cb) }
            .onSuccess { networkCallback = cb }
            .onFailure {
                watching.set(false)
                Log.w(TAG, "could not watch network for bridge resume", it)
            }
    }

    private fun clearState() {
        active = false
        networkCallback?.let { runCatching { connectivity.unregisterNetworkCallback(it) } }
        networkCallback = null
        watching.set(false)
    }

    private fun networkUp(): Boolean =
        connectivity.activeNetwork != null &&
            connectivity.getNetworkCapabilities(connectivity.activeNetwork) != null

    private inline fun mutating(block: () -> Unit) {
        bridgeMutating = true
        try {
            block()
        } finally {
            bridgeMutating = false
        }
    }

    companion object {
        private const val TAG = "nori.bridge"
        private const val BRIDGE_BATCH = 12

        /** True while [OfflineBridge] is editing the playlist, so the service does not treat it as a user edit. */
        @Volatile var bridgeMutating: Boolean = false
            private set

        fun pick(seed: Song?, done: List<Song>, exclude: Set<String>, n: Int): List<Song> {
            val pool = done.filter { it.id !in exclude && !it.isExternal && !it.id.startsWith("ext-") }
            if (pool.isEmpty()) return emptyList()
            fun score(s: Song): Int {
                var v = 0
                if (seed != null) {
                    if (seed.artist.isNotBlank() && s.artist.equals(seed.artist, ignoreCase = true)) v += 4
                    if (seed.albumId != null && s.albumId == seed.albumId) v += 3
                    if (!seed.genre.isNullOrBlank() && s.genre == seed.genre) v += 2
                    if (seed.artistId != null && s.artistId == seed.artistId) v += 2
                }
                if (s.starred) v += 1
                return v
            }
            return pool.groupBy(::score).toSortedMap(compareByDescending { it })
                .values.flatMap { it.shuffled() }
                .take(n)
        }
    }
}

private val MediaItem.isBridge: Boolean
    get() = mediaMetadata.extras?.getBoolean(BRIDGE_FLAG) == true

private val MediaItem.isParkedHead: Boolean
    get() = mediaMetadata.extras?.getBoolean(PARKED_HEAD_FLAG) == true

private fun MediaItem.asBridge(): MediaItem = withFlag(BRIDGE_FLAG, true).withFlag(PARKED_HEAD_FLAG, false)

private fun MediaItem.asParkedHead(): MediaItem = withFlag(PARKED_HEAD_FLAG, true).withFlag(BRIDGE_FLAG, false)

private fun MediaItem.clearBridgeTags(): MediaItem = withFlag(BRIDGE_FLAG, false).withFlag(PARKED_HEAD_FLAG, false)

private fun MediaItem.withFlag(key: String, on: Boolean): MediaItem {
    val extras = Bundle(mediaMetadata.extras ?: Bundle.EMPTY)
    if (on) extras.putBoolean(key, true) else extras.remove(key)
    return buildUpon().setMediaMetadata(mediaMetadata.buildUpon().setExtras(extras).build()).build()
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

fun MediaItem.isBridgeItem(): Boolean = mediaMetadata.extras?.getBoolean(BRIDGE_FLAG) == true
