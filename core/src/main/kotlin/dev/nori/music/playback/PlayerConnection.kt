package dev.nori.music.playback

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
import dev.nori.music.Nori
import dev.nori.music.ffi.RadioStation
import dev.nori.music.ffi.Song
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

enum class Repeat { OFF, ALL, ONE }

/** Everything a UI needs to draw the player. Position is deliberately absent: see [PlayerConnection.positionMs]. */
data class PlayerState(
    val connected: Boolean = false,
    val queue: List<Song> = emptyList(),
    val index: Int = -1,
    /** What a skip either way lands on, shuffle and repeat included; -1 at an end. Not [index] ± 1 under shuffle. */
    val nextIndex: Int = -1,
    val previousIndex: Int = -1,
    /** [queue]'s indices in the order they play: the shuffle order under shuffle, otherwise 0 until size. */
    val order: List<Int> = emptyList(),
    /** Indices of the songs added by hand (Play next, Add to queue). */
    val queued: Set<Int> = emptySet(),
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
    /**
     * Playing from downloads while the server is unreachable; the original queue is parked after the
     * bridge block and comes back when the network does.
     */
    val bridging: Boolean = false,
) {
    val current: Song? get() = queue.getOrNull(index)
}

/**
 * How long a seek is watched for the player losing it. Long enough for a track to be fetched over a
 * slow line, short enough that it cannot reach back into a song the listener has settled into.
 */
private const val KEEP_SEEK_MS = 15_000L

/**
 * The UI's only handle on playback. It talks to [PlaybackService] through a
 * MediaController, so the UI holds no player and the service can outlive it.
 * State is pushed on change; the playhead is pulled ([positionMs]) so that a
 * hidden player screen costs nothing.
 */
@UnstableApi
class PlayerConnection(private val context: Context, private val nori: Nori) {
    private val _state = MutableStateFlow(PlayerState())
    val state: StateFlow<PlayerState> = _state
    private var controller: MediaController? = null
    private var connecting = false
    private val pending = ArrayList<(MediaController) -> Unit>()

    // The controller is released while the app is in the background and built again when it returns,
    // and for that moment it can answer nothing at all. Reporting zero then makes the seek bar snap to
    // 0:00 and jump back a heartbeat later, which looks like a bug in playback rather than in the UI.
    @Volatile private var lastPosition = 0L
    @Volatile private var lastPositionAt = 0L
    @Volatile private var lastBuffered = 0L

    val positionMs: Long get() {
        val c = controller
        if (c != null) {
            // Through a transition the player runs ahead of the ear (the held ending is counted as
            // played so the next track arrives in time to be mixed in); the sink says what is really
            // heard, and the bar shows that, in the song it belongs to (see publish).
            lastPosition = heard(c)?.second ?: c.currentPosition
            lastPositionAt = android.os.SystemClock.elapsedRealtime()
            return lastPosition
        }
        // Still reconnecting: carry on from where it was, moving if it was playing.
        val elapsed = if (_state.value.playing && lastPositionAt > 0) android.os.SystemClock.elapsedRealtime() - lastPositionAt else 0
        return (lastPosition + elapsed).coerceAtLeast(0)
    }

    /**
     * The song being heard and the place in it, while that is not what the player says. Through a
     * transition the player runs ahead of the ear: the held ending is counted as played the moment it
     * is decoded, so that the next track arrives in time to be mixed in, and the player is on the next
     * song while this one's ending still plays alone. The sink says what is really heard; the UI shows
     * that, in the song it belongs to. Null when the player's own word is the truth.
     *
     * The sink's reading is taken when the player asks for its position, which with a deep buffer is
     * seconds apart, so it is run on from there at one times - and past the point where the mix takes
     * over the ear has left the song, whether or not the sink has been asked since.
     */
    private var heardBefore = false
    private fun heard(c: MediaController): Pair<String, Long>? {
        val id = TransitionSink.heardId
        val on = id != null && run {
            val since = if (c.isPlaying) android.os.SystemClock.elapsedRealtime() - TransitionSink.heardAtMs else 0L
            val ms = TransitionSink.heardUs / 1000 + since
            ms < TransitionSink.heardUntilUs / 1000
        }
        // The ear left the song between two readings: the page changes song now, not at the next one.
        if (heardBefore && !on) main.post { controller?.let { publish(it, queueChanged = false) } }
        heardBefore = on
        if (!on) return null
        val since = if (c.isPlaying) android.os.SystemClock.elapsedRealtime() - TransitionSink.heardAtMs else 0L
        val duration = _state.value.queue.firstOrNull { it.id == id }?.duration?.toLong()?.times(1000) ?: Long.MAX_VALUE
        return id!! to (TransitionSink.heardUs / 1000 + since).coerceIn(0, duration)
    }

