package dev.nori.music.playback

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRouting
import android.media.AudioTrack
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.SimpleBasePlayer
import androidx.media3.common.Timeline
import androidx.media3.common.Tracks
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import dalvik.annotation.optimization.CriticalNative
import dev.nori.music.Nori

/**
 * nori-engine, the Rust player (crates/android/src/player.rs): the core's queue played into an AudioTrack
 * the engine writes itself (crates/android/src/track.rs). Commands go in as primitives; the engine's
 * events come out through [RustBridge.signal] and [event], one wake of the main thread per batch.
 */
internal object RustPlayerJni {
    init { System.loadLibrary("norimusic") }
    /** [float] is the high quality output setting; [memoryMb] the app's memory class. 0 when it could not start. */
    @JvmStatic external fun create(sdk: Int, float: Boolean, memoryMb: Int): Long
    @JvmStatic external fun destroy(h: Long)
    @JvmStatic @CriticalNative external fun playAt(h: Long, index: Int, ms: Long)
    @JvmStatic @CriticalNative external fun play(h: Long)
    @JvmStatic @CriticalNative external fun pause(h: Long)
    /** The core's queue was edited (or reordered): the engine follows it. */
    @JvmStatic @CriticalNative external fun queueChanged(h: Long)
    @JvmStatic @CriticalNative external fun setRepeat(h: Long, mode: Int)
    @JvmStatic @CriticalNative external fun replan(h: Long)
    @JvmStatic @CriticalNative external fun gainChanged(h: Long)
    /** The sound and the controls' fades as the core's settings are now. */
    @JvmStatic @CriticalNative external fun applySettings(h: Long)
    /** Where the ear is in it now (the engine's `status().position_now()`), read when asked, never ticked. */
    @JvmStatic @CriticalNative external fun positionMs(h: Long): Long
    @JvmStatic @CriticalNative external fun mixing(h: Long): Boolean
    @JvmStatic @CriticalNative external fun bytesWritten(h: Long): Long
    /** The next event, `kind shl 32 or index` (kind: state 0, song 1, error 2, output 3); -1 when there are no more. */
    @JvmStatic @CriticalNative external fun event(h: Long): Long
    /** The words of the event [event] last gave: the song's id, the error, the output's name. */
    @JvmStatic external fun eventText(h: Long): String?
    /** The track's route changed: [type] is `AudioDeviceInfo.TYPE_*`. */
    @JvmStatic external fun device(h: Long, type: Int, name: String?)
}

/**
 * What the Rust player asks of the platform, from its own threads: an AudioTrack, a song's bytes, the cache
 * key a song resolves to, and a wake for its events. Each is rare: a track per start, a body per burst of a
 * song, a key per song, a signal per batch of events.
 */
@UnstableApi
internal object RustBridge {
    @Volatile var player: EnginePlayer? = null

    @JvmStatic fun openTrack(rate: Int, channels: Int, float: Boolean, frames: Int): AudioTrack? = player?.openTrack(rate, channels, float, frames)
    @JvmStatic fun open(url: String, from: Long): RustBody? = player?.open(url, from)
    @JvmStatic fun key(id: String): String? = player?.key(id)
    @JvmStatic fun signal() { player?.signal() }
}

/** A song's bytes from [from] on, read by the Rust player's loader a buffer at a time. [length] is -1 when unknown. */
class RustBody internal constructor(private val source: DataSource, @JvmField val length: Long, private val done: () -> Unit) {
    @JvmField val buffer = ByteArray(64 * 1024)

    /** Bytes read into [buffer], at most [max]; -1 at the end, -2 when the connection broke. */
    fun read(max: Int): Int = try {
        val n = source.read(buffer, 0, minOf(max, buffer.size))
        if (n == C.RESULT_END_OF_INPUT) -1 else n
    } catch (e: Exception) {
        android.util.Log.w("nori", "rust player: the song's bytes stopped coming: $e")
        -2
    }

    fun close() {
        runCatching { source.close() }
        done()
    }
}

