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
import dev.nori.music.ffi.Hand
import dev.nori.music.ffi.NextAction
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

    @Volatile private var lastBuffered = 0L

    /**
     * Where the seek bar is. Through a transition the player runs ahead of the ear (the held ending is
     * counted as played so the next track arrives in time to be mixed in); the sink says what is really
     * heard, and the bar shows that, in the song it belongs to (see publish) - held while the ear has
     * changed song and the page has not followed yet, and run on from where it was while the controller
     * is being built again and can answer nothing (reporting zero then makes the bar snap to 0:00 and
     * jump back a heartbeat later). Those rules are nori-player's (heard.rs Playhead); this is one JNI
     * call with primitives in and out per frame.
     */
    val positionMs: Long get() {
        val c = controller ?: return PlayheadJni.runOn(clock, android.os.SystemClock.elapsedRealtime(), _state.value.playing)
        return heard(c, _state.value.index)
    }

    /**
     * The song being heard and the place in it, while that is not what the player says. Through a
     * transition the player runs ahead of the ear: the held ending is counted as played the moment it
     * is decoded, so that the next track arrives in time to be mixed in, and the player is on the next
     * song while this one's ending still plays alone. The sink says what is really heard; the UI shows
     * that, in the song it belongs to; the song is left in [heardIndex]. False when the
     * player's own word is the truth.
     *
     * The sink's reading is taken when the player asks for its position, which with a deep buffer is
     * seconds apart, so it is run on from there at one times - and past the point where the mix takes
     * over the ear has left the song, whether or not the sink has been asked since. The rules live in
     * nori-player (crates/player/src/heard.rs), so any app on it shows the same.
     */
    private fun heard(c: MediaController): Boolean {
        // One call, primitives only. The queue is the core's own.
        read(HeardJni.at(clock, android.os.SystemClock.elapsedRealtime(), c.isPlaying, c.currentMediaItemIndex, c.nextMediaItemIndex, c.currentPosition))
        return heardIndex >= 0
    }

    /** [heard] for the seek bar, whose page shows queue index [shown]: the place the bar shows. */
    private fun heard(c: MediaController, shown: Int): Long =
        read(PlayheadJni.position(clock, android.os.SystemClock.elapsedRealtime(), c.isPlaying, c.currentMediaItemIndex, c.nextMediaItemIndex, c.currentPosition, shown))

    /** Unpacks an answer into [heardIndex]; returns the place in it. */
    private fun read(r: Long): Long {
        heardIndex = (r ushr 44).toInt() - 1
        // The ear changed song between two readings: the page changes with it now, not at the next one.
        if ((r ushr 43) and 1L != 0L) main.post { controller?.let { publish(it, queueChanged = false) } }
        return r and ((1L shl 43) - 1)
    }

    /** What [heard] last found: the queue index the ear is on (-1: the player's own). */
    private var heardIndex = -1
    /** nori-player's reading of the transition engine: see crates/player/src/heard.rs. */
    private val clock = HeardJni.create()

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
        // The queue is the core's (crates/core/src/playlist.rs), read in one call; the controller's copy
        // of it trails the service a little, so the core's is taken when both are the same length.
        val view = if (fresh || p.shuffleModeEnabled != old.shuffle) dev.nori.music.ffi.playlistView().takeIf { it.songs.size == p.mediaItemCount } else null
        val queue = view?.songs ?: if (fresh) dev.nori.music.ffi.queueSongs(List(p.mediaItemCount) { p.getMediaItemAt(it).mediaId }) else old.queue
        val order = view?.order?.map { it.toInt() } ?: if (fresh || p.shuffleModeEnabled != old.shuffle) playOrder(p) else old.order
        val queued = view?.queued?.mapTo(HashSet()) { it.toInt() } ?: if (fresh) (0 until p.mediaItemCount).filterTo(HashSet()) { p.getMediaItemAt(it).queuedAs() != null } else old.queued
        // The song on the page is the one being heard. Into a transition the player has moved on to
        // the next song while the ending of this one still plays alone (see heard); the page stays
        // on this song until the mix is heard, and moves to the next one the moment it is, even while
        // the player is still on the old one. Which copy of a song queued twice that is, the heard
        // tracker decides (crates/player/src/heard.rs); a queue that has just changed is looked up anew.
        val heardIndex = (p as? MediaController)?.takeIf(::heard)?.let { this.heardIndex }?.takeIf { it >= 0 }?.let { i ->
            if (fresh) old.queue.getOrNull(i)?.id?.let { id -> queue.indexOfFirst { it.id == id }.takeIf { it >= 0 } } else i
        }?.takeIf { queue.getOrNull(it)?.id != item?.mediaId }
        _state.value = old.copy(
            connected = true, queue = queue, order = order, queued = queued,
            index = if (p.mediaItemCount == 0) -1 else heardIndex ?: p.currentMediaItemIndex,
            nextIndex = if (p.mediaItemCount == 0) -1 else p.nextMediaItemIndex,
            previousIndex = if (p.mediaItemCount == 0) -1 else p.previousMediaItemIndex,
            // For a stream the live metadata carries what the station announces (ICY title); which of that and
            // the station's name shows is the core's (words.rs radio_title).
            radio = item?.takeIf { it.isRadio }?.let { dev.nori.music.ffi.radioTitle(p.mediaMetadata.title?.toString(), it.mediaMetadata.title?.toString()) },
            playing = p.isPlaying, buffering = p.playbackState == Player.STATE_BUFFERING && p.playWhenReady,
            // A weighted shuffle plays a pre-spread list with the player's shuffle off so the order sticks;
            // the core keeps the control lit until the user turns it off or starts a plain Play.
            shuffle = p.shuffleModeEnabled || dev.nori.music.ffi.playlistShuffleShown(),
            repeat = when (p.repeatMode) { Player.REPEAT_MODE_ALL -> Repeat.ALL; Player.REPEAT_MODE_ONE -> Repeat.ONE; else -> Repeat.OFF },
            // The heard song's length while the ear is a song behind the player, else the player's, else
            // the tags' (nori_player::heard::shown_duration_ms).
            durationMs = dev.nori.music.ffi.shownDurationMs(heardIndex?.let { queue[it].duration.toLong() } ?: -1, p.duration, item?.mediaMetadata?.durationMs ?: 0),
            error = if (p.playerError == null) null else old.error,
            bridging = view?.bridging ?: old.bridging,
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

    private fun items(songs: List<Song>): List<MediaItem> = songs.toMediaItems { nori.library.coverUrl(it.coverArt, NOTIFICATION_ART) }

    // ---- queue ----

    fun play(songs: List<Song>, startIndex: Int = 0, shuffle: Boolean = false) = with { c ->
        if (songs.isEmpty()) return@with
        // Shuffle lit when this start asked for shuffle; cleared on a plain Play, so the album
        // control does not stay on after the row's Play starts some other queue. Pause and resume on
        // the page's own queue do not come through here, and leave the light as it was. Said to the
        // core at once, so the page does not flicker while the queue's own change is on its way.
        dev.nori.music.ffi.playlistShowShuffle(shuffle)
        c.shuffleModeEnabled = shuffle
        c.setMediaItems(items(songs), if (shuffle) C.INDEX_UNSET else startIndex.coerceIn(0, songs.lastIndex), 0)
        c.prepare()
        c.play()
    }

    /**
     * Play [songs] in the given order while keeping the Shuffle control lit. Used for weighted
     * artist-spread shuffles: media3's own shuffle would undo the spread.
     */
    fun playShuffledOrder(songs: List<Song>) = with { c ->
        if (songs.isEmpty()) return@with
        dev.nori.music.ffi.playlistShowShuffle(true)
        // Marked as already in order: the service takes it as it is and turns the player's own shuffle off.
        val items = items(songs)
        c.setMediaItems(listOf(items.first().ordered()) + items.drop(1), 0, 0)
        c.prepare()
        c.play()
        _state.value = _state.value.copy(shuffle = true)
    }

    // Where these land is the core's (nori_player::playlist::Playlist::take): after the playing song, and
    // for "last" after the songs added by hand before them, whatever the shuffle order says.
    fun playNext(songs: List<Song>) = with { c ->
        c.addMediaItems(items(songs).map { it.queued(Hand.NEXT) })
        if (c.playbackState == Player.STATE_IDLE) c.prepare()
    }

    fun enqueue(songs: List<Song>) = with { c ->
        c.addMediaItems(items(songs).map { it.queued(Hand.LAST) })
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
        // With nothing after, the service refills the queue and takes the skip when songs land
        // (nori_player::transport::next_action).
        when (dev.nori.music.ffi.nextAction(c.hasNextMediaItem())) {
            NextAction.SKIP -> c.seekToNextMediaItem()
            NextAction.FILL_THEN_SKIP -> c.sendCustomCommand(SessionCommand(PlaybackService.CMD_FILL_NEXT, Bundle.EMPTY), Bundle.EMPTY)
        }
    }
    /**
     * A rewind is a seek to the top, not a skip: on a queue restored but never prepared the
     * controller drops a bare seekToPrevious without a word, and the song then starts from where
     * it had been left - the first press seemingly doing nothing, the second working now that the
     * source is open. So it goes through the seek path, which prepares and watches the seek into
     * place. The rule for which one it is mirrors the service's (media3 rewinds past three seconds).
     */
    fun previous() = with { c ->
        // Restart here, or let the player's own previous decide: nori_player::queue::previous_restarts.
        if (dev.nori.music.ffi.queuePreviousRestarts(c.currentPosition, c.hasPreviousMediaItem())) { seekTo(0); if (!c.playWhenReady) c.play() }
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
        val heardIndex = _state.value.index.takeIf { heard(c) && it >= 0 && it != c.currentMediaItemIndex }
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
        // Where it was, and whether that means anything, is the keeper's to judge (nori_player::seek).
        SeekJni.ask(seeks, ms, android.os.SystemClock.elapsedRealtime(), c.playbackState == Player.STATE_READY, c.currentPosition)
        wantedId = c.currentMediaItem?.mediaId
        _pendingSeek.value = ms
        // A queue restored from the last time the app ran is deliberately left unprepared, so that
        // opening the app touches nothing. Such a player has no seekable window, the controller drops
        // every seek without a word, and the song then started from where it had been left - the finger
        // ignored. Asking for a place in a song is asking for the song, so prepare it; the seek itself
        // lands through the watch below, once there is something to seek in.
        if (!c.isCommandAvailable(Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM)) c.prepare()
        c.seekTo(ms)
        main.removeCallbacks(watch)
        main.postDelayed(watch, seekLookMs)
    }

    /** The seek being made to stick, in Rust (crates/player/src/seek.rs), and the song it was asked in. */
    private val seeks = SeekJni.create()
    /** How often the watch looks (nori_player::seek::LOOK_EVERY_MS), read once. */
    private val seekLookMs by lazy { dev.nori.music.ffi.playbackTimings().seekLookMs }
    private var wantedId: String? = null
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
            if (_pendingSeek.value != null) main.postDelayed(this, seekLookMs)
        }
    }

    private fun forget() { SeekJni.forget(seeks); wantedId = null; _pendingSeek.value = null; main.removeCallbacks(watch) }

    private fun keepSeek(p: Player) {
        if (_pendingSeek.value == null) return
        val verdict = SeekJni.look(
            seeks, android.os.SystemClock.elapsedRealtime(), p.currentMediaItem?.mediaId == wantedId,
            p.playbackState == Player.STATE_READY, p.currentPosition, p.isPlaying,
        )
        when {
            verdict == -2L -> forget()
            verdict >= 0L -> p.seekTo(verdict)
        }
    }

    fun setShuffle(on: Boolean) = with {
        dev.nori.music.ffi.playlistShowShuffle(on)
        it.shuffleModeEnabled = on
        _state.value = _state.value.copy(shuffle = on || it.shuffleModeEnabled)
    }

    fun cycleRepeat() = with {
        it.repeatMode = dev.nori.music.ffi.queueNextRepeat(it.repeatMode.toUByte()).toInt()
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
        val shown = dev.nori.music.ffi.sleepShown(minutes.coerceAtLeast(0).toUInt(), endOfTrack, songs.coerceAtLeast(0).toUInt(), SystemClock.elapsedRealtime())
        _state.value = _state.value.copy(sleepAt = shown.atMs, sleepAtEndOfTrack = shown.atEndOfTrack)
    }
}

/**
 * The seek bar's place over a [HeardJni] clock (crates/core/src/heard.rs over nori_player::heard::Playhead):
 * asked every frame the bar is drawn, so primitives only.
 */
internal object PlayheadJni {
    init { System.loadLibrary("norimusic") }

    /** As [HeardJni.at], with the place the bar shows while the page shows queue index [shown] (-1: nothing). */
    @JvmStatic external fun position(h: Long, nowMs: Long, playing: Boolean, on: Int, next: Int, positionMs: Long, shown: Int): Long
    /** The last place shown, run on from then if [playing]: for while the controller cannot be asked. */
    @JvmStatic external fun runOn(h: Long, nowMs: Long, playing: Boolean): Long
}