    val bufferedMs: Long get() = controller?.bufferedPosition?.also { lastBuffered = it } ?: lastBuffered

    fun connect() {
        if (controller != null || connecting) return
        connecting = true
        val future = MediaController.Builder(context, SessionToken(context, ComponentName(context, PlaybackService::class.java))).buildAsync()
        future.addListener({
            connecting = false
            val c = runCatching { future.get() }.getOrNull() ?: return@addListener
            controller = c
            c.addListener(listener)
            // The ear leaving the player's song, or catching up with it, is a song change to the
            // UI, and the player itself fires no event for it. Fired on the playback thread.
            TransitionSink.onHeardChanged = { main.post { controller?.let { publish(it, queueChanged = false) } } }
            publish(c, queueChanged = true)
            pending.forEach { it(c) }
            pending.clear()
        }, MoreExecutors.directExecutor())
    }

    fun disconnect() {
        forget()
        controller?.let { it.removeListener(listener); it.release() }
        controller = null
        _state.value = _state.value.copy(connected = false)
    }

    /**
     * Every controller call goes through here, and a MediaController may only be touched from the main
     * thread - it throws otherwise. Callers are not all on it (switching servers happens on an IO
     * thread while the network is being probed), so anything arriving from elsewhere is posted rather
     * than left to blow up in the caller's face.
     */
    private fun with(action: (MediaController) -> Unit) {
        if (android.os.Looper.myLooper() != android.os.Looper.getMainLooper()) {
            android.os.Handler(android.os.Looper.getMainLooper()).post { with(action) }
            return
        }
        controller?.let(action) ?: run { pending += action; connect() }
    }

    private val listener = object : Player.Listener {
        override fun onEvents(player: Player, events: Player.Events) {
            keepSeek(player)
            publish(player, events.contains(Player.EVENT_TIMELINE_CHANGED))
        }

        override fun onPlayerError(error: PlaybackException) {
            _state.value = _state.value.copy(error = error.cause?.message ?: error.errorCodeName)
        }
    }

    private fun publish(p: Player, queueChanged: Boolean) {
        val old = _state.value
        val item = p.currentMediaItem
        val fresh = queueChanged || !old.connected
        val queue = if (fresh) (0 until p.mediaItemCount).map { p.getMediaItemAt(it).toSong() } else old.queue
        val order = if (fresh || p.shuffleModeEnabled != old.shuffle) playOrder(p) else old.order
        val queued = if (fresh) (0 until p.mediaItemCount).filterTo(HashSet()) { p.getMediaItemAt(it).queuedAs() != null } else old.queued
        // The song on the page is the one being heard. Into a transition the player has moved on to
        // the next song while the ending of this one still plays alone (see heard); the page stays
        // on this song until the mix is heard. Its place in the queue is the nearest earlier one
        // with that id, the song it just left.
        val heardIndex = (p as? MediaController)?.let(::heard)?.first?.takeIf { it != item?.mediaId }?.let { id ->
            val at = p.currentMediaItemIndex
            (at - 1 downTo 0).firstOrNull { queue.getOrNull(it)?.id == id } ?: queue.indexOfLast { it.id == id }.takeIf { it >= 0 }
        }
        _state.value = old.copy(
            connected = true, queue = queue, order = order, queued = queued,
            index = if (p.mediaItemCount == 0) -1 else heardIndex ?: p.currentMediaItemIndex,
            nextIndex = if (p.mediaItemCount == 0) -1 else p.nextMediaItemIndex,
            previousIndex = if (p.mediaItemCount == 0) -1 else p.previousMediaItemIndex,
            // For a stream the live metadata carries what the station announces (ICY title), falling back to its name.
            radio = item?.takeIf { it.isRadio }?.let { p.mediaMetadata.title?.toString()?.takeIf(String::isNotBlank) ?: it.mediaMetadata.title?.toString() },
            playing = p.isPlaying, buffering = p.playbackState == Player.STATE_BUFFERING && p.playWhenReady,
            shuffle = p.shuffleModeEnabled,
            repeat = when (p.repeatMode) { Player.REPEAT_MODE_ALL -> Repeat.ALL; Player.REPEAT_MODE_ONE -> Repeat.ONE; else -> Repeat.OFF },
            durationMs = if (heardIndex != null) queue[heardIndex].duration.toLong() * 1000
                else p.duration.takeIf { it != C.TIME_UNSET && it > 0 } ?: (item?.mediaMetadata?.durationMs ?: 0),
            error = if (p.playerError == null) null else old.error,
            bridging = item?.isBridgeItem() == true ||
                (0 until p.mediaItemCount).any { p.getMediaItemAt(it).isBridgeItem() },
        )
    }