/**
 * The Rust player as media3 sees it, so the session, the notification, the lock screen, headset buttons,
 * Android Auto and the widget follow it as they follow ExoPlayer, through [PlaybackService]'s `Controls`.
 * The queue is the core's: `Controls` edits it there first, and this keeps the items the session shows
 * and tells the engine. The song shown is the one heard (the engine's song events, which come through a
 * mix at the moment the next song is audible); the position is the engine's, read when asked.
 *
 * What ExoPlayer does around the player and the engine does not, this does as ExoPlayer would: audio
 * focus, pausing when headphones are pulled out, the CPU wake lock while music plays. A seek or a skip
 * asked for while paused waits here until play, so nothing is fetched until the music is wanted.
 */
@UnstableApi
class EnginePlayer(private val context: Context, private val nori: Nori) : SimpleBasePlayer(Looper.getMainLooper()) {
    private val main = Handler(Looper.getMainLooper())
    private val h = RustPlayerJni.create(
        Build.VERSION.SDK_INT, nori.settings.value.hiRes,
        context.getSystemService(android.app.ActivityManager::class.java).memoryClass,
    )

    private val items = ArrayList<MediaItem>()
    private val uids = ArrayList<Long>()
    private var nextUid = 0L
    /** Changes with every edit, for the timeline kept below. */
    private var edits = 0

    /** The song shown: the one heard, or the one a seek asked for while paused. */
    private var current = 0
    /** Where [current] starts once play is pressed; null when the engine is on it already. */
    private var pending: Long? = 0L
    /** The engine was given a song to play since the list was set or the player stopped. */
    private var started = false
    /** A song the engine was sent to, whose song event is that jump, not a song ending. */
    private var expecting = -1
    private var prepared = false
    private var playWhenReady = false
    private var whyPlayWhenReady = Player.PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST
    private var suppressed = Player.PLAYBACK_SUPPRESSION_REASON_NONE
    private var engineState = ENGINE_IDLE
    private var repeat = Player.REPEAT_MODE_OFF
    private var shuffle = false
    private var error: PlaybackException? = null
    /** The engine moved on by itself (a song ended into the next): said once, in the next state. */
    private var moved = false
    @Volatile private var loading = 0
    /** Pause when the song playing ends (the sleep timer's "end of this song"). */
    var pauseAtEndOfItem = false

    // ---- what the service asks of it directly ----

    fun applySettings() = RustPlayerJni.applySettings(h)
    fun gainChanged() = RustPlayerJni.gainChanged(h)
    fun replan() = RustPlayerJni.replan(h)
    val mixing: Boolean get() = RustPlayerJni.mixing(h)
    val bytesWritten: Long get() = RustPlayerJni.bytesWritten(h)

    // ---- the state media3 reads ----

    private val position = PositionSupplier { pending ?: RustPlayerJni.positionMs(h) }
    private var timeline: QueueTimeline? = null
    private var timelineAt = Pair(-1, -1L)

    /** The list with the core's play order under shuffle, built again only when either changed. */
    private fun timeline(): QueueTimeline {
        val at = Pair(edits, if (shuffle) PlaylistJni.rev() else -1L)
        timeline?.takeIf { timelineAt == at }?.let { return it }
        val order = if (shuffle && items.isNotEmpty()) IntArray(items.size).takeIf { PlaylistJni.order(it) == items.size } else null
        return QueueTimeline(ArrayList(items), uids.toLongArray(), order).also { timeline = it; timelineAt = at }
    }

    private fun playbackState(): Int = when {
        !prepared || error != null -> Player.STATE_IDLE
        items.isEmpty() || engineState == ENGINE_ENDED -> Player.STATE_ENDED
        else -> Player.STATE_READY
    }

    override fun getState(): State {
        val b = State.Builder()
            .setAvailableCommands(COMMANDS)
            .setPlayWhenReady(playWhenReady, whyPlayWhenReady)
            .setPlaybackState(playbackState())
            .setPlaybackSuppressionReason(suppressed)
            .setRepeatMode(repeat)
            .setShuffleModeEnabled(shuffle)
            .setIsLoading(loading > 0)
            .setAudioAttributes(ATTRIBUTES)
            .setPlaylist(timeline(), Tracks.EMPTY, null)
            .setCurrentMediaItemIndex(if (items.isEmpty()) C.INDEX_UNSET else current.coerceIn(0, items.size - 1))
            .setContentPositionMs(position)
        error?.let { b.setPlayerError(it) }
        if (moved) {
            moved = false
            b.setPositionDiscontinuity(Player.DISCONTINUITY_REASON_AUTO_TRANSITION, 0)
        }
        return b.build()
    }

