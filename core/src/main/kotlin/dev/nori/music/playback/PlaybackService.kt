package dev.nori.music.playback

import dev.nori.music.ffi.queue.FillNext
import dev.nori.music.ffi.queue.Hand
import dev.nori.music.ffi.queue.OnError
import dev.nori.music.ffi.model.PlaybackError
import dev.nori.music.ffi.model.Switch
import android.app.AlarmManager
import android.app.PendingIntent
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.LruCache
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.common.TrackSelectionParameters.AudioOffloadPreferences
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.DefaultLoadControl
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.ShuffleOrder.DefaultShuffleOrder
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DefaultAudioSink
import androidx.media3.exoplayer.audio.DefaultAudioTrackBufferSizeProvider
import androidx.media3.exoplayer.DecoderReuseEvaluation
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.extractor.DefaultExtractorsFactory
import androidx.media3.datasource.DataSourceBitmapLoader
import androidx.media3.session.CacheBitmapLoader
import androidx.media3.session.CommandButton
import android.net.wifi.WifiManager
import androidx.media3.session.LibraryResult
import androidx.media3.session.MediaLibraryService
import androidx.media3.session.MediaSession
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionResult
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import dev.nori.music.Nori
import dev.nori.music.data.StarKind
import dev.nori.music.ffi.model.PlayQueue
import dev.nori.music.ffi.model.Song
import dev.nori.music.settings.Prefs
import dev.nori.music.settings.ReplayGainMode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.guava.future
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The one place audio is played. Built for the screen-off case: decoding is
 * offloaded to the audio DSP when the device can, buffers are large so the
 * radio works in bursts, and nothing here polls or ticks while music plays.
 */
@UnstableApi
class PlaybackService : MediaLibraryService() {
    companion object {
        const val CMD_SLEEP = "nori.sleep"
        const val CMD_TUNING = "nori.tuning"
        /** The notification's and lock screen's heart: favourite or unfavourite the current song. */
        const val CMD_FAVOURITE = "nori.favourite"
        /** The notification's and lock screen's shuffle toggle. */
        const val CMD_SHUFFLE = "nori.shuffle"
        /**
         * Next was pressed with nothing after the current song. Autofill may still be fetching similar
         * songs: remember the skip and take it when they land, instead of the press dying as a no-op.
         */
        const val CMD_FILL_NEXT = "nori.fillNext"
        /** Broadcast inside the package on every track or play-state change; what a home-screen widget listens to. */
        const val ACTION_STATE = "dev.nori.music.STATE"
        const val EXTRA_TITLE = "title"
        const val EXTRA_ARTIST = "artist"
        const val EXTRA_PLAYING = "playing"
        const val ARG_ON = "on"
        const val ARG_MINUTES = "minutes"
        const val ARG_END_OF_TRACK = "endOfTrack"
        const val ARG_SONGS = "songs"
        /** Whether the chain is currently asking for offload; read by the test bridge, which cannot see in here. */
        @Volatile var offloadWanted = false
        /** The Rust player while it is the one playing; read by the test bridge. */
        @Volatile var rustPlayer: EnginePlayer? = null
            private set
        /** The player the running service built, "exoplayer" or "rust"; null with no service. For the perf recorder. */
        @Volatile var engine: String? = null
            private set
        /** The AudioTrack the player last opened, and what was asked of it; null with no service. For the perf recorder. */
        @Volatile var track: OpenedTrack? = null
            internal set
    }

