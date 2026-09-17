package dev.flint.music.playback

import android.content.ComponentName
import android.content.Context
import android.os.Bundle
import android.os.SystemClock
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.common.util.Util
import androidx.media3.session.MediaController
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.MoreExecutors
import dev.flint.music.Flint
import dev.flint.music.ffi.RadioStation
import dev.flint.music.ffi.Song
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

enum class Repeat { OFF, ALL, ONE }

/** Everything a UI needs to draw the player. Position is deliberately absent: see [PlayerConnection.positionMs]. */
data class PlayerState(
    val connected: Boolean = false,
    val queue: List<Song> = emptyList(),
    val index: Int = -1,
    val radio: String? = null,
    val playing: Boolean = false,
    val buffering: Boolean = false,
    val shuffle: Boolean = false,
    val repeat: Repeat = Repeat.OFF,
    val durationMs: Long = 0,
    val error: String? = null,
    /** elapsedRealtime at which playback will pause, or 0. */
    val sleepAt: Long = 0,
    val sleepAtEndOfTrack: Boolean = false,
) {
    val current: Song? get() = queue.getOrNull(index)
}

/**
 * The UI's only handle on playback. It talks to [PlaybackService] through a
 * MediaController, so the UI holds no player and the service can outlive it.
 * State is pushed on change; the playhead is pulled ([positionMs]) so that a
 * hidden player screen costs nothing.
 */
@UnstableApi
class PlayerConnection(private val context: Context, private val flint: Flint) {
    private val _state = MutableStateFlow(PlayerState())
    val state: StateFlow<PlayerState> = _state
    private var controller: MediaController? = null
    private var connecting = false
    private val pending = ArrayList<(MediaController) -> Unit>()

    val positionMs: Long get() = controller?.currentPosition ?: 0
    val bufferedMs: Long get() = controller?.bufferedPosition ?: 0

    fun connect() {
        if (controller != null || connecting) return
        connecting = true
        val future = MediaController.Builder(context, SessionToken(context, ComponentName(context, PlaybackService::class.java))).buildAsync()
        future.addListener({
            connecting = false
            val c = runCatching { future.get() }.getOrNull() ?: return@addListener
            controller = c
            c.addListener(listener)
            publish(c, queueChanged = true)
            pending.forEach { it(c) }
            pending.clear()
        }, MoreExecutors.directExecutor())
    }

    fun disconnect() {
        controller?.let { it.removeListener(listener); it.release() }
        controller = null
        _state.value = _state.value.copy(connected = false)
    }

    private fun with(action: (MediaController) -> Unit) {
        controller?.let(action) ?: run { pending += action; connect() }
    }

    private val listener = object : Player.Listener {
        override fun onEvents(player: Player, events: Player.Events) {
            publish(player, events.contains(Player.EVENT_TIMELINE_CHANGED))
        }

        override fun onPlayerError(error: PlaybackException) {
            _state.value = _state.value.copy(error = error.cause?.message ?: error.errorCodeName)
        }
    }

    private fun publish(p: Player, queueChanged: Boolean) {
        val old = _state.value
        val item = p.currentMediaItem
        val queue = if (queueChanged || !old.connected) (0 until p.mediaItemCount).map { p.getMediaItemAt(it).toSong() } else old.queue
        _state.value = old.copy(
            connected = true, queue = queue, index = if (p.mediaItemCount == 0) -1 else p.currentMediaItemIndex,
            radio = item?.takeIf { it.isRadio }?.mediaMetadata?.title?.toString(),
            playing = p.isPlaying, buffering = p.playbackState == Player.STATE_BUFFERING && p.playWhenReady,
            shuffle = p.shuffleModeEnabled,
            repeat = when (p.repeatMode) { Player.REPEAT_MODE_ALL -> Repeat.ALL; Player.REPEAT_MODE_ONE -> Repeat.ONE; else -> Repeat.OFF },
            durationMs = p.duration.takeIf { it != C.TIME_UNSET && it > 0 } ?: (item?.mediaMetadata?.durationMs ?: 0),
            error = if (p.playerError == null) null else old.error,
        )
    }

    private fun items(songs: List<Song>): List<MediaItem> = songs.map { it.toMediaItem(flint.library.coverUrl(it.coverArt, 512)) }

    // ---- queue ----

    fun play(songs: List<Song>, startIndex: Int = 0, shuffle: Boolean = false) = with { c ->
        if (songs.isEmpty()) return@with
        c.shuffleModeEnabled = shuffle
        c.setMediaItems(items(songs), if (shuffle) C.INDEX_UNSET else startIndex.coerceIn(0, songs.lastIndex), 0)
        c.prepare()
        c.play()
    }

    fun playNext(songs: List<Song>) = with { c ->
        c.addMediaItems(if (c.mediaItemCount == 0) 0 else c.currentMediaItemIndex + 1, items(songs))
        if (c.playbackState == Player.STATE_IDLE) c.prepare()
    }

    fun enqueue(songs: List<Song>) = with { c ->
        c.addMediaItems(items(songs))
        if (c.playbackState == Player.STATE_IDLE) c.prepare()
    }

    fun playRadio(station: RadioStation) = with { c ->
        c.setMediaItem(station.toMediaItem())
        c.prepare()
        c.play()
    }

    fun skipTo(index: Int) = with { c -> c.seekToDefaultPosition(index); if (c.playbackState == Player.STATE_IDLE) c.prepare(); c.play() }
    fun remove(index: Int) = with { it.removeMediaItem(index) }
    fun move(from: Int, to: Int) = with { it.moveMediaItem(from, to) }
    fun clear() = with { it.clearMediaItems() }

    // ---- transport ----

    /** Also prepares a queue that was restored but never loaded. */
    fun toggle() = with { Util.handlePlayPauseButtonAction(it) }
    fun next() = with { it.seekToNextMediaItem() }
    fun previous() = with { it.seekToPrevious() }
    fun seekTo(ms: Long) = with { it.seekTo(ms) }
    fun setShuffle(on: Boolean) = with { it.shuffleModeEnabled = on }

    fun cycleRepeat() = with {
        it.repeatMode = when (it.repeatMode) {
            Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
            Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
            else -> Player.REPEAT_MODE_OFF
        }
    }

    /** While true the service trades its deep audio buffer for immediate response; for the equalizer screen. */
    fun setTuning(on: Boolean) = with { c ->
        c.sendCustomCommand(SessionCommand(PlaybackService.CMD_TUNING, Bundle.EMPTY), Bundle().apply { putBoolean(PlaybackService.ARG_ON, on) })
    }

    /** [minutes] 0 and [endOfTrack] false cancels. */
    fun sleep(minutes: Int, endOfTrack: Boolean = false) = with { c ->
        c.sendCustomCommand(SessionCommand(PlaybackService.CMD_SLEEP, Bundle.EMPTY), Bundle().apply {
            putInt(PlaybackService.ARG_MINUTES, minutes); putBoolean(PlaybackService.ARG_END_OF_TRACK, endOfTrack)
        })
        _state.value = _state.value.copy(sleepAt = if (minutes > 0) SystemClock.elapsedRealtime() + minutes * 60_000L else 0, sleepAtEndOfTrack = endOfTrack)
    }
}