    // ---- commands ----

    private fun done(): ListenableFuture<*> = Futures.immediateVoidFuture()

    /** The engine starts on [current] from where the list or a seek left it. */
    private fun start() {
        if (items.isEmpty() || h == 0L) return
        if (!started || pending != null) {
            expecting = current
            RustPlayerJni.playAt(h, current, pending ?: 0)
            pending = null
            started = true
        } else {
            RustPlayerJni.play(h)
        }
    }

    override fun handleSetPlayWhenReady(playWhenReady: Boolean): ListenableFuture<*> {
        if (playWhenReady && !focus()) return done()
        this.playWhenReady = playWhenReady
        whyPlayWhenReady = Player.PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST
        suppressed = Player.PLAYBACK_SUPPRESSION_REASON_NONE
        if (playWhenReady) {
            if (prepared) start()
        } else {
            RustPlayerJni.pause(h)
            unfocus()
        }
        follow()
        return done()
    }

    override fun handlePrepare(): ListenableFuture<*> {
        prepared = true
        if (h != 0L) error = null
        if (playWhenReady && focus()) start()
        follow()
        return done()
    }

    /** Stopped: the engine pauses and the place is kept, to start from after the next prepare. */
    override fun handleStop(): ListenableFuture<*> {
        if (started && pending == null) pending = RustPlayerJni.positionMs(h)
        RustPlayerJni.pause(h)
        started = false
        prepared = false
        unfocus()
        follow()
        return done()
    }

    override fun handleRelease(): ListenableFuture<*> {
        unfocus()
        follow(released = true)
        if (RustBridge.player === this) RustBridge.player = null
        main.removeCallbacks(drain)
        RustPlayerJni.destroy(h)
        return done()
    }

    override fun handleSetRepeatMode(repeatMode: Int): ListenableFuture<*> {
        repeat = repeatMode
        RustPlayerJni.setRepeat(h, repeatMode)
        return done()
    }

    /** The core already put the songs in their new order (`Controls`); the engine and the timeline follow it. */
    override fun handleSetShuffleModeEnabled(shuffleModeEnabled: Boolean): ListenableFuture<*> {
        shuffle = shuffleModeEnabled
        edited()
        return done()
    }

    override fun handleSetMediaItems(mediaItems: MutableList<MediaItem>, startIndex: Int, startPositionMs: Long): ListenableFuture<*> {
        items.clear()
        uids.clear()
        for (item in mediaItems) { items += item; uids += nextUid++ }
        edited()
        current = if (startIndex == C.INDEX_UNSET || items.isEmpty()) timeline().getFirstWindowIndex(shuffle).coerceAtLeast(0) else startIndex.coerceIn(0, items.size - 1)
        pending = if (startPositionMs == C.TIME_UNSET) 0 else startPositionMs
        started = false
        if (items.isEmpty()) RustPlayerJni.pause(h)
        // A new list while music plays plays at once, as ExoPlayer's does.
        else if (prepared && playWhenReady) start()
        return done()
    }

    override fun handleAddMediaItems(index: Int, mediaItems: MutableList<MediaItem>): ListenableFuture<*> {
        val at = index.coerceIn(0, items.size)
        val had = items.isNotEmpty()
        items.addAll(at, mediaItems)
        uids.addAll(at, List(mediaItems.size) { nextUid++ })
        if (had && at <= current) current += mediaItems.size
        edited()
        return done()
    }

    override fun handleRemoveMediaItems(fromIndex: Int, toIndex: Int): ListenableFuture<*> {
        val to = toIndex.coerceAtMost(items.size)
        if (fromIndex >= to) return done()
        val gone = current in fromIndex until to
        items.subList(fromIndex, to).clear()
        uids.subList(fromIndex, to).clear()
        when {
            current >= to -> current -= to - fromIndex
            gone -> current = fromIndex.coerceAtMost(items.size - 1).coerceAtLeast(0)
        }
        edited()
        if (items.isEmpty()) {
            RustPlayerJni.pause(h)
            started = false
            pending = 0
        } else if (gone) {
            // The song playing went: the one after it plays, as media3 moves on.
            pending = 0
            if (prepared && playWhenReady) start() else started = false
        }
        return done()
    }