    private fun playOrder(p: Player): List<Int> {
        val t = p.currentTimeline
        if (!p.shuffleModeEnabled || t.isEmpty) return List(p.mediaItemCount) { it }
        val order = ArrayList<Int>(t.windowCount)
        var i = t.getFirstWindowIndex(true)
        while (i != C.INDEX_UNSET) { order += i; i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, true) }
        return order
    }

    private fun items(songs: List<Song>): List<MediaItem> = songs.map { it.toMediaItem(nori.library.coverUrl(it.coverArt, NOTIFICATION_ART)) }

    // ---- queue ----

    fun play(songs: List<Song>, startIndex: Int = 0, shuffle: Boolean = false) = with { c ->
        if (songs.isEmpty()) return@with
        c.shuffleModeEnabled = shuffle
        c.setMediaItems(items(songs), if (shuffle) C.INDEX_UNSET else startIndex.coerceIn(0, songs.lastIndex), 0)
        c.prepare()
        c.play()
    }

    // Where these land is the service's business (PlaybackService.upNext): after the playing song, and
    // for "last" after the songs added by hand before them, whatever the shuffle order says.
    fun playNext(songs: List<Song>) = with { c ->
        c.addMediaItems(items(songs).map { it.queued("next") })
        if (c.playbackState == Player.STATE_IDLE) c.prepare()
    }

    fun enqueue(songs: List<Song>) = with { c ->
        c.addMediaItems(items(songs).map { it.queued("last") })
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
    fun next() = with { c ->
        forget()
        if (c.hasNextMediaItem()) c.seekToNextMediaItem()
        else c.sendCustomCommand(SessionCommand(PlaybackService.CMD_FILL_NEXT, Bundle.EMPTY), Bundle.EMPTY)
    }
    /**
     * A rewind is a seek to the top, not a skip: on a queue restored but never prepared the
     * controller drops a bare seekToPrevious without a word, and the song then starts from where
     * it had been left - the first press seemingly doing nothing, the second working now that the
     * source is open. So it goes through the seek path, which prepares and watches the seek into
     * place. The rule for which one it is mirrors the service's (media3 rewinds past three seconds).
     */
    fun previous() = with { c ->
        val skips = nori.settings.value.previousAlwaysSkips && c.hasPreviousMediaItem()
        if (!skips && c.currentPosition > 3_000) { seekTo(0); if (!c.playWhenReady) c.play() }
        else { forget(); c.seekToPrevious() }
    }
    /** The song before, even well into this one - a swipe is a request for the other record, not a restart. */
    fun previousItem() = with { forget(); it.seekToPreviousMediaItem() }
    /**
     * A seek, and then a second one if the player did not keep it. Asked for on a song that is still
     * being fetched, the seek lands on a source that has not been opened yet: the player accepts it,
     * loads the track and starts it from the beginning, and the position the finger asked for is gone.
     * So it is remembered until the song is really playing on from there, and asked for again if the
     * player ended up back at the top of the same song.
     */
    fun seekTo(ms: Long) = with { c ->
        // A tap is a place in the song on the page. While the ear is still on the song the player
        // has left (see publish), that is the earlier song: the seek goes to it, not to the one the
        // player is already counting.
        val heardIndex = _state.value.index.takeIf { heard(c) != null && it >= 0 && it != c.currentMediaItemIndex }
        if (heardIndex != null) { forget(); c.seekTo(heardIndex, ms); return@with }
        // Where it was is read here, when it means something - a ready player - and the watch only
        // falls back to anchoring on its first READY observation while the player is still opening
        // (see keepSeek). Anchoring unconditionally after the fact puts the anchor next to the
        // target once the controller has applied the seek, and the direction test then reads a
        // forward seek as a backward one and fires it again, up to three times: the song jumping
        // back to the tapped place with a gap each time. On a player that is not ready yet the
        // position read now is meaningless - idle reports 0 while the session restores to wherever
        // the queue was left, and anchoring on 0 makes the watch read the restored position as
        // "moved by someone else" and give up on a dropped seek. Already at the target anchors
        // AT it: the controller answers from its own books, so this can catch the asked place
        // itself and the direction test would otherwise misfire on it the same way.
        val atAsk = if (c.playbackState == Player.STATE_READY) c.currentPosition.let { if (kotlin.math.abs(it - ms) <= 1_500) ms else it } else null
        wanted = Seek(ms, c.currentMediaItem?.mediaId, android.os.SystemClock.elapsedRealtime() + KEEP_SEEK_MS, atAsk)
        _pendingSeek.value = ms
        // A queue restored from the last time the app ran is deliberately left unprepared, so that
        // opening the app touches nothing. Such a player has no seekable window, the controller drops
        // every seek without a word, and the song then started from where it had been left - the finger
        // ignored. Asking for a place in a song is asking for the song, so prepare it; the seek itself
        // lands through the watch below, once there is something to seek in.
        if (!c.isCommandAvailable(Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM)) c.prepare()
        c.seekTo(ms)
        main.removeCallbacks(watch)
        main.postDelayed(watch, 300)
    }

    /**
     * Where a seek asked to go, where the player was when it was asked, in which song, and how long to
     * go on watching for it; see [seekTo].
     */
    private class Seek(val target: Long, val id: String?, var until: Long, from: Long?) {
        var tries = 0
        /// Anchor and lowest position: taken when the seek was asked on a ready player, otherwise
        /// on the first READY observation; see keepSeek.
        var from: Long? = from
        var low: Long? = from
        /// Last position seen, to tell a session still converging on the target from a stuck one.
        var last: Long? = null
    }
    private var wanted: Seek? = null
    /**
     * Where a seek asked to go, while the watch is still making sure it sticks. The seek bar
     * holds this instead of its own timer, so a slow seek (prepare, then the re-ask) reads as
     * one held place rather than a jump, a snap-back and a glide. Cleared with the watch.
     */
    private val _pendingSeek = MutableStateFlow<Long?>(null)
    val pendingSeek: StateFlow<Long?> = _pendingSeek
    private val main by lazy { android.os.Handler(android.os.Looper.getMainLooper()) }

    /**
     * The player's own events are not enough to catch this. A controller answers from its own books
     * the moment it is asked, so right after a seek it reports the position the finger chose whatever
     * the session did with it; the truth arrives later, and not always as an event. So the seek is
     * also looked at on a timer until it is clearly kept or the song has moved on.
     */
    private val watch = object : Runnable {
        override fun run() {
            controller?.let(::keepSeek)
            if (wanted != null) main.postDelayed(this, 300)
        }
    }

    private fun forget() { wanted = null; _pendingSeek.value = null; main.removeCallbacks(watch) }

    private fun keepSeek(p: Player) {
        val w = wanted ?: return
        // The song changed under it: there is nothing left to keep.
        if (w.id != p.currentMediaItem?.mediaId) { forget(); return }
        val now = android.os.SystemClock.elapsedRealtime()
        if (p.playbackState != Player.STATE_READY) {
            // A source that never opens still ends the watch; a slow one gets its full window.
            if (now > w.until) forget()
            return
        }
        val pos = p.currentPosition
        if (w.from == null) {
            // Asked on a player that was still opening, so there was nothing truthful to anchor on
            // then: this first sight of a ready player is what "where it was" means. The original
            // seek may have landed already (then pos is the target and the checks below keep it)
            // or been dropped (then the loop below re-asks). Either way the watch now measures
            // from truth. Already there anchors AT the target: the controller answers from its own
            // books, so this can catch the asked place itself, and the direction test below would
            // otherwise read a forward seek as a backward one and fire it again, yanking the song
            // back once it has played on. Same snap as the ask-time anchor above.
            val landed = kotlin.math.abs(pos - w.target) <= 1_500
            w.from = if (landed) w.target else pos
            w.low = if (landed) w.target else pos
            w.until = now + KEEP_SEEK_MS
            return
        }
        if (now > w.until) { forget(); return }
        val from = w.from ?: pos
        val target = w.target
        val playing = p.isPlaying
        val near = kotlin.math.abs(pos - target) <= 1_500
        // Paused where the finger asked: landed - a paused player moves for nothing else.
        if (near && !playing) { forget(); return }
        w.low = minOf(w.low ?: pos, pos)
        // Still converging on the target (a transcode lands seeks in stages, seconds apart): give it
        // its window rather than giving up, so the bar keeps holding the asked place throughout.
        val converging = w.last?.let { last ->
            val was = kotlin.math.abs(last - target)
            val isClose = kotlin.math.abs(pos - target)
            isClose + 250 < was
        } == true
        w.last = pos
        if (converging) { w.until = now + KEEP_SEEK_MS; return }
        val cameDown = (w.low ?: pos) < from - 500
        if (target >= from) {
            // Forward: played past it, and the anchor is truthful, so this cannot misfire on a
            // restored position the way ask-time anchoring did.
            if (pos > target + 400) { forget(); return }
            if (near) return // playing through it: wait for the proof above
            // Still at the start: dropped or not yet applied - ask again, briefly.
            if (pos <= from + 1_500) {
                if (w.tries++ >= 3) { forget(); return }
                p.seekTo(target)
            } else forget() // playing on without it; the recovery window has passed
            return
        }
        // Backward: pos > target is the starting condition, not proof of anything. Proof is having
        // come down from the start and reached the target's neighbourhood while playing on.
        if (playing && cameDown && pos >= target - 1_500) { forget(); return }
        if (near) return // may be arriving; wait (paused-near already kept above)
        // Never moved: dropped - ask again, briefly. Anything else (overshot, partial) is stale.
        if (!cameDown && pos >= from - 1_500) {
            if (w.tries++ >= 3) { forget(); return }
            p.seekTo(target)
        } else forget()
    }
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

    /** Presses one of the session's own buttons (the notification's heart or shuffle) the way the notification does; for the test bridge. */
    fun pressSessionButton(action: String) = with { it.sendCustomCommand(SessionCommand(action, Bundle.EMPTY), Bundle.EMPTY) }

    /** The notification's extra buttons as the session last published them, e.g. "heart_filled shuffle_off"; for the test bridge. */
    val sessionButtons: String get() = controller?.mediaButtonPreferences.orEmpty().joinToString(" ") {
        when (it.icon) {
            androidx.media3.session.CommandButton.ICON_HEART_FILLED -> "heart_filled"
            androidx.media3.session.CommandButton.ICON_HEART_UNFILLED -> "heart"
            androidx.media3.session.CommandButton.ICON_SHUFFLE_ON -> "shuffle_on"
            androidx.media3.session.CommandButton.ICON_SHUFFLE_OFF -> "shuffle_off"
            else -> it.sessionCommand?.customAction ?: "?"
        }
    }

    /** [minutes] 0 and [endOfTrack] false cancels. */
    fun sleep(minutes: Int, endOfTrack: Boolean = false, songs: Int = 0) = with { c ->
        c.sendCustomCommand(SessionCommand(PlaybackService.CMD_SLEEP, Bundle.EMPTY), Bundle().apply {
            putInt(PlaybackService.ARG_MINUTES, minutes); putBoolean(PlaybackService.ARG_END_OF_TRACK, endOfTrack); putInt(PlaybackService.ARG_SONGS, songs)
        })
        _state.value = _state.value.copy(sleepAt = if (minutes > 0) SystemClock.elapsedRealtime() + minutes * 60_000L else 0, sleepAtEndOfTrack = endOfTrack || songs > 0)
    }
}