    private lateinit var nori: Nori
    /** Whichever player plays: [exo], or [rust] when the "Playback engine" setting said Rust at start. */
    private lateinit var player: Player
    private var exo: ExoPlayer? = null
    private var rust: EnginePlayer? = null
    private lateinit var session: MediaLibrarySession
    /** The player as the session sees it; every change to the queue goes through here, to the core first. */
    private lateinit var controls: Controls
    private lateinit var scrobbler: Scrobbler
    private val equalizer = Equalizer()
    private var hiRes = false
    /** Kept so a freshly measured track can have its transition planned again; see AutoMixPrefetch. */
    private var transitionSink: TransitionSink? = null
    @Suppress("DEPRECATION")
    private val wifiLock by lazy { applicationContext.getSystemService(WifiManager::class.java).createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "nori:loading").apply { setReferenceCounted(false) } }
    private var offloaded = false
    /** The speed and pitch the sound is being made with; see applyAudio. */
    /**
     * The sink failed to open or to write once. Offload is the only part of the chain that can fail on a
     * device the app cannot see into (a USB DAC, a dock, a car head unit), so it is given up for the life of
     * the service rather than retried into a loop of silent tracks.
     */
    private var offloadRefused = false
    private lateinit var precacher: Precacher
    private lateinit var analyser: AutoMixPrefetch
    private val precache = Runnable { precacheAhead(); analyseAhead() }
    private val measure = Runnable { analyseAhead() }
    /** How long each chore waits (crates/core/src/rules.rs playback_timings), read once. */
    private val timings by lazy { dev.nori.music.ffi.queue.playbackTimings() }
    /** What the volume should be once no fade is running: 1, or the ReplayGain attenuation. */
    private var targetVolume = 1f
    private var fade: Runnable? = null
    private var offlineBridge: OfflineBridge? = null
    /**
     * When the output is rebuilt: for the equalizer screen's shallow buffer and back, and for settings
     * that need a new chain - at the next boundary or pause while music plays, at once otherwise. The
     * rules are nori_player::transport::Chain's.
     */
    private val chain = dev.nori.music.ffi.queue.ChainState()
    /**
     * The last thing that happens before the AudioTrack exists, and the only place that knows exactly what it
     * will be: encoding, rate and whether the stream is being offloaded. Preferred mixer attributes are read
     * by the framework when the track is built, so a DAC can only be engaged from here - setting them
     * afterwards, as this used to, changes nothing about the track already playing.
     */
    private val tracks = DefaultAudioSink.AudioTrackProvider { config, attrs, sessionId, context ->
        nori.dac.onFormat(config.sampleRate, config.encoding)
        val track = DefaultAudioSink.AudioTrackProvider.DEFAULT.getAudioTrack(config, attrs, sessionId, context)
        // Pin the track to the DAC as well: bit-perfect attributes apply to one device, and letting Android
        // pick the route again afterwards is how the two end up disagreeing.
        nori.dac.preferredDevice()?.let { runCatching { track.setPreferredDevice(it) } }
        nori.dac.onTrack(config.sampleRate, config.encoding, config.offload)
        android.util.Log.i("nori", "AudioTrack ${config.sampleRate} Hz enc=${config.encoding} buffer=${config.bufferSize} offload=${config.offload} usb=${nori.outputs.usb.value}")
        // media3 asks for no performance mode of its own.
        Companion.track = OpenedTrack(track, "exoplayer", config.bufferSize, AudioTrack.PERFORMANCE_MODE_NONE)
        track
    }

    private val shallowBuffer = DefaultAudioTrackBufferSizeProvider.Builder().build()
    private val deepBuffer = DefaultAudioTrackBufferSizeProvider.Builder().setTargetPcmBufferDurationUs(dev.nori.music.ffi.settings.burstBufferUs().toInt()).setMaxPcmBufferDurationUs(dev.nori.music.ffi.settings.burstBufferUs().toInt()).build()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val main = Handler(Looper.getMainLooper())
    private val served = LruCache<String, MediaItem>(500)
    private val saveQueue = Runnable { persistQueue(push = false) }
    /**
     * Paused for a long while: the output goes, and with it media3's once-a-second tick, so the phone sleeps.
     * The queue and the place in the song stay (nori_player::transport::IDLE_RELEASE_MS).
     */
    private val idleRelease = Runnable {
        if (!player.playWhenReady && player.playbackState != Player.STATE_IDLE) {
            android.util.Log.i("nori", "paused a long while: output released")
            persistQueue(push = false)
            player.stop()
        }
    }
    private val sleepAlarm = AlarmManager.OnAlarmListener { player.pause() }

    override fun onCreate() {
        super.onCreate()
        nori = Nori.get(this)
        scrobbler = Scrobbler(nori, scope)
        hiRes = nori.settings.value.hiRes
        // Read once, here: the two players cannot be swapped under a running session.
        player = if (nori.settings.value.playbackEngine == 1) {
            android.util.Log.i("nori", "playback engine: Rust")
            EnginePlayer(this, nori).also { rust = it; rustPlayer = it }
        } else exoPlayer().also { exo = it }
        engine = if (rust != null) "rust" else "exoplayer"
        // A song fetched ahead can be measured now rather than at the next queue move, which for the song
        // after the first one streamed is after its boundary (see AutoMixPrefetch).
        precacher = Precacher(nori.sources) { main.removeCallbacks(measure); main.post(measure) }
        analyser = AutoMixPrefetch(nori.sources, { nori.core }) { transitionSink?.replan(); rust?.replan() }
        // The bridge edits ExoPlayer's list and reads its errors; the Rust player has none yet.
        exo?.let { offlineBridge = OfflineBridge(this, it, { nori.core }, main, ::applyEdit) }
        player.addListener(listener)

        // Fired from the audio device callback (main) and from the audio track provider (playback thread);
        // the player may only be touched on the main looper.
        nori.dac.onChanged = { main.post { applyAudio(nori.settings.value); applyGain() } }
        nori.dac.start()
        nori.outputs.start()
        // Plugging in headphones or a DAC swaps the whole sound chain, if a profile is bound to it.
        scope.launch {
            // A device arriving or leaving changes what the audio chain may do (see applyAudio), whether or
            // not the user binds sound profiles to outputs.
            nori.outputs.usb.collect { applyAudio(nori.settings.value) }
        }
        scope.launch {
            // The device's own sound (a bound profile or AutoEQ curve), or the sound from before it came back.
            nori.outputs.current.collect { output -> nori.deviceSound.onOutput(output) }
        }
        applyAudio(nori.settings.value)
        scope.launch {
            // What a change asks of the player is the core's call (settings_store.rs); the sound chain and
            // the planner's settings follow there by themselves, so a screen's setting costs nothing here.
            nori.settings.effects.collect { e ->
                if (e and 1 != 0) applyAudio(nori.settings.value)
                // The sound chain's values alone: ExoPlayer's chain reads them from the core as it runs; the
                // Rust player keeps its own and is handed them.
                else if (e and 8 != 0) rust?.applySettings()
                if (e and 2 != 0) applyGain()
                if (e and 4 != 0) setupTransitions()
            }
        }

        val open = packageManager.getLaunchIntentForPackage(packageName)?.let { PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE) }
        controls = Controls(player)
        session = MediaLibrarySession.Builder(this, controls, Callback())
            // Notification and lock-screen art: same connection pool as everything else, last bitmap kept, decoded no larger than needed.
            .setBitmapLoader(CacheBitmapLoader(DataSourceBitmapLoader.Builder(this).setDataSourceFactory(nori.sources.network).setMaximumOutputDimension(512).build()))
            // Controllers extrapolate the playhead themselves; a broadcast every few seconds is a wake-up for nothing.
            .setPeriodicPositionUpdateEnabled(false)
            .apply { open?.let(::setSessionActivity) }.build()
        // The notification and the lock screen carry the app's own mark, not media3's stock play circle.
        // Its id stays media3's default (1001): the download notification lives on 2001 so the two never replace each other.
        setMediaNotificationProvider(
            androidx.media3.session.DefaultMediaNotificationProvider.Builder(this)
                .setNotificationId(androidx.media3.session.DefaultMediaNotificationProvider.DEFAULT_NOTIFICATION_ID).build()
                .apply { setSmallIcon(dev.nori.music.core.R.drawable.ic_notification) },
        )
        // A heart changed anywhere in the app (or by the notification itself) redraws the notification's heart.
        // A StateFlow: it emits only on a change, so this is idle while music plays untouched.
        scope.launch { nori.library.starMarks.collect { refreshButtons() } }
        restoreQueue()
    }

    /** ExoPlayer over the core's decoder, sound chain and transition engine: the default path. */
    private fun exoPlayer(): ExoPlayer {
        Equalizer.active = equalizer
        val renderers = object : DefaultRenderersFactory(this) {
            // The core decodes what it can (RustAudio.kt), ahead of MediaCodec, which takes the rest.
            override fun buildAudioRenderers(
                context: android.content.Context, extensionRendererMode: Int, mediaCodecSelector: androidx.media3.exoplayer.mediacodec.MediaCodecSelector,
                enableDecoderFallback: Boolean, audioSink: AudioSink, eventHandler: Handler, eventListener: androidx.media3.exoplayer.audio.AudioRendererEventListener,
                out: java.util.ArrayList<androidx.media3.exoplayer.Renderer>,
            ) {
                out += RustAudioRenderer(eventHandler, eventListener, audioSink) { hiRes }
                super.buildAudioRenderers(context, extensionRendererMode, mediaCodecSelector, enableDecoderFallback, audioSink, eventHandler, eventListener, out)
            }
            override fun buildAudioSink(context: android.content.Context, enableFloatOutput: Boolean, enableAudioTrackPlaybackParams: Boolean): AudioSink =
                TransitionSink(
                    DefaultAudioSink.Builder(context).setAudioProcessorChain(SoundChain(equalizer))
                        // Formats the DSP cannot take (FLAC on most phones) are decoded on the CPU, and fed in bursts (nori_player::burst).
                        .setAudioTrackBufferSizeProvider { min, encoding, mode, frameSize, rate, bitrate, speed ->
                            // Deep for bursts; shallow only while the equalizer screen is open, so that moving a band is heard at once.
                            (if (chain.isTuning()) shallowBuffer else deepBuffer).getBufferSizeInBytes(min, encoding, mode, frameSize, rate, bitrate, speed)
                        }
                        .setEnableFloatOutput(enableFloatOutput).setEnableAudioTrackPlaybackParams(enableAudioTrackPlaybackParams)
                        .setAudioTrackProvider(tracks).build()
                ).also { transitionSink = it; updateBurst() }
        }
        renderers.setEnableAudioFloatOutput(hiRes)
        val player = ExoPlayer.Builder(this, renderers)
            // ID3 metadata is not read: the covers, tags and ReplayGain all come from the server, and a song's embedded
            // picture (a few MB each) stayed on the heap for the song playing and the next one. The frames gapless
            // playback needs are still read (media3's REQUIRED_ID3_FRAME_PREDICATE).
            .setMediaSourceFactory(DefaultMediaSourceFactory(this, DefaultExtractorsFactory().setConstantBitrateSeekingEnabled(true)
                .setMp3ExtractorFlags(androidx.media3.extractor.mp3.Mp3Extractor.FLAG_DISABLE_ID3_METADATA)
                .setFlacExtractorFlags(androidx.media3.extractor.flac.FlacExtractor.FLAG_DISABLE_ID3_METADATA)).setDataSourceFactory(nori.sources.factory))
            .setAudioAttributes(AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build(), true)
            .setHandleAudioBecomingNoisy(true)
            // The CPU lock only. media3's network mode would pin Wi-Fi out of power save for as long as music plays;
            // a track is fetched in seconds and then played from memory, so the Wi-Fi lock is held just while loading.
            .setWakeMode(C.WAKE_MODE_LOCAL)
            // The playback thread sleeps until the renderers can make progress instead of looping every 10 ms.
            .experimentalSetDynamicSchedulingEnabled(true)
            .setLoadControl(
                // Fill up to ten minutes in one go, then leave the network alone until a minute is left. The cap is a
                // quarter of the app's heap class, at most 48 MB: a whole MP3/Opus track everywhere, FLAC in two or three fetches.
                dev.nori.music.ffi.queue.loadControl(getSystemService(android.app.ActivityManager::class.java).memoryClass.toUInt()).let { c ->
                    DefaultLoadControl.Builder().setBufferDurationsMs(c[0].toInt(), c[1].toInt(), c[2].toInt(), c[3].toInt())
                        .setTargetBufferBytes(c[4].toInt()).setPrioritizeTimeOverSizeThresholds(false).build()
                }
            )
            .build()
        // A dropout is otherwise invisible: the output ran dry and the listener heard a gap, but nothing
        // says so. Named in the log, with whether a mix was on, so "a cut on the change" has a trace.
        player.addAnalyticsListener(object : androidx.media3.exoplayer.analytics.AnalyticsListener {
            override fun onAudioUnderrun(
                eventTime: androidx.media3.exoplayer.analytics.AnalyticsListener.EventTime,
                bufferSize: Int, bufferSizeMs: Long, elapsedSinceLastFeedMs: Long,
            ) {
                android.util.Log.w("nori", "audio underrun: ${bufferSizeMs} ms buffer, ${elapsedSinceLastFeedMs} ms since last feed, mixing=${TransitionSink.mixing}, holding=${TransitionSink.holding}")
            }
            override fun onAudioSinkError(eventTime: androidx.media3.exoplayer.analytics.AnalyticsListener.EventTime, audioSinkError: Exception) {
                android.util.Log.w("nori", "audio sink error: $audioSinkError")
            }
        })
        player.addAudioOffloadListener(object : ExoPlayer.AudioOffloadListener {
            override fun onOffloadedPlayback(offloaded: Boolean) { this@PlaybackService.offloaded = offloaded; updateBurst() }
        })
        return player
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo) = session

    override fun onTaskRemoved(rootIntent: android.content.Intent?) {
        if (!player.playWhenReady || player.mediaItemCount == 0) stopSelf()
    }

    override fun onDestroy() {
        Equalizer.active = null
        rustPlayer = null
        engine = null
        track = null
        persistQueue(push = false)
        getSystemService(AlarmManager::class.java).cancel(sleepAlarm)
        main.removeCallbacks(precache)
        main.removeCallbacks(measure)
        main.removeCallbacks(idleRelease)
        precacher.release()
        analyser.release()
        offlineBridge?.abandon()
        offlineBridge = null
        nori.dac.onChanged = {}
        nori.dac.stop()
        nori.outputs.stop()
        if (wifiLock.isHeld) wifiLock.release()
        session.release()
        player.release()
        scope.cancel()
        super.onDestroy()
    }

    // ---- what changes with the track ----

    private val listener = object : Player.Listener {
        override fun onMediaItemTransition(item: MediaItem?, reason: Int) {
            val looped = reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT
            // The Rust player walked the core's queue itself (explicit songs skipped, the ReplayGain volume
            // set): this is only the song the ear arrived on.
            if (rust != null) return arrived(item, if (looped) dev.nori.music.ffi.queue.TrackChange.LOOPED else dev.nori.music.ffi.queue.TrackChange.MOVED)
            // The core follows the player onto the song and says what arriving there means (an explicit
            // song skipped, a repeat loop, a new song; crates/core/src/playlist.rs playlist_transition).
            val arrival = dev.nori.music.ffi.queue.playlistTransition(player.currentMediaItemIndex, looped)
            // A settings change that needed a new chain waited for a boundary instead of cutting the
            // track: this is it. Skipped on a repeat-one loop, which should restart seamlessly; the
            // swap then waits for a real boundary. A planned crossfade into this track dies with the
            // rebuild - the setting wins over one mix.
            if (chain.boundary(looped && player.repeatMode == Player.REPEAT_MODE_ONE)) {
                android.util.Log.i("nori", "chain swap at the boundary")
                if (player.playbackState != Player.STATE_IDLE) { player.stop(); player.prepare() }
            }
            refreshButtons()
            when (arrival) {
                dev.nori.music.ffi.queue.Onto.SKIP -> return player.seekToNextMediaItem()
                dev.nori.music.ffi.queue.Onto.LOOP -> return scrobbler.onTrack(item?.mediaId, dev.nori.music.ffi.queue.TrackChange.LOOPED, player.isPlaying)
                dev.nori.music.ffi.queue.Onto.SONG -> scrobbler.onTrack(item?.mediaId, dev.nori.music.ffi.queue.TrackChange.MOVED, player.isPlaying)
            }
            applyGain()
            arrived(item, null)
        }

        /** A song reached: scrobbled as [change] (none when already told), then what follows a new song. */
        private fun arrived(item: MediaItem?, change: dev.nori.music.ffi.queue.TrackChange?) {
            if (change != null) {
                refreshButtons()
                scrobbler.onTrack(item?.mediaId, change, player.isPlaying)
                if (change == dev.nori.music.ffi.queue.TrackChange.LOOPED) return
            }
            scheduleSave()
            autoFill()
            if (nori.settings.value.bridgeOffline) offlineBridge?.onTrack()
            announce()
            refreshUpcoming()
            // A few seconds in, the current track has been fetched and the radio is still up: fetch ahead now.
            main.removeCallbacks(precache)
            main.postDelayed(precache, timings.precacheAfterMs)
            if (dev.nori.music.ffi.queue.sleepSongChanged()) pauseAtEnd(true)
        }

        override fun onIsLoadingChanged(isLoading: Boolean) {
            if (isLoading && !wifiLock.isHeld) wifiLock.acquire() else if (!isLoading && wifiLock.isHeld) wifiLock.release()
            // A burst of loading has ended, and the player fetches the next song itself as the one before
            // is whole: that song is on the device now, and the precacher, finding it being written,
            // fetched nothing and said nothing. Measuring looks again (nothing at all while AutoMix is off).
            if (!isLoading) { main.removeCallbacks(measure); main.postDelayed(measure, timings.measureAfterEditMs) }
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            // Sound is coming out: whatever failed before it is no longer a run (rules.rs queue_playing).
            if (isPlaying) dev.nori.music.ffi.queue.queuePlaying()
            // Paused is silent: the deep buffer comes back at once rather than waiting a song, and the
            // rebuild carries any other pending swap with it.
            if (!isPlaying && !player.playWhenReady && exo != null && chain.paused()) reconfigureSink(urgent = true)
            announce()
            scrobbler.onPlaying(isPlaying)
            if (!isPlaying && !player.playWhenReady) persistQueue(push = true)
            main.removeCallbacks(idleRelease)
            if (!isPlaying && !player.playWhenReady && player.playbackState != Player.STATE_IDLE && exo != null) main.postDelayed(idleRelease, timings.idleReleaseMs)
        }

        override fun onShuffleModeEnabledChanged(on: Boolean) { refreshUpcoming(); refreshButtons() }
        override fun onRepeatModeChanged(mode: Int) = refreshUpcoming()

        override fun onTimelineChanged(timeline: androidx.media3.common.Timeline, reason: Int) {
            if (reason == Player.TIMELINE_CHANGE_REASON_PLAYLIST_CHANGED && !editing) follow()
            refreshUpcoming()
            if (reason == Player.TIMELINE_CHANGE_REASON_PLAYLIST_CHANGED) {
                scheduleSave()
                // The queue was edited: what comes next is not what it was, and measuring the new next
                // track is the whole point of measuring ahead at all. Only that - the fetching ahead is
                // left alone, since restarting it would throw away a track it is halfway through.
                main.removeCallbacks(measure)
                main.postDelayed(measure, timings.measureAfterEditMs)
            }
        }

        override fun onPlayerError(error: androidx.media3.common.PlaybackException) {
            // The Rust player skips or stops by the core's rules itself; what reaches here is its output
            // or the engine failing to start, which trying again would not change.
            if (rust != null) { android.util.Log.w("nori", "rust player: $error"); return }
            // What to do is nori_player::queue::on_error's call: an output that refuses the offloaded
            // stream goes back to the CPU path, a network failure goes to the offline bridge when it is on,
            // anything else skips a few and then stops.
            // The platform only reads its exceptions into a kind; what to do, and the run of songs that
            // would not play, are the core's (nori_player::queue::on_error through crates/core/src/rules.rs).
            val sink = generateSequence(error.cause) { it.cause }.any {
                it is AudioSink.InitializationException || it is AudioSink.WriteException || it is AudioSink.ConfigurationException
            }
            val kind = when { sink -> PlaybackError.OUTPUT; error.isNetworkish() -> PlaybackError.NETWORK; else -> PlaybackError.OTHER }
            when (dev.nori.music.ffi.queue.queueError(kind, offloadRefused, offlineBridge != null)) {
                OnError.GIVE_UP_OFFLOAD -> {
                    android.util.Log.w("nori", "audio sink refused the stream, giving up offload", error)
                    offloadRefused = true
                    applyAudio(nori.settings.value)
                    player.prepare()
                    player.play()
                }
                OnError.BRIDGE -> when {
                    offlineBridge?.onPlaybackError(error) == true -> dev.nori.music.ffi.queue.queueBridged()
                    dev.nori.music.ffi.queue.queueBridgeFailed() -> skipAfterError()
                }
                OnError.SKIP -> skipAfterError()
                OnError.STOP -> {}
            }
        }

        override fun onPlaybackStateChanged(state: Int) {
            if (state == Player.STATE_ENDED) scrobbler.onTrack(null, dev.nori.music.ffi.queue.TrackChange.ENDED, false)
        }
    }

    /** The core said to skip a song that will not play (and counted it). */
    private fun skipAfterError() {
        player.seekToNextMediaItem()
        player.prepare()
        player.play()
    }

    /** What the heart and shuffle buttons last showed, so an unrelated change does not rebuild the notification. */
    private var buttonsShown: dev.nori.music.ffi.words.SessionButtons? = null

    private fun currentStarred(item: MediaItem): Boolean =
        nori.library.isStarred(StarKind.SONG, item.mediaId, dev.nori.music.ffi.queue.queueFlags(item.mediaId) and 2u != 0u)

    /**
     * Heart and shuffle beside previous / play / next, in the secondary slots the way other players put
     * them. Called when the track, the shuffle flag or a star changes - never on a timer. Whether the
     * heart shows and what each says are the core's (words.rs words_session_buttons).
     */
    private fun refreshButtons() {
        if (!::session.isInitialized) return
        val item = player.currentMediaItem
        val b = dev.nori.music.ffi.wordsSessionButtons(item != null && currentStarred(item), player.shuffleModeEnabled)
        if (b == buttonsShown) return
        buttonsShown = b
        val buttons = ArrayList<CommandButton>(2)
        b.heart?.let { words ->
            buttons += CommandButton.Builder(if (b.starred) CommandButton.ICON_HEART_FILLED else CommandButton.ICON_HEART_UNFILLED)
                .setDisplayName(words)
                .setSessionCommand(SessionCommand(CMD_FAVOURITE, Bundle.EMPTY))
                .setSlots(CommandButton.SLOT_BACK_SECONDARY, CommandButton.SLOT_OVERFLOW).build()
        }
        buttons += CommandButton.Builder(if (b.shuffling) CommandButton.ICON_SHUFFLE_ON else CommandButton.ICON_SHUFFLE_OFF)
            .setDisplayName(b.shuffle)
            .setSessionCommand(SessionCommand(CMD_SHUFFLE, Bundle.EMPTY))
            .setSlots(CommandButton.SLOT_FORWARD_SECONDARY, CommandButton.SLOT_OVERFLOW).build()
        session.setMediaButtonPreferences(buttons)
    }

    private fun updateBurst() { transitionSink?.bursting = chain.bursting(offloaded) }

    /** Pause once the song playing ends (the sleep timer's "end of this song"). */
    private fun pauseAtEnd(on: Boolean) {
        exo?.pauseAtEndOfMediaItems = on
        rust?.pauseAtEndOfItem = on
    }

    private fun announce() {
        val m = player.currentMediaItem?.mediaMetadata
        sendBroadcast(android.content.Intent(ACTION_STATE).setPackage(packageName)
            .putExtra(EXTRA_TITLE, m?.title?.toString()).putExtra(EXTRA_ARTIST, m?.artist?.toString()).putExtra(EXTRA_PLAYING, player.isPlaying))
    }

    /**
     * A processor joins or leaves the chain, and the buffer depth changes, only when the sink is
     * configured again. Mid-track that rebuild cuts the song, so unless the current path is broken
     * (an offloaded track that must come back to the CPU plays silence) or nothing is playing, it
     * waits for the next track boundary, where the swap is inaudible.
     */
    private fun reconfigureSink(urgent: Boolean = false) {
        if (exo == null) return
        if (player.playbackState == Player.STATE_IDLE || urgent) {
            if (player.playbackState != Player.STATE_IDLE) {
                android.util.Log.i("nori", "chain swap now")
                player.stop(); player.prepare()
            }
            return
        }
        if (chain.defer()) android.util.Log.i("nori", "chain swap deferred to the next track")
    }

    /** The planner reads the transition settings itself (crates/core/src/automix/planner.rs); only the output's say is handed over. */
    private fun setupTransitions() {
        // The Rust player hands the planner the output's say itself, and asks again.
        rust?.let { return it.replan() }
        dev.nori.music.ffi.automix.transitionSetup(transitionsOff)
        transitionSink?.replan()
    }

    /**
     * DSP, crossfade, speed, offload and bit-perfect constrain each other. What may run and whether the
     * output is rebuilt for it is the core's (crates/core/src/dsp.rs audio_apply, over its own settings);
     * the sound chain follows the settings there by itself. This applies the answer to media3.
     */
    private fun applyAudio(p: Prefs) {
        nori.dac.setEnabled(p.bitPerfect)
        // The Rust player puts the same policy over its own chain (nori-engine's `apply`): it is handed the
        // settings, and takes offload, speed, silence skipping and the transitions from them.
        rust?.let { r ->
            r.applySettings()
            main.removeCallbacks(measure)
            main.postDelayed(measure, timings.measureAfterSettingsMs)
            return
        }
        val usb = nori.outputs.usb.value
        val a = dev.nori.music.ffi.settings.audioApply(
            dev.nori.music.ffi.model.OutputState(hiRes = hiRes, bitPerfect = nori.dac.state.value.bitPerfect, usb = usb, offloadRefused = offloadRefused),
            offloaded,
        )
        val policy = a.policy
        transitionsOff = policy.transitionsOff
        setupTransitions()
        // The pinned output rate stands down with everything else that touches samples.
        transitionSink?.lockRate = policy.lockRate
        exo?.skipSilenceEnabled = policy.skipSilence
        // Speed and pitch ride the player's parameters, which the sink's own stretcher hears live - no
        // rebuild, no gap; an offloaded track is the exception the rebuild rule covers.
        player.playbackParameters = androidx.media3.common.PlaybackParameters(a.speed, a.pitch)
        // Offload hands the compressed stream to the audio chip: only while nothing needs the samples, and
        // never to a USB output, which the chip cannot reach. See nori_player::policy.
        offloadWanted = policy.offload
        player.trackSelectionParameters = player.trackSelectionParameters.buildUpon().setAudioOffloadPreferences(
            AudioOffloadPreferences.Builder()
                .setAudioOffloadMode(if (policy.offload) AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_ENABLED else AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_DISABLED)
                .setIsGaplessSupportRequired(true).build()
        ).build()
        // The processor stays in the PCM chain whenever nothing is offloaded (identity when flat), so moving
        // a band is live; joining or leaving the chain is what may need the rebuild.
        equalizer.enabled = policy.processorInChain
        when (a.rebuild) {
            dev.nori.music.ffi.model.Rebuild.NOW -> reconfigureSink(urgent = true)
            dev.nori.music.ffi.model.Rebuild.AT_BOUNDARY -> reconfigureSink()
            dev.nori.music.ffi.model.Rebuild.NONE -> {}
        }
        // The song playing was planned under the old settings. Turning a crossfade on and waiting for
        // the song to end is how anyone tries this out, and without asking again that first ending was
        // always the one that did nothing.
        transitionSink?.replan()
        // And AutoMix has nothing to plan from until the tracks coming up have been measured, which was
        // only ever started by the queue moving: switched on in the middle of a song, the first mix it
        // could have made was two boundaries away.
        main.removeCallbacks(measure)
        main.postDelayed(measure, timings.measureAfterSettingsMs)
    }

    @Volatile private var transitionsOff = false

    /**
     * The window the transition planner sees. A plan is asked for once, on the first buffer of a track,
     * and the sink keeps the answer - so whenever this window changes the plan has to be asked for
     * again. Without that the planner's answer was whatever the queue happened to be in the instant the
     * track's first buffer was decoded: the audio thread runs ahead of the main one, so the first track
     * of a fresh queue was regularly planned against a window holding nothing but itself, and then
     * never planned again. It played to its end and stopped dead. The same went for a song queued, the
     * queue reordered or shuffle turned on after the track had started. The window is the core's, from
     * its own queue; it says when it changed.
     */
    private fun refreshUpcoming() {
        // The Rust player follows the core's window itself whenever it is told the queue changed.
        if (rust != null) return
        if (dev.nori.music.ffi.queue.playlistWindow()) transitionSink?.replan()
    }

    /** ReplayGain as plain volume: costs nothing and survives offload. Attenuation only. */
    private fun applyGain() {
        // The Rust player sets each song's level on its first sample; it only needs telling the settings changed.
        rust?.let { return it.gainChanged() }
        // The level is the core's decision (nori_player::policy::replay_gain over its queue and settings).
        targetVolume = dev.nori.music.ffi.queue.playlistGain(nori.dac.state.value.bitPerfect)
        if (fade == null) player.volume = targetVolume
    }

    /**
     * Ramps the volume from where it is to [to] x target over [ms], then runs [then]. Ticks only while it
     * lasts; each tick's volume, and when it is over, are nori_player::transport::fade_step's.
     */
    private fun ramp(to: Float, ms: Int, then: () -> Unit = {}) {
        fade?.let(main::removeCallbacks)
        val from = player.volume
        val start = SystemClock.uptimeMillis()
        val first = Stages.fadeStep(from, to * targetVolume, start, start, ms)
        if (Stages.fadeDone(first)) { fade = null; player.volume = Stages.fadeVolume(first); then(); return }
        fade = object : Runnable {
            override fun run() {
                val step = Stages.fadeStep(from, to * targetVolume, start, SystemClock.uptimeMillis(), ms)
                player.volume = Stages.fadeVolume(step)
                if (!Stages.fadeDone(step)) main.postDelayed(this, timings.fadeTickMs) else { fade = null; then() }
            }
        }.also(main::post)
    }

    /** What the session (notification, headset, our UI, Android Auto) actually controls: the player plus the configured manners. */
    private inner class Controls(p: Player) : androidx.media3.common.ForwardingPlayer(p) {
        /** The Rust player runs the fades itself, at its track's volume, from the same settings. */
        private val ms get() = if (rust != null) 0 else nori.settings.value.fadeMs

        // How each control sounds is decided in nori-player (nori_player::transport); this runs the fades.
        override fun play() {
            takePending()
            // Let go after a long pause (see idleRelease): opened again here.
            if (wrappedPlayer.playbackState == Player.STATE_IDLE && wrappedPlayer.mediaItemCount > 0) wrappedPlayer.prepare()
            val fadeIn = dev.nori.music.ffi.settings.playFade(ms, wrappedPlayer.isPlaying)
            if (fadeIn != null) wrappedPlayer.volume = 0f
            super.play()
            if (fadeIn != null) ramp(1f, fadeIn)
        }

        override fun pause() {
            takePending()
            val fadeOut = dev.nori.music.ffi.settings.pauseFade(ms, wrappedPlayer.isPlaying)
            if (fadeOut != null) ramp(0f, fadeOut) { super.pause(); wrappedPlayer.volume = targetVolume } else super.pause()
        }

        /**
         * A switch waiting out its dip: the old sound has to fall before the flush, so the action runs a
         * heartbeat after the finger. Whether it may still run when due is nori_player::transport::
         * SwitchQueue's (anything else moving on first, a track ending inside the dip, drops it instead
         * of yanking back); only the action itself is kept here.
         */
        private var softPending: (() -> Unit)? = null
        private val switches = dev.nori.music.ffi.queue.SwitchState()

        /**
         * Complete a waiting switch now: a second switch chains behind instead of cancelling the
         * first, so the queue steps once per tap, and a pause never swallows the seek it interrupts.
         */
        private fun takePending() {
            val action = softPending ?: return
            softPending = null
            if (switches.take(player.currentMediaItem?.mediaId, player.currentMediaItemIndex)) action()
        }

        /** Down first, then the switch, then back up (the dip is nori_player::transport::switch_dip's). */
        private fun softly(switch: Switch, action: () -> Unit) {
            if (rust != null) return action()
            val dip = dev.nori.music.ffi.settings.switchDip(ms, switch, wrappedPlayer.isPlaying) ?: run { action(); return }
            takePending()
            softPending = action
            switches.wait(player.currentMediaItem?.mediaId, player.currentMediaItemIndex)
            ramp(0f, dip.downMs) { takePending(); ramp(1f, dip.upMs) }
        }

        /**
         * A skip the user asked for while the music is paused starts it (nori_player::transport::
         * skip_plays). Only the buttons go through here - the service's own skips (an explicit track, a
         * track that will not play) call the player underneath, so a queue that was paused stays paused
         * while it steps over them.
         */
        private fun andPlay(action: () -> Unit) {
            action()
            if (dev.nori.music.ffi.queue.skipPlays(playWhenReady)) play()
        }

        // The queue is the core's (crates/core/src/playlist.rs): each change is made there first, and
        // the player is told the same change and, while shuffling, the play order the core chose.
        override fun addMediaItem(mediaItem: MediaItem) = addMediaItems(Int.MAX_VALUE, listOf(mediaItem))
        override fun addMediaItem(index: Int, mediaItem: MediaItem) = addMediaItems(index, listOf(mediaItem))
        override fun addMediaItems(mediaItems: List<MediaItem>) = addMediaItems(Int.MAX_VALUE, mediaItems)
        override fun addMediaItems(index: Int, mediaItems: List<MediaItem>) {
            if (mediaItems.isEmpty()) return
            // Where they go (after the playing song when added by hand, else where the controller asked)
            // is the core's; each item says how it came.
            val at = index.coerceIn(0, wrappedPlayer.mediaItemCount).toUInt()
            val c = dev.nori.music.ffi.queue.playlistTake(at, ids(mediaItems), mediaItems.map { it.queuedAs() ?: Hand.NO })
            edit(c) { super.addMediaItems(c.at, mediaItems) }
        }

        // A fresh evening: the parked online queue from a bridge is not part of this request.
        override fun setMediaItem(mediaItem: MediaItem) = setMediaItems(listOf(mediaItem))
        override fun setMediaItem(mediaItem: MediaItem, resetPosition: Boolean) = setMediaItems(listOf(mediaItem), resetPosition)
        override fun setMediaItem(mediaItem: MediaItem, startPositionMs: Long) = setMediaItems(listOf(mediaItem), 0, startPositionMs)
        override fun setMediaItems(mediaItems: List<MediaItem>) = setMediaItems(mediaItems, C.INDEX_UNSET, C.TIME_UNSET)
        override fun setMediaItems(mediaItems: List<MediaItem>, resetPosition: Boolean) =
            if (resetPosition) setMediaItems(mediaItems, C.INDEX_UNSET, C.TIME_UNSET) else setMediaItems(mediaItems, wrappedPlayer.currentMediaItemIndex, wrappedPlayer.currentPosition)
        override fun setMediaItems(mediaItems: List<MediaItem>, startIndex: Int, startPositionMs: Long) {
            offlineBridge?.abandon()
            // A list already in the order it plays (a weighted shuffle) goes in as it is, shown as
            // shuffled; the player's own shuffle, which would undo that order, goes off.
            val ordered = mediaItems.firstOrNull()?.inOrder() == true
            val c = if (ordered) dev.nori.music.ffi.queue.playlistSetOrdered(ids(mediaItems))
            else dev.nori.music.ffi.queue.playlistSet(ids(mediaItems), startIndex.coerceAtMost(mediaItems.size - 1), wrappedPlayer.shuffleModeEnabled)
            edit(c) {
                if (ordered && wrappedPlayer.shuffleModeEnabled) super.setShuffleModeEnabled(false)
                super.setMediaItems(mediaItems, c.at.coerceAtLeast(0), if (startIndex == C.INDEX_UNSET) C.TIME_UNSET else startPositionMs)
            }
        }
        override fun clearMediaItems() {
            offlineBridge?.abandon()
            edit(dev.nori.music.ffi.queue.playlistSet(emptyList(), -1, false)) { super.clearMediaItems() }
        }
        override fun removeMediaItem(index: Int) = removeMediaItems(index, index + 1)
        override fun removeMediaItems(fromIndex: Int, toIndex: Int) {
            // The player drops removed songs from its play order and keeps the rest as it was, as the core does.
            edit(dev.nori.music.ffi.queue.playlistRemove(fromIndex.toUInt(), toIndex.toUInt())) { super.removeMediaItems(fromIndex, toIndex) }
        }
        override fun moveMediaItem(currentIndex: Int, newIndex: Int) = moveMediaItems(currentIndex, currentIndex + 1, newIndex)
        override fun moveMediaItems(fromIndex: Int, toIndex: Int, newIndex: Int) {
            edit(dev.nori.music.ffi.queue.playlistMove(fromIndex.toUInt(), toIndex.toUInt(), newIndex.toUInt())) { super.moveMediaItems(fromIndex, toIndex, newIndex) }
        }
        /** Shuffle on: the playing song first, the songs added by hand after it, the rest shuffled. */
        override fun setShuffleModeEnabled(shuffleModeEnabled: Boolean) {
            val c = dev.nori.music.ffi.queue.playlistShuffle(shuffleModeEnabled)
            edit(c) { order(c); super.setShuffleModeEnabled(shuffleModeEnabled) }
        }
        override fun setRepeatMode(repeatMode: Int) {
            dev.nori.music.ffi.queue.playlistRepeat(repeatMode.toUByte())
            super.setRepeatMode(repeatMode)
        }

        override fun seekTo(positionMs: Long) = softly(Switch.SEEK) { super.seekTo(positionMs) }
        override fun seekTo(mediaItemIndex: Int, positionMs: Long) = softly(Switch.TO_SONG) { super.seekTo(mediaItemIndex, positionMs) }
        override fun seekToNext() = andPlay { softly(Switch.SKIP) { super.seekToNext() } }
        override fun seekToNextMediaItem() = andPlay { softly(Switch.SKIP) { super.seekToNextMediaItem() } }
        override fun seekToPreviousMediaItem() = andPlay { softly(Switch.SKIP) { super.seekToPreviousMediaItem() } }
        // Well into a song this goes back to 0:00 rather than to the song before (media3's own rule,
        // three seconds, unless the user has previous always skip: nori_player::queue::previous_restarts),
        // which paused means: start this one again, from the top, playing.
        override fun seekToPrevious() = andPlay {
            softly(Switch.SKIP) {
                if (!dev.nori.music.ffi.queue.queuePreviousRestarts(currentPosition, hasPreviousMediaItem()) && hasPreviousMediaItem()) super.seekToPreviousMediaItem()
                else super.seekToPrevious()
            }
        }

        /** Stopping drops a switch still waiting out its dip; starting over is not continuing it. */
        override fun stop() { softPending = null; switches.dropWaiting(); super.stop() }
    }

    private fun ids(items: List<MediaItem>): List<String> = items.map { it.mediaId }

    /** True while [Controls] makes an edit the core already made, so the player's list is checked once, after. */
    private var editing = false

    /** The player's side of an edit the core made: [change], then the core's play order, then one check. */
    private inline fun edit(c: dev.nori.music.ffi.queue.QueueChange, change: () -> Unit) {
        editing = true
        try { change(); order(c) } finally { editing = false }
        follow()
    }

    /**
     * A change the core made to its queue on its own (the offline bridge), made to the player the same
     * way: ranges out, songs in, then the jump, and playing.
     */
    private fun applyEdit(e: dev.nori.music.ffi.queue.QueueEdit) {
        editing = true
        try {
            for (k in e.remove.indices step 2) player.removeMediaItems(e.remove[k].toInt(), e.remove[k + 1].toInt())
            if (e.songs.isNotEmpty()) player.addMediaItems(e.at.toInt(), held(e.songs))
            order(dev.nori.music.ffi.queue.QueueChange(e.seek, e.shuffled))
            if (e.seek >= 0) player.seekTo(e.seek, C.TIME_UNSET)
        } finally { editing = false }
        follow()
        if (e.seek >= 0) { player.prepare(); player.play() }
    }

    /** The core's play order, while shuffling, written into the player: copied by the core straight into the array the order is made from. */
    private fun order(c: dev.nori.music.ffi.queue.QueueChange) {
        // The Rust player's timeline reads the core's order itself.
        val exo = exo ?: return
        if (!c.shuffled) return
        val order = IntArray(exo.mediaItemCount)
        if (PlaylistJni.order(order) != order.size) return
        exo.setShuffleOrder(DefaultShuffleOrder(order, SystemClock.elapsedRealtime()))
    }

    /**
     * The player's list after an edit, checked against the core's. Every edit is meant to go through
     * [Controls]; one that did not (the offline bridge edits the player directly) is taken as it is.
     * The check crosses as hashes (the ids' own, which the platform keeps with each string); only a list
     * that differs is sent whole.
     */
    private fun follow() {
        // The Rust player's list is only ever changed through the core, so there is nothing to catch.
        if (rust != null) return
        val t = player.currentTimeline
        val count = player.mediaItemCount
        val shuffling = player.shuffleModeEnabled
        var ids = 1
        for (k in 0 until count) ids = 31 * ids + player.getMediaItemAt(k).mediaId.hashCode()
        var order = 1
        if (shuffling) {
            var i = t.getFirstWindowIndex(true)
            while (i != C.INDEX_UNSET) { order = 31 * order + i; i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, true) }
        }
        if (PlaylistJni.same(count, player.currentMediaItemIndex, shuffling, ids, order)) return
        val walk = ArrayList<UInt>(t.windowCount)
        if (shuffling) {
            var i = t.getFirstWindowIndex(true)
            while (i != C.INDEX_UNSET) { walk += i.toUInt(); i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, true) }
        }
        dev.nori.music.ffi.queue.playlistFollow(List(count) { player.getMediaItemAt(it).mediaId }, player.currentMediaItemIndex, shuffling, walk)
    }

    private fun precacheAhead() {
        // Which songs coming up are fetched early (how many for this network, whether a mix needs the
        // next one early, none the downloads have), and from where, is the core's in one call.
        val songs = nori.sources.precacheTargets()
        if (songs.isEmpty()) return precacher.cancel()
        precacher.update(songs)
    }

    /**
     * Measures the track playing and the two after it, unless they have been measured before. AutoMix
     * plans a transition from both halves' analyses, and until this existed the only way to get one was
     * to have played the track through: the first time two songs met they were faded rather than mixed,
     * and the plan for the boundary the listener was already in the middle of arrived too late to use.
     * Off entirely when AutoMix is (the core then names no songs), and it never fetches anything (see
     * AutoMixPrefetch).
     */
    private fun analyseAhead() = analyser.update(dev.nori.music.ffi.queue.queueMeasure())

    /**
     * Keeps the music going past the end of the queue. When to fetch (the last song, or one song left,
     * one fetch at a time; never for a radio stream or a repeating queue) is the core's
     * (crates/core/src/autofill.rs over nori_player::queue::Refill), and so is what comes - the user's
     * choice twice over, and every route reads the library, so this never makes octo-fiesta download a
     * provider track.
     */
    private fun autoFill() {
        if (dev.nori.music.ffi.queue.autofillStart()) fetchFill()
    }

    private fun fetchFill() = scope.launch {
        val fresh = runCatching { nori.library.autofill() }.getOrDefault(emptyList())
        // Player work stays on this scope's main dispatcher.
        if (dev.nori.music.ffi.queue.autofillArrived(fresh.size.toUInt())) {
            controls.addMediaItems(held(fresh))
            refreshUpcoming()
        }
        // A next pressed at the end while these were on the way is taken now, if the user is still there.
        if (dev.nori.music.ffi.queue.autofillLanded()) player.seekToNextMediaItem()
    }

    /** Next with nothing after: fetch, and take the skip when the songs land (the core remembers the press). */
    private fun fillThenNext() {
        when (dev.nori.music.ffi.queue.autofillNext()) {
            FillNext.SKIP -> player.seekToNextMediaItem()
            FillNext.FETCH -> fetchFill()
            FillNext.WAIT -> {}
        }
    }

    // ---- the queue outlives the process ----

    private fun scheduleSave() {
        main.removeCallbacks(saveQueue)
        main.postDelayed(saveQueue, timings.saveAfterMs)
    }

    private fun persistQueue(push: Boolean) {
        main.removeCallbacks(saveQueue)
        // The queue is the core's (crates/core/src/playlist.rs); only the place in the song is the player's.
        val current = player.currentMediaItem?.mediaId
        val position = player.currentPosition.coerceAtLeast(0)
        scope.launch(Dispatchers.IO) {
            runCatching { nori.core.playlistSave(position.toULong()) }
            // What the server is handed (only with scrobbling on, radio left out) is the core's too, read there.
            if (push) runCatching { nori.library.pushQueue(current, position) }
        }
    }

    private fun restoreQueue() = scope.launch {
        val q = withContext(Dispatchers.IO) { runCatching { nori.core.loadQueue() }.getOrNull() } ?: return@launch
        if (q.songs.isEmpty() || player.mediaItemCount > 0) return@launch
        // Not prepared: nothing touches the network until the user presses play. The core keeps the index
        // inside the queue it hands back.
        controls.setMediaItems(held(q.songs), q.index.toInt(), q.positionMs.toLong())
    }

    /** Songs as the player's items, handed to the core in one call (see MediaItems.toMediaItems). */
    private fun items(songs: List<Song>): List<MediaItem> = songs.toMediaItems { nori.library.coverUrl(it.coverArt, NOTIFICATION_ART) }
    private fun item(s: Song): MediaItem = items(listOf(s)).first()
    /** Songs the core made for the queue and already keeps (MediaItems.heldMediaItems): nothing handed back. */
    private fun held(songs: List<Song>): List<MediaItem> = songs.heldMediaItems { nori.library.coverUrl(it.coverArt, NOTIFICATION_ART) }

    // ---- session: custom commands, Android Auto browsing, voice search ----

    private inner class Callback : MediaLibrarySession.Callback {
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
            val commands = MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS.buildUpon().add(SessionCommand(CMD_SLEEP, Bundle.EMPTY)).add(SessionCommand(CMD_TUNING, Bundle.EMPTY))
                .add(SessionCommand(CMD_FAVOURITE, Bundle.EMPTY)).add(SessionCommand(CMD_SHUFFLE, Bundle.EMPTY))
                .add(SessionCommand(CMD_FILL_NEXT, Bundle.EMPTY)).build()
            return MediaSession.ConnectionResult.AcceptedResultBuilder(session).setAvailableSessionCommands(commands).build()
        }

        override fun onCustomCommand(session: MediaSession, controller: MediaSession.ControllerInfo, command: SessionCommand, args: Bundle): ListenableFuture<SessionResult> {
            if (command.customAction == CMD_SLEEP) {
                val alarms = getSystemService(AlarmManager::class.java)
                alarms.cancel(sleepAlarm)
                // The songs still to go are counted in the core (rules.rs sleep_set, sleep_song_changed).
                pauseAtEnd(dev.nori.music.ffi.queue.sleepSet(args.getInt(ARG_SONGS).coerceAtLeast(0).toUInt(), args.getBoolean(ARG_END_OF_TRACK)))
                val minutes = args.getInt(ARG_MINUTES)
                // An alarm, not a Handler: with offloaded playback the CPU sleeps and uptime stops counting.
                if (minutes > 0) dev.nori.music.ffi.queue.sleepDelay(minutes.toUInt()).let { (delay, slack) ->
                    alarms.setWindow(AlarmManager.ELAPSED_REALTIME_WAKEUP, SystemClock.elapsedRealtime() + delay, slack, "nori.sleep", sleepAlarm, main)
                }
            }
            if (command.customAction == CMD_FAVOURITE) {
                // Only while the heart shows (a song of the library is playing; the core's call).
                val item = player.currentMediaItem?.takeIf { buttonsShown?.heart != null }
                if (item != null) {
                    val on = !currentStarred(item)
                    // The same path as the app's heart: the mark goes up at once (and redraws both hearts),
                    // the request runs on an IO thread inside Library, and a failure puts the mark back.
                    scope.launch { runCatching { nori.library.star(StarKind.SONG, item.mediaId, on) }.onFailure { android.util.Log.w("nori", "star from the notification failed: $it") } }
                }
            }
            if (command.customAction == CMD_SHUFFLE) controls.shuffleModeEnabled = !player.shuffleModeEnabled
            if (command.customAction == CMD_FILL_NEXT) fillThenNext()
            // The Rust player keeps the same rule in its own pipeline (nori_player::transport::Chain::tuning).
            if (command.customAction == CMD_TUNING) rust?.setTuning(args.getBoolean(ARG_ON))
            if (command.customAction == CMD_TUNING && exo != null) {
                val on = args.getBoolean(ARG_ON)
                // Shallow buffer makes a band move audible within ~0.5 s instead of up to the deep
                // 10 s AudioTrack fill. Rebuilding mid-track is a stop/prepare gap, so while music
                // plays the swap waits for the next boundary (or the next pause); bursts turn
                // off immediately so the track stops being topped up in multi-second bursts.
                val rebuild = chain.tuning(on, equalizer.enabled, player.playbackState == Player.STATE_IDLE, player.playWhenReady)
                updateBurst()
                if (rebuild) reconfigureSink(urgent = true)
            }
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
        }

        override fun onAddMediaItems(session: MediaSession, controller: MediaSession.ControllerInfo, items: MutableList<MediaItem>): ListenableFuture<MutableList<MediaItem>> {
            val query = items.singleOrNull()?.requestMetadata?.searchQuery
            if (query != null) return scope.future { items(nori.library.search(query).songs).toMutableList() }
            // Our own UI sends complete items. Android Auto sends bare ids of things it was shown earlier.
            if (items.all { it.mediaMetadata.title != null }) return Futures.immediateFuture(items.map { it.playable() }.toMutableList())
            return scope.future {
                items.mapNotNull { i -> served.get(i.mediaId) ?: withContext(Dispatchers.IO) { runCatching { nori.library.song(i.mediaId) }.getOrNull() }?.let(::item) }.toMutableList()
            }
        }

        override fun onPlaybackResumption(session: MediaSession, controller: MediaSession.ControllerInfo): ListenableFuture<MediaSession.MediaItemsWithStartPosition> = scope.future {
            val q = withContext(Dispatchers.IO) { nori.core.loadQueue() }
            MediaSession.MediaItemsWithStartPosition(held(q.songs), q.index.toInt(), q.positionMs.toLong())
        }

        override fun onGetLibraryRoot(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, params: LibraryParams?) =
            Futures.immediateFuture(LibraryResult.ofItem(folder(nori.client.browseRoot()), params))

        override fun onGetChildren(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, parentId: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> =
            scope.future {
                val children = runCatching { children(parentId) }.getOrDefault(emptyList())
                children.forEach { if (it.mediaMetadata.isPlayable == true) served.put(it.mediaId, it) }
                LibraryResult.ofItemList(children.drop(page * pageSize).take(pageSize), params)
            }

        override fun onSearch(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, query: String, params: LibraryParams?): ListenableFuture<LibraryResult<Void>> {
            scope.launch {
                val n = runCatching { nori.library.search(query).songs.size }.getOrDefault(0)
                session.notifySearchResultChanged(browser, query, n, params)
            }
            return Futures.immediateFuture(LibraryResult.ofVoid())
        }

        override fun onGetSearchResult(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, query: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> =
            scope.future {
                val songs = runCatching { items(nori.library.search(query).songs) }.getOrDefault(emptyList())
                songs.forEach { served.put(it.mediaId, it) }
                LibraryResult.ofItemList(songs.drop(page * pageSize).take(pageSize), params)
            }
    }

    private fun folder(f: dev.nori.music.ffi.library.BrowseFolder): MediaItem = MediaItem.Builder().setMediaId(f.id).setMediaMetadata(
        MediaMetadata.Builder().setTitle(f.title).setArtist(f.subtitle).setArtworkUri(f.art?.let(android.net.Uri::parse))
            .setIsBrowsable(true).setIsPlayable(false).setMediaType(MediaMetadata.MEDIA_TYPE_FOLDER_MIXED).build()
    ).build()

    /** What a folder of the car's tree holds is the core's (crates/core/src/car.rs); this makes the items. */
    private suspend fun children(parent: String): List<MediaItem> {
        val page = nori.client.browseChildren(parent)
        return page.folders.map(::folder) + items(page.songs)
    }
}

/**
 * An AudioTrack a player opened, with what was asked of it: [askedBytes] of buffer and the performance
 * mode [askedMode]. The perf build reads what the platform made of it against that; nothing else does.
 */
class OpenedTrack(val track: AudioTrack, val engine: String, val askedBytes: Int, val askedMode: Int)