    override fun handleMoveMediaItems(fromIndex: Int, toIndex: Int, newIndex: Int): ListenableFuture<*> {
        val on = uids.getOrNull(current)
        val moving = items.subList(fromIndex, toIndex).toList()
        val movingUids = uids.subList(fromIndex, toIndex).toList()
        items.subList(fromIndex, toIndex).clear()
        uids.subList(fromIndex, toIndex).clear()
        val at = newIndex.coerceIn(0, items.size)
        items.addAll(at, moving)
        uids.addAll(at, movingUids)
        on?.let { u -> current = uids.indexOf(u).coerceAtLeast(0) }
        edited()
        return done()
    }

    /**
     * A seek, a skip or a tap on a song of the queue. Playing, the engine goes there at once (with its own
     * dip); paused, it waits here until play, as a paused ExoPlayer fetches nothing either. A seek in the
     * song heard is a jump to it as well: in the last seconds of a song the engine is already reading the
     * next one, and its own seek would land there.
     */
    override fun handleSeek(mediaItemIndex: Int, positionMs: Long, seekCommand: Int): ListenableFuture<*> {
        if (items.isEmpty()) return done()
        val target = if (mediaItemIndex == C.INDEX_UNSET) current else mediaItemIndex.coerceIn(0, items.size - 1)
        val ms = if (positionMs == C.TIME_UNSET) 0 else positionMs.coerceAtLeast(0)
        val live = started && pending == null && prepared && playWhenReady
        if (!live) {
            current = target
            pending = ms
        } else {
            if (target != current) expecting = target
            current = target
            RustPlayerJni.playAt(h, target, ms)
        }
        return done()
    }

    private fun edited() {
        edits++
        RustPlayerJni.queueChanged(h)
    }

    // ---- the engine's events ----

    /** Called on one of the engine's threads: its events are taken on the main thread. */
    internal fun signal() { main.post(drain) }

    private val drain = Runnable {
        while (true) {
            val e = RustPlayerJni.event(h)
            if (e < 0) break
            val arg = e.toInt()
            when ((e ushr 32).toInt()) {
                EVENT_STATE -> onState(arg)
                EVENT_SONG -> onSong(arg, RustPlayerJni.eventText(h))
                EVENT_ERROR -> android.util.Log.w("nori", "rust player: ${RustPlayerJni.eventText(h)}")
                // The output device's own sound is DeviceSound's, from Outputs, as on the ExoPlayer path.
                else -> RustPlayerJni.eventText(h)
            }
        }
        follow()
        invalidateState()
    }

    private fun onState(state: Int) {
        engineState = state
        when {
            // Played to the end of the queue: ExoPlayer keeps wanting to play, and says it ended.
            state == ENGINE_ENDED -> {}
            // The engine stopped by itself (the queue's rules after songs that would not play).
            state == ENGINE_PAUSED && playWhenReady && suppressed == Player.PLAYBACK_SUPPRESSION_REASON_NONE -> {
                playWhenReady = false
                whyPlayWhenReady = Player.PLAY_WHEN_READY_CHANGE_REASON_END_OF_MEDIA_ITEM
            }
            state == ENGINE_IDLE && started -> {
                error = PlaybackException("the audio output would not open", null, PlaybackException.ERROR_CODE_AUDIO_TRACK_INIT_FAILED)
            }
        }
    }

    private fun onSong(index: Int, id: String?) {
        // The engine's index is into the queue it last read; the id says which song, should an edit have
        // moved it since.
        val i = if (items.getOrNull(index)?.mediaId == id) index else items.indices.filter { items[it].mediaId == id }.minByOrNull { kotlin.math.abs(it - index) } ?: return
        val asked = i == expecting
        expecting = -1
        current = i
        if (asked) return
        // A song ending into the next one.
        moved = true
        if (pauseAtEndOfItem) {
            pauseAtEndOfItem = false
            RustPlayerJni.pause(h)
            playWhenReady = false
            whyPlayWhenReady = Player.PLAY_WHEN_READY_CHANGE_REASON_END_OF_MEDIA_ITEM
        }
    }

    // ---- the platform around the player, as ExoPlayer runs it ----

    private val audio = context.getSystemService(AudioManager::class.java)
    private val focusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
        .setAudioAttributes(android.media.AudioAttributes.Builder().setUsage(android.media.AudioAttributes.USAGE_MEDIA).setContentType(android.media.AudioAttributes.CONTENT_TYPE_MUSIC).build())
        .setOnAudioFocusChangeListener({ change -> onFocus(change) }, main)
        .build()
    private var focused = false

    private fun focus(): Boolean {
        if (!focused) focused = audio.requestAudioFocus(focusRequest) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        return focused
    }

    private fun unfocus() {
        if (focused) audio.abandonAudioFocusRequest(focusRequest)
        focused = false
    }

    /** A call or another player: paused for good, or until it is over. Ducking the system does itself. */
    private fun onFocus(change: Int) {
        when (change) {
            AudioManager.AUDIOFOCUS_LOSS -> {
                focused = false
                if (playWhenReady) {
                    RustPlayerJni.pause(h)
                    playWhenReady = false
                    whyPlayWhenReady = Player.PLAY_WHEN_READY_CHANGE_REASON_AUDIO_FOCUS_LOSS
                }
            }
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> if (playWhenReady) {
                RustPlayerJni.pause(h)
                suppressed = Player.PLAYBACK_SUPPRESSION_REASON_TRANSIENT_AUDIO_FOCUS_LOSS
            }
            AudioManager.AUDIOFOCUS_GAIN -> if (suppressed != Player.PLAYBACK_SUPPRESSION_REASON_NONE) {
                suppressed = Player.PLAYBACK_SUPPRESSION_REASON_NONE
                if (playWhenReady) start()
            }
        }
        follow()
        invalidateState()
    }

    /** Headphones pulled out: paused, as ExoPlayer's `setHandleAudioBecomingNoisy` does. */
    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (!playWhenReady) return
            RustPlayerJni.pause(h)
            playWhenReady = false
            whyPlayWhenReady = Player.PLAY_WHEN_READY_CHANGE_REASON_AUDIO_BECOMING_NOISY
            follow()
            invalidateState()
        }
    }
    private var listening = false

    @Suppress("DEPRECATION")
    private val wakeLock = context.getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "nori:engine").apply { setReferenceCounted(false) }

    /** The receiver and the CPU lock are held exactly while music is wanted, as ExoPlayer's WAKE_MODE_LOCAL. */
    private fun follow(released: Boolean = false) {
        val playing = !released && playWhenReady && prepared && engineState != ENGINE_ENDED
        if (playing && !listening) {
            val filter = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
            if (Build.VERSION.SDK_INT >= 33) context.registerReceiver(noisy, filter, Context.RECEIVER_NOT_EXPORTED) else context.registerReceiver(noisy, filter)
            listening = true
        } else if (!playing && listening) {
            context.unregisterReceiver(noisy)
            listening = false
        }
        if (playing && !wakeLock.isHeld) wakeLock.acquire() else if (!playing && wakeLock.isHeld) wakeLock.release()
    }

    // ---- what the Rust side asks for (RustBridge) ----

    /**
     * The engine's AudioTrack, opened as the ExoPlayer path opens its own: music attributes, a deep buffer
     * in the power-saving mode (the framework's deep-buffer output, which wakes least), pinned to a DAC
     * when bit-perfect is on, and the route told to the engine whenever it changes. Called on the engine's
     * thread.
     */
    internal fun openTrack(rate: Int, channels: Int, float: Boolean, frames: Int): AudioTrack? = runCatching {
        val encoding = if (float) AudioFormat.ENCODING_PCM_FLOAT else AudioFormat.ENCODING_PCM_16BIT
        // The DAC's mixer attributes are read by the framework when the track is built: set for this format first.
        nori.dac.onFormat(rate, encoding)
        val bitPerfect = nori.dac.state.value.bitPerfect
        val track = AudioTrack.Builder()
            .setAudioAttributes(android.media.AudioAttributes.Builder().setUsage(android.media.AudioAttributes.USAGE_MEDIA).setContentType(android.media.AudioAttributes.CONTENT_TYPE_MUSIC).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(rate).setEncoding(encoding).setChannelMask(if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO).build())
            .setTransferMode(AudioTrack.MODE_STREAM)
            .setBufferSizeInBytes(frames * channels * if (float) 4 else 2)
            .setPerformanceMode(if (bitPerfect) AudioTrack.PERFORMANCE_MODE_NONE else AudioTrack.PERFORMANCE_MODE_POWER_SAVING)
            .build()
        // Started with a quarter of a second in it rather than once full: a seek is heard as soon as its
        // first burst is decoded. Before Android 12 a track waits to be full, and the engine fills it.
        if (Build.VERSION.SDK_INT >= 31) runCatching { track.setStartThresholdInFrames(rate / 4) }
        nori.dac.preferredDevice()?.let { runCatching { track.setPreferredDevice(it) } }
        nori.dac.onTrack(rate, encoding, false)
        track.addOnRoutingChangedListener(AudioRouting.OnRoutingChangedListener { r ->
            r.routedDevice?.let { d -> RustPlayerJni.device(h, d.type, d.productName?.toString()) }
        }, main)
        val mode = if (track.performanceMode == AudioTrack.PERFORMANCE_MODE_POWER_SAVING) "power saving" else "normal"
        android.util.Log.i("nori", "rust AudioTrack: $rate Hz x$channels enc=$encoding, ${track.bufferSizeInFrames} of $frames frames (${track.bufferSizeInFrames * 1000L / rate} ms), $mode, bitPerfect=$bitPerfect")
        track
    }.onFailure { android.util.Log.w("nori", "rust AudioTrack would not open", it) }.getOrNull()

    /**
     * A song's bytes from [from] on, through the same data sources ExoPlayer reads: a download, then the
     * stream cache, then the network on the app's one OkHttp client (its TLS, certificates and headers),
     * the quality resolved now. Called on a loader thread; while one is open the service holds the Wi-Fi lock.
     */
    internal fun open(url: String, from: Long): RustBody? {
        val source = nori.sources.factory.createDataSource()
        val length = try {
            source.open(DataSpec.Builder().setUri(Uri.parse(url)).setPosition(from).build())
        } catch (e: Exception) {
            runCatching { source.close() }
            android.util.Log.w("nori", "rust player: $url would not open: $e")
            return null
        }
        loaded(+1)
        return RustBody(source, if (length == C.LENGTH_UNSET.toLong()) -1 else length) { loaded(-1) }
    }

    private fun loaded(by: Int) {
        val was = loading > 0
        synchronized(this) { loading += by }
        if (was != loading > 0) main.post { invalidateState() }
    }

    /** The cache key [id] resolves to now: a download's, or the stream's at this network's quality. */
    internal fun key(id: String): String? = runCatching { nori.sources.resolve(DataSpec(songUri(id))).key }.getOrNull()

    // Last, once everything above exists: from here on the engine's threads may call in.
    init {
        RustBridge.player = this
        if (h == 0L) error = PlaybackException("the Rust player would not start", null, PlaybackException.ERROR_CODE_FAILED_RUNTIME_CHECK)
    }

    private companion object {
        const val ENGINE_IDLE = 0
        const val ENGINE_PAUSED = 2
        const val ENGINE_ENDED = 3
        const val EVENT_STATE = 0
        const val EVENT_SONG = 1
        const val EVENT_ERROR = 2

        val ATTRIBUTES: AudioAttributes = AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build()

        /** Volume and speed are the engine's (fades, ReplayGain, the settings), so they are not offered. */
        val COMMANDS: Player.Commands = Player.Commands.Builder().addAll(
            Player.COMMAND_PLAY_PAUSE, Player.COMMAND_PREPARE, Player.COMMAND_STOP, Player.COMMAND_RELEASE,
            Player.COMMAND_SEEK_TO_DEFAULT_POSITION, Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM,
            Player.COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM, Player.COMMAND_SEEK_TO_PREVIOUS,
            Player.COMMAND_SEEK_TO_NEXT_MEDIA_ITEM, Player.COMMAND_SEEK_TO_NEXT, Player.COMMAND_SEEK_TO_MEDIA_ITEM,
            Player.COMMAND_SEEK_BACK, Player.COMMAND_SEEK_FORWARD,
            Player.COMMAND_SET_REPEAT_MODE, Player.COMMAND_SET_SHUFFLE_MODE,
            Player.COMMAND_GET_CURRENT_MEDIA_ITEM, Player.COMMAND_GET_TIMELINE, Player.COMMAND_GET_METADATA,
            Player.COMMAND_SET_MEDIA_ITEM, Player.COMMAND_CHANGE_MEDIA_ITEMS, Player.COMMAND_GET_AUDIO_ATTRIBUTES,
            Player.COMMAND_GET_VOLUME,
        ).build()
    }
}

/**
 * The queue as media3's timeline: one window per song, walked in the core's play order under shuffle
 * ([PlaylistJni.order]) so that next and previous, the notification and the controllers' queue land where
 * the engine goes.
 */
@UnstableApi
private class QueueTimeline(private val items: List<MediaItem>, private val uids: LongArray, private val order: IntArray?) : Timeline() {
    private val byUid = HashMap<Any, Int>(uids.size * 2).apply { uids.forEachIndexed { i, u -> put(u, i) } }
    /** Where each song is in [order]. */
    private val place = order?.let { o -> IntArray(o.size).also { p -> o.forEachIndexed { k, i -> if (i in p.indices) p[i] = k } } }

    override fun getWindowCount() = items.size
    override fun getPeriodCount() = items.size

    private fun durationUs(i: Int): Long = items[i].mediaMetadata.durationMs?.takeIf { it > 0 }?.let { it * 1000 } ?: C.TIME_UNSET

    override fun getWindow(windowIndex: Int, window: Window, defaultPositionProjectionUs: Long): Window =
        window.set(uids[windowIndex], items[windowIndex], null, C.TIME_UNSET, C.TIME_UNSET, C.TIME_UNSET, true, false, null, 0, durationUs(windowIndex), windowIndex, windowIndex, 0)

    override fun getPeriod(periodIndex: Int, period: Period, setIds: Boolean): Period =
        period.set(if (setIds) uids[periodIndex] else null, if (setIds) uids[periodIndex] else null, periodIndex, durationUs(periodIndex), 0)

    override fun getIndexOfPeriod(uid: Any): Int = byUid[uid] ?: C.INDEX_UNSET
    override fun getUidOfPeriod(periodIndex: Int): Any = uids[periodIndex]

    private fun shuffled(shuffle: Boolean) = shuffle && order != null && place != null

    override fun getFirstWindowIndex(shuffle: Boolean): Int = when {
        isEmpty -> C.INDEX_UNSET
        shuffled(shuffle) -> order!![0]
        else -> 0
    }

    override fun getLastWindowIndex(shuffle: Boolean): Int = when {
        isEmpty -> C.INDEX_UNSET
        shuffled(shuffle) -> order!![order.size - 1]
        else -> items.size - 1
    }

    override fun getNextWindowIndex(windowIndex: Int, repeatMode: Int, shuffle: Boolean): Int {
        if (repeatMode == Player.REPEAT_MODE_ONE) return windowIndex
        if (windowIndex == getLastWindowIndex(shuffle)) return if (repeatMode == Player.REPEAT_MODE_ALL) getFirstWindowIndex(shuffle) else C.INDEX_UNSET
        return if (shuffled(shuffle)) order!![place!![windowIndex] + 1] else windowIndex + 1
    }

    override fun getPreviousWindowIndex(windowIndex: Int, repeatMode: Int, shuffle: Boolean): Int {
        if (repeatMode == Player.REPEAT_MODE_ONE) return windowIndex
        if (windowIndex == getFirstWindowIndex(shuffle)) return if (repeatMode == Player.REPEAT_MODE_ALL) getLastWindowIndex(shuffle) else C.INDEX_UNSET
        return if (shuffled(shuffle)) order!![place!![windowIndex] - 1] else windowIndex - 1
    }
}
