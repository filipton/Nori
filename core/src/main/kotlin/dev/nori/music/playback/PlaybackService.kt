package dev.nori.music.playback

import dev.nori.music.ffi.Switch
import android.app.AlarmManager
import android.app.PendingIntent
import android.media.AudioFormat
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
import dev.nori.music.data.AlbumSort
import dev.nori.music.data.StarKind
import dev.nori.music.ffi.PlayQueue
import dev.nori.music.ffi.Song
import dev.nori.music.settings.AutoFillBasis
import dev.nori.music.settings.AutoFillKind
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
    }

    private lateinit var nori: Nori
    private lateinit var player: ExoPlayer
    private lateinit var session: MediaLibrarySession
    private lateinit var scrobbler: Scrobbler
    private val equalizer = Equalizer()
    private var hiRes = false
    /** Kept so a freshly measured track can have its transition planned again; see AutoMixPrefetch. */
    private var transitionSink: TransitionSink? = null
    @Suppress("DEPRECATION")
    private val wifiLock by lazy { applicationContext.getSystemService(WifiManager::class.java).createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "nori:loading").apply { setReferenceCounted(false) } }
    private var offloaded = false
    /** The speed and pitch the sound is being made with; see applyAudio. */
    private var appliedSpeed = 1f
    private var appliedPitch = 1f
    /**
     * The sink failed to open or to write once. Offload is the only part of the chain that can fail on a
     * device the app cannot see into (a USB DAC, a dock, a car head unit), so it is given up for the life of
     * the service rather than retried into a loop of silent tracks.
     */
    private var offloadRefused = false
    /** Items from the current one onwards, as the playback thread may ask about them (decoding runs ahead). */
    @Volatile private var upcoming: List<MediaItem> = emptyList()
    @Volatile private var previous: MediaItem? = null
    private lateinit var precacher: Precacher
    private lateinit var analyser: AutoMixPrefetch
    private val precache = Runnable { precacheAhead(); analyseAhead() }
    private val measure = Runnable { analyseAhead() }
    /** What the volume should be once no fade is running: 1, or the ReplayGain attenuation. */
    private var targetVolume = 1f
    private var fade: Runnable? = null
    private var errorsInARow = 0
    /** Sleep timer "after N songs": transitions still to go. */
    private var sleepAfterSongs = 0
    private var offlineBridge: OfflineBridge? = null
    /**
     * Autofill is on the wire for the end of the queue. A next press that found nothing to skip to sets
     * [pendingNext] so the skip happens the moment the songs land - without it the press is a wall
     * until the user hits next again (or previous then next).
     */
    private var autoFillInFlight = false
    private var pendingNext = false
    /** Song id that asked for the pending next; ignored if the user has moved on (previous, jump). */
    private var pendingNextFrom: String? = null
    /**
     * When the output is rebuilt: for the equalizer screen's shallow buffer and back, and for settings
     * that need a new chain - at the next boundary or pause while music plays, at once otherwise. The
     * rules are nori_player::transport::Chain's.
     */
    private val chain = dev.nori.music.ffi.ChainState()
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
        track
    }

    private val shallowBuffer = DefaultAudioTrackBufferSizeProvider.Builder().build()
    private val deepBuffer = DefaultAudioTrackBufferSizeProvider.Builder().setTargetPcmBufferDurationUs(dev.nori.music.ffi.burstBufferUs().toInt()).setMaxPcmBufferDurationUs(dev.nori.music.ffi.burstBufferUs().toInt()).build()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val main = Handler(Looper.getMainLooper())
    private val served = LruCache<String, MediaItem>(500)
    private val saveQueue = Runnable { persistQueue(push = false) }
    private val sleepAlarm = AlarmManager.OnAlarmListener { player.pause() }

    override fun onCreate() {
        super.onCreate()
        Equalizer.active = equalizer
        nori = Nori.get(this)
        scrobbler = Scrobbler(nori, scope)

        val renderers = object : DefaultRenderersFactory(this) {
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
        hiRes = nori.settings.value.hiRes
        renderers.setEnableAudioFloatOutput(hiRes)
        player = ExoPlayer.Builder(this, renderers)
            .setMediaSourceFactory(DefaultMediaSourceFactory(this, DefaultExtractorsFactory().setConstantBitrateSeekingEnabled(true)).setDataSourceFactory(nori.sources.factory))
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
                dev.nori.music.ffi.loadControl(getSystemService(android.app.ActivityManager::class.java).memoryClass.toUInt()).let { c ->
                    DefaultLoadControl.Builder().setBufferDurationsMs(c[0].toInt(), c[1].toInt(), c[2].toInt(), c[3].toInt())
                        .setTargetBufferBytes(c[4].toInt()).setPrioritizeTimeOverSizeThresholds(false).build()
                }
            )
            .build()
        precacher = Precacher(nori.sources)
        analyser = AutoMixPrefetch(nori.sources, { nori.core }) { transitionSink?.replan() }
        offlineBridge = OfflineBridge(
            this, player, nori.downloads, nori.sources, main,
            cover = { nori.library.coverUrl(it.coverArt, NOTIFICATION_ART) },
            onChanged = { /* PlayerConnection picks bridging up from media extras on the next publish. */ },
        )
        player.addListener(listener)
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
            var last = nori.settings.value
            nori.settings.prefs.collect { p ->
                if (p.copy(replayGain = last.replayGain, preampDb = last.preampDb, untaggedGainDb = last.untaggedGainDb, scrobblePercent = last.scrobblePercent, listPrefs = last.listPrefs, homeRows = last.homeRows, pinnedPlaylists = last.pinnedPlaylists, autoEqAuto = last.autoEqAuto) != last) applyAudio(p)
                if (p.replayGain != last.replayGain || p.preampDb != last.preampDb || p.untaggedGainDb != last.untaggedGainDb) applyGain()
                // AutoMix's loudness matching stands down under ReplayGain, so the planner hears of it too.
                if (p.replayGain != last.replayGain) setupTransitions(p)
                last = p
            }
        }

        val open = packageManager.getLaunchIntentForPackage(packageName)?.let { PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE) }
        session = MediaLibrarySession.Builder(this, Controls(player), Callback())
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

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo) = session

    override fun onTaskRemoved(rootIntent: android.content.Intent?) {
        if (!player.playWhenReady || player.mediaItemCount == 0) stopSelf()
    }

    override fun onDestroy() {
        Equalizer.active = null
        persistQueue(push = false)
        getSystemService(AlarmManager::class.java).cancel(sleepAlarm)
        main.removeCallbacks(precache)
        main.removeCallbacks(measure)
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
            // A settings change that needed a new chain waited for a boundary instead of cutting the
            // track: this is it. Skipped on a repeat-one loop, which should restart seamlessly; the
            // swap then waits for a real boundary. A planned crossfade into this track dies with the
            // rebuild - the setting wins over one mix.
            if (chain.boundary(reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT && player.repeatMode == Player.REPEAT_MODE_ONE)) {
                android.util.Log.i("nori", "chain swap at the boundary")
                if (player.playbackState != Player.STATE_IDLE) { player.stop(); player.prepare() }
            }
            refreshButtons()
            if (item != null && nori.settings.value.skipExplicit && dev.nori.music.ffi.queueFlags(item.mediaId) and 1u != 0u && player.hasNextMediaItem()) return player.seekToNextMediaItem()
            if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT && item != null) return scrobbler.onTrack(item.mediaId, player.isPlaying)
            scrobbler.onTrack(item?.takeUnless { it.isRadio }?.mediaId, player.isPlaying)
            applyGain()
            scheduleSave()
            autoFill(item)
            if (nori.settings.value.bridgeOffline) offlineBridge?.onTrack(item)
            announce()
            errorsInARow = 0
            refreshUpcoming()
            // A few seconds in, the current track has been fetched and the radio is still up: fetch ahead now.
            main.removeCallbacks(precache)
            main.postDelayed(precache, 6_000)
            if (sleepAfterSongs > 0) dev.nori.music.ffi.sleepSongChanged(sleepAfterSongs.toUInt()).let { (left, pause) ->
                sleepAfterSongs = left.toInt()
                if (pause != 0u) player.pauseAtEndOfMediaItems = true
            }
        }

        override fun onIsLoadingChanged(isLoading: Boolean) {
            if (isLoading && !wifiLock.isHeld) wifiLock.acquire() else if (!isLoading && wifiLock.isHeld) wifiLock.release()
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            // Paused is silent: the deep buffer comes back at once rather than waiting a song, and the
            // rebuild carries any other pending swap with it.
            if (!isPlaying && !player.playWhenReady && chain.paused()) reconfigureSink(urgent = true)
            announce()
            scrobbler.onPlaying(isPlaying)
            if (!isPlaying && !player.playWhenReady) persistQueue(push = true)
        }

        override fun onShuffleModeEnabledChanged(on: Boolean) { if (on) shuffleAroundCurrent(); refreshUpcoming(); refreshButtons() }
        override fun onRepeatModeChanged(mode: Int) = refreshUpcoming()

        override fun onTimelineChanged(timeline: androidx.media3.common.Timeline, reason: Int) {
            refreshUpcoming()
            if (reason == Player.TIMELINE_CHANGE_REASON_PLAYLIST_CHANGED) {
                scheduleSave()
                // The queue was edited: what comes next is not what it was, and measuring the new next
                // track is the whole point of measuring ahead at all. Only that - the fetching ahead is
                // left alone, since restarting it would throw away a track it is halfway through.
                main.removeCallbacks(measure)
                main.postDelayed(measure, 2_000)
            }
        }

        override fun onPlayerError(error: androidx.media3.common.PlaybackException) {
            // What to do is nori_player::queue::on_error's call: an output that refuses the offloaded
            // stream goes back to the CPU path, a network failure goes to the offline bridge when it is on,
            // anything else skips a few and then stops.
            val sink = generateSequence(error.cause) { it.cause }.any {
                it is AudioSink.InitializationException || it is AudioSink.WriteException || it is AudioSink.ConfigurationException
            }
            val kind: UByte = when { sink -> 0u; error.isNetworkish() -> 1u; else -> 2u }
            val p = nori.settings.value
            when (dev.nori.music.ffi.queueOnError(kind, offloadRefused, p.bridgeOffline && offlineBridge != null, p.skipOnError, player.hasNextMediaItem(), errorsInARow.toUInt()).toInt()) {
                0 -> {
                    android.util.Log.w("nori", "audio sink refused the stream, giving up offload", error)
                    offloadRefused = true
                    applyAudio(p)
                    player.prepare()
                    player.play()
                }
                1 -> if (offlineBridge?.onPlaybackError(error) == true) errorsInARow = 0 else skipAfterError(p)
                2 -> skipAfterError(p)
            }
        }

        override fun onPlaybackStateChanged(state: Int) {
            if (state == Player.STATE_ENDED) scrobbler.onTrack(null, false)
        }
    }


    private fun skipAfterError(p: Prefs) {
        if (!p.skipOnError || !player.hasNextMediaItem() || errorsInARow >= 3) return
        errorsInARow++
        player.seekToNextMediaItem()
        player.prepare()
        player.play()
    }

    /** What the heart and shuffle buttons last showed, so an unrelated change does not rebuild the notification. */
    private var buttonsShown: String? = null

    private fun currentStarred(item: MediaItem): Boolean =
        nori.library.isStarred(StarKind.SONG, item.mediaId, dev.nori.music.ffi.queueFlags(item.mediaId) and 2u != 0u)

    /**
     * Heart and shuffle beside previous / play / next, in the secondary slots the way other players put
     * them. Called when the track, the shuffle flag or a star changes - never on a timer.
     */
    private fun refreshButtons() {
        if (!::session.isInitialized) return
        val item = player.currentMediaItem?.takeUnless { it.isRadio }
        val starred = item?.let(::currentStarred)
        val shuffle = player.shuffleModeEnabled
        val key = "$starred/$shuffle"
        if (key == buttonsShown) return
        buttonsShown = key
        val buttons = ArrayList<CommandButton>(2)
        if (starred != null) buttons += CommandButton.Builder(if (starred) CommandButton.ICON_HEART_FILLED else CommandButton.ICON_HEART_UNFILLED)
            .setDisplayName(if (starred) "Remove from favourites" else "Add to favourites")
            .setSessionCommand(SessionCommand(CMD_FAVOURITE, Bundle.EMPTY))
            .setSlots(CommandButton.SLOT_BACK_SECONDARY, CommandButton.SLOT_OVERFLOW).build()
        buttons += CommandButton.Builder(if (shuffle) CommandButton.ICON_SHUFFLE_ON else CommandButton.ICON_SHUFFLE_OFF)
            .setDisplayName(if (shuffle) "Shuffle off" else "Shuffle on")
            .setSessionCommand(SessionCommand(CMD_SHUFFLE, Bundle.EMPTY))
            .setSlots(CommandButton.SLOT_FORWARD_SECONDARY, CommandButton.SLOT_OVERFLOW).build()
        session.setMediaButtonPreferences(buttons)
    }

    private fun updateBurst() { transitionSink?.bursting = chain.bursting(offloaded) }

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
        if (player.playbackState == Player.STATE_IDLE || urgent) {
            if (player.playbackState != Player.STATE_IDLE) {
                android.util.Log.i("nori", "chain swap now")
                player.stop(); player.prepare()
            }
            return
        }
        if (chain.defer()) android.util.Log.i("nori", "chain swap deferred to the next track")
    }

    /** The transition planner's settings, which it keeps in Rust (crates/core/src/automix/planner.rs). */
    private fun setupTransitions(p: Prefs) {
        dev.nori.music.ffi.transitionSetup(
            dev.nori.music.ffi.TransitionPrefs(
                autoMix = p.autoMix, crossfadeS = p.crossfadeSec, autoMixMaxS = p.autoMixMaxS, beatMatch = p.autoMixBeatMatch,
                maxTempoChangePct = p.autoMixMaxTempoPct, bassSwap = p.autoMixBassSwap, filterEffects = p.autoMixFilters,
                echoOut = p.autoMixEchoOut, keepPitch = p.autoMixKeepPitch, keepAlbums = p.crossfadeKeepAlbums,
                replayGain = p.replayGain != ReplayGainMode.OFF,
            ),
            transitionsOff,
        )
        transitionSink?.replan()
    }

    /** DSP, crossfade, speed, offload and bit-perfect constrain each other; this is where that is decided. */
    private fun applyAudio(p: Prefs) {
        nori.dac.setEnabled(p.bitPerfect)
        // Which parts of the chain may run is decided in the player crate (nori_player::policy), the same
        // for every platform; what follows only applies it to media3.
        val usb = nori.outputs.usb.value
        val policy = dev.nori.music.ffi.audioPolicy(
            dev.nori.music.ffi.AudioPrefs(
                dsp = p.dsp, skipSilence = p.skipSilence, offload = p.offload, crossfadeS = p.crossfadeSec,
                autoMix = p.autoMix, speed = p.speed, pitch = p.pitch,
            ),
            dev.nori.music.ffi.OutputState(hiRes = hiRes, bitPerfect = nori.dac.state.value.bitPerfect, usb = usb, offloadRefused = offloadRefused),
        )
        val untouched = policy.untouched
        // Curve and output stage are always live: the Rust side picks them up on the next buffer.
        // Empty bands + flat output is an identity memcpy, so "EQ off" on a PCM path costs almost nothing.
        equalizer.setChain(if (p.eqEnabled) p.eqBands else emptyList(), p.effectivePreampDb, p.crossfeedDb)
        equalizer.setOutput(p.balance, p.mono, p.limiterThresholdDb, 120f, if (p.limiter) 5f else 0f)
        transitionsOff = policy.transitionsOff
        setupTransitions(p)
        // The pinned output rate stands down with everything else that touches samples.
        transitionSink?.lockRate = policy.lockRate
        player.skipSilenceEnabled = policy.skipSilence
        // Speed and pitch ride the player's parameters, which the sink's own stretcher hears live -
        // no rebuild, no gap. The one exception is an offloaded track: the chip plays what it was
        // given at 1x, so leaving offload still cuts (and the position is kept).
        val tempoChanged = appliedSpeed != p.speed || appliedPitch != p.pitch
        appliedSpeed = p.speed
        appliedPitch = p.pitch
        player.playbackParameters = androidx.media3.common.PlaybackParameters(p.speed, p.pitch)
        // Offload hands the compressed stream to the audio chip: only while nothing needs the samples, and
        // never to a USB output, which the chip cannot reach (Android still opens the track and reports it
        // playing, so a DAC just sits there in silence). See nori_player::policy.
        val offload = policy.offload
        player.trackSelectionParameters = player.trackSelectionParameters.buildUpon().setAudioOffloadPreferences(
            AudioOffloadPreferences.Builder()
                .setAudioOffloadMode(if (offload) AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_ENABLED else AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_DISABLED)
                .setIsGaplessSupportRequired(true).build()
        ).build()
        // Keep the processor in the PCM chain whenever we are not offloading (identity when flat).
        // Toggling EQ / bands / limiter then only touches setChain/setOutput — no sink rebuild, no gap.
        // Joining or leaving the chain still needs a rebuild; that waits for the next track unless the
        // current offloaded path is already the wrong one (USB / refused), where silence is worse.
        val wantProcessor = policy.processorInChain
        val offloadChanged = offloadWanted != offload
        offloadWanted = offload
        val processorChanged = equalizer.enabled != wantProcessor
        if (processorChanged) equalizer.enabled = wantProcessor
        // Whether that rebuilds the output now, at the next song boundary or not at all: nori_player::transport::rebuild.
        val change = dev.nori.music.ffi.ChainChange(
            offloaded = offloaded, offload = offload, offloadChanged = offloadChanged, usb = usb,
            offloadRefused = offloadRefused, tempoChanged = tempoChanged, processorChanged = processorChanged,
        )
        when (dev.nori.music.ffi.sinkRebuild(change)) {
            dev.nori.music.ffi.Rebuild.NOW -> reconfigureSink(urgent = true)
            dev.nori.music.ffi.Rebuild.AT_BOUNDARY -> reconfigureSink()
            dev.nori.music.ffi.Rebuild.NONE -> {}
        }
        // The song playing was planned under the old settings. Turning a crossfade on and waiting for
        // the song to end is how anyone tries this out, and without asking again that first ending was
        // always the one that did nothing.
        transitionSink?.replan()
        // And AutoMix has nothing to plan from until the tracks coming up have been measured, which was
        // only ever started by the queue moving: switched on in the middle of a song, the first mix it
        // could have made was two boundaries away.
        main.removeCallbacks(measure)
        main.postDelayed(measure, 1_000)
    }

    @Volatile private var transitionsOff = false

    /** Mirrors the player's shuffle flag for the audio thread, which may not ask the player itself. */
    @Volatile private var shuffling = false

    /**
     * The window the transition planner sees. A plan is asked for once, on the first buffer of a track,
     * and the sink keeps the answer - so whenever this window changes the plan has to be asked for
     * again. Without that the planner's answer was whatever the queue happened to be in the instant the
     * track's first buffer was decoded: the audio thread runs ahead of the main one, so the first track
     * of a fresh queue was regularly planned against a window holding nothing but itself, and then
     * never planned again. It played to its end and stopped dead. The same went for a song queued, the
     * queue reordered or shuffle turned on after the track had started.
     */
    private fun refreshUpcoming() {
        val wasShuffling = shuffling
        shuffling = player.shuffleModeEnabled
        val t = player.currentTimeline
        val was = upcoming
        if (t.isEmpty || player.currentMediaItemIndex == C.INDEX_UNSET) { upcoming = emptyList(); return }
        val list = ArrayList<MediaItem>(8)
        // The song before this one is kept aside for the planner (see planFor), not put in the window:
        // everything else reads the window's first entry as the song playing.
        val before = t.getPreviousWindowIndex(player.currentMediaItemIndex, player.repeatMode, player.shuffleModeEnabled)
        previous = if (before != C.INDEX_UNSET) player.getMediaItemAt(before) else null
        var i = player.currentMediaItemIndex
        while (i != C.INDEX_UNSET && list.size < 8) {
            list += player.getMediaItemAt(i)
            i = t.getNextWindowIndex(i, player.repeatMode, player.shuffleModeEnabled)
        }
        upcoming = list
        if (was.size != list.size || was.indices.any { was[it].mediaId != list[it].mediaId } || shuffling != wasShuffling) {
            // The planner reads its window in Rust (crates/core/src/automix/planner.rs); it is handed over
            // only when it changes.
            dev.nori.music.ffi.queueWindow((listOfNotNull(previous) + list).map { it.mediaId }, shuffling)
            transitionSink?.replan()
        }
    }

    private fun nextItem(): MediaItem? = player.nextMediaItemIndex.takeIf { it != C.INDEX_UNSET }?.let(player::getMediaItemAt)
    private fun previousItem(): MediaItem? = player.previousMediaItemIndex.takeIf { it != C.INDEX_UNSET }?.let(player::getMediaItemAt)

    /** ReplayGain as plain volume: costs nothing and survives offload. Attenuation only. */
    private fun applyGain() {
        val p = nori.settings.value
        // The level is the core's decision (nori_player::policy::replay_gain over the queued songs it
        // keeps, crates/core/src/queue.rs); this names the songs around the one playing.
        targetVolume = dev.nori.music.ffi.queueGain(
            before = previousItem()?.mediaId, current = player.currentMediaItem?.mediaId, after = nextItem()?.mediaId,
            mode = when (p.replayGain) {
                ReplayGainMode.OFF -> dev.nori.music.ffi.GainMode.OFF
                ReplayGainMode.TRACK -> dev.nori.music.ffi.GainMode.TRACK
                ReplayGainMode.ALBUM -> dev.nori.music.ffi.GainMode.ALBUM
                ReplayGainMode.AUTO -> dev.nori.music.ffi.GainMode.AUTO
            },
            preampDb = p.preampDb, untaggedDb = p.untaggedGainDb, bitPerfect = nori.dac.state.value.bitPerfect, shuffling = shuffling,
        )
        if (fade == null) player.volume = targetVolume
    }

    /** Ramps the volume from where it is to [to] x target over [ms], then runs [then]. Ticks only while it lasts. */
    private fun ramp(to: Float, ms: Int, then: () -> Unit = {}) {
        fade?.let(main::removeCallbacks)
        if (ms <= 0) { fade = null; player.volume = to * targetVolume; then(); return }
        val from = player.volume
        val start = SystemClock.uptimeMillis()
        fade = object : Runnable {
            override fun run() {
                val t = ((SystemClock.uptimeMillis() - start) / ms.toFloat()).coerceIn(0f, 1f)
                player.volume = Dsp.fadeVolume(from, to * targetVolume, t)
                if (t < 1f) main.postDelayed(this, 16) else { fade = null; then() }
            }
        }.also(main::post)
    }

    /** What the session (notification, headset, our UI, Android Auto) actually controls: the player plus the configured manners. */
    private inner class Controls(p: ExoPlayer) : androidx.media3.common.ForwardingPlayer(p) {
        private val ms get() = nori.settings.value.fadeMs

        // How each control sounds is decided in nori-player (nori_player::transport); this runs the fades.
        override fun play() {
            takePending()
            val fadeIn = dev.nori.music.ffi.playFade(ms, wrappedPlayer.isPlaying)
            if (fadeIn != null) wrappedPlayer.volume = 0f
            super.play()
            if (fadeIn != null) ramp(1f, fadeIn)
        }

        override fun pause() {
            takePending()
            val fadeOut = dev.nori.music.ffi.pauseFade(ms, wrappedPlayer.isPlaying)
            if (fadeOut != null) ramp(0f, fadeOut) { super.pause(); wrappedPlayer.volume = targetVolume } else super.pause()
        }

        /**
         * A switch waiting out its dip: the old sound has to fall before the flush, so the action
         * runs a heartbeat after the finger. Guarded by what was current when asked: anything else
         * moving on first (a track ending inside the dip) drops it instead of yanking back.
         */
        private var softPending: (() -> Unit)? = null
        private var softItem: String? = null
        private var softIndex = C.INDEX_UNSET

        /**
         * Complete a waiting switch now: a second switch chains behind instead of cancelling the
         * first, so the queue steps once per tap, and a pause never swallows the seek it interrupts.
         */
        private fun takePending() {
            val action = softPending ?: return
            softPending = null
            if (player.currentMediaItem?.mediaId == softItem && player.currentMediaItemIndex == softIndex) action()
        }

        /** Down first, then the switch, then back up (the dip is nori_player::transport::switch_dip's). */
        private fun softly(switch: Switch, action: () -> Unit) {
            val dip = dev.nori.music.ffi.switchDip(ms, switch, wrappedPlayer.isPlaying) ?: run { action(); return }
            takePending()
            softPending = action
            softItem = player.currentMediaItem?.mediaId
            softIndex = player.currentMediaItemIndex
            ramp(0f, dip.downMs) { takePending(); ramp(1f, dip.upMs) }
        }

        /**
         * A skip asked for while the music is paused is a request for music, not for a different song
         * to sit paused on: the track changes and starts. Only the buttons go through here - the
         * service's own skips (an explicit track, a track that will not play) call the player
         * underneath, so a queue that was paused stays paused while it steps over them.
         */
        private fun andPlay(action: () -> Unit) {
            action()
            if (!playWhenReady) play()
        }

        override fun addMediaItems(mediaItems: List<MediaItem>) = addMediaItems(Int.MAX_VALUE, mediaItems)
        override fun addMediaItems(index: Int, mediaItems: List<MediaItem>) {
            if (mediaItems.isNotEmpty() && wrappedPlayer.mediaItemCount > 0 && mediaItems.all { it.queuedAs() != null }) upNext(mediaItems)
            else super.addMediaItems(index.coerceAtMost(wrappedPlayer.mediaItemCount), mediaItems)
        }

        // A fresh evening: the parked online queue from a bridge is not part of this request.
        override fun setMediaItems(mediaItems: List<MediaItem>) {
            if (!OfflineBridge.bridgeMutating) offlineBridge?.abandon()
            super.setMediaItems(mediaItems)
        }
        override fun setMediaItems(mediaItems: List<MediaItem>, resetPosition: Boolean) {
            if (!OfflineBridge.bridgeMutating) offlineBridge?.abandon()
            super.setMediaItems(mediaItems, resetPosition)
        }
        override fun setMediaItems(mediaItems: List<MediaItem>, startIndex: Int, startPositionMs: Long) {
            if (!OfflineBridge.bridgeMutating) offlineBridge?.abandon()
            super.setMediaItems(mediaItems, startIndex, startPositionMs)
        }
        override fun clearMediaItems() {
            if (!OfflineBridge.bridgeMutating) offlineBridge?.abandon()
            super.clearMediaItems()
        }

        override fun seekTo(positionMs: Long) = softly(Switch.SEEK) { super.seekTo(positionMs) }
        override fun seekTo(mediaItemIndex: Int, positionMs: Long) = softly(Switch.TO_SONG) { super.seekTo(mediaItemIndex, positionMs) }
        override fun seekToNext() = andPlay { softly(Switch.SKIP) { super.seekToNext() } }
        override fun seekToNextMediaItem() = andPlay { softly(Switch.SKIP) { super.seekToNextMediaItem() } }
        override fun seekToPreviousMediaItem() = andPlay { softly(Switch.SKIP) { super.seekToPreviousMediaItem() } }
        // Well into a song this goes back to 0:00 rather than to the song before (media3's own rule,
        // three seconds), which paused means: start this one again, from the top, playing.
        override fun seekToPrevious() = andPlay {
            softly(Switch.SKIP) { if (nori.settings.value.previousAlwaysSkips && hasPreviousMediaItem()) super.seekToPreviousMediaItem() else super.seekToPrevious() }
        }

        /** Stopping drops a switch still waiting out its dip; starting over is not continuing it. */
        override fun stop() { softPending = null; super.stop() }
    }

    /**
     * Play next and Add to queue, the way Apple does them: the songs go right after the playing one
     * ("next"), or after the songs added by hand before them ("last"), in the order given, and then the
     * queue carries on as it was. In the list itself they sit there too, so turning shuffle off keeps
     * them next. Under shuffle, ExoPlayer would drop each at a random place in the play order, so the
     * order is rebuilt with them where they belong and everything else where it was.
     */
    private fun upNext(items: List<MediaItem>) {
        // Where they go, and the play order when shuffling, is nori_player::queue::place's call.
        val n = player.mediaItemCount
        val hand = List(n) { player.getMediaItemAt(it).queuedAs() != null }
        val p = dev.nori.music.ffi.queuePlace(
            n.toUInt(), player.currentMediaItemIndex.coerceAtLeast(0).toUInt(), hand,
            if (player.shuffleModeEnabled) playOrder().map { it.toUInt() } else null,
            items.first().queuedAs() == "last", items.size.toUInt(),
        )
        player.addMediaItems(p.at.toInt(), items)
        p.order?.let { order -> player.setShuffleOrder(DefaultShuffleOrder(IntArray(order.size) { order[it].toInt() }, SystemClock.elapsedRealtime())) }
    }

    /** The shuffled play order as the player walks it. */
    private fun playOrder(): List<Int> {
        val t = player.currentTimeline
        val order = ArrayList<Int>(t.windowCount)
        var i = t.getFirstWindowIndex(true)
        while (i != C.INDEX_UNSET) { order += i; i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, true) }
        return order
    }

    /**
     * Shuffle turned on: the playing song first, the songs added by hand after it, the rest shuffled
     * (nori_player::queue::shuffle_around).
     */
    private fun shuffleAroundCurrent() {
        val n = player.mediaItemCount
        val cur = player.currentMediaItemIndex
        if (n < 2 || cur == C.INDEX_UNSET) return
        val order = dev.nori.music.ffi.queueShuffleAround(n.toUInt(), cur.toUInt(), List(n) { player.getMediaItemAt(it).queuedAs() != null }, System.nanoTime().toULong())
        player.setShuffleOrder(DefaultShuffleOrder(IntArray(order.size) { order[it].toInt() }, SystemClock.elapsedRealtime()))
    }

    private fun precacheAhead() {
        val p = nori.settings.value
        // Which songs coming up are fetched early is nori_player::queue::precache_range's call.
        val range = dev.nori.music.ffi.queuePrecache(
            (if (nori.http.metered) p.precacheMobile else p.precacheWifi).toUInt(), !transitionsOff && (p.crossfadeSec > 0 || p.autoMix), player.shuffleModeEnabled,
        )
        if (range.isEmpty()) return precacher.cancel()
        val fetching = nori.downloads.state.value.pendingIds
        precacher.update(upcoming.drop(range[0].toInt()).take((range[1] - range[0]).toInt() + 1)) { it in fetching }
    }

    /**
     * Measures the track playing and the two after it, unless they have been measured before. AutoMix
     * plans a transition from both halves' analyses, and until this existed the only way to get one was
     * to have played the track through: the first time two songs met they were faded rather than mixed,
     * and the plan for the boundary the listener was already in the middle of arrived too late to use.
     * Off entirely when AutoMix is, and it never fetches anything (see AutoMixPrefetch).
     */
    private fun analyseAhead() {
        if (!nori.settings.value.autoMix) return analyser.cancel()
        analyser.update(upcoming.take(dev.nori.music.ffi.queueMeasureAhead().toInt()).map { it.mediaId })
    }

    /**
     * Keeps the music going past the end of the queue. Starts when the last song is reached *or* when
     * only one song still follows - that one-ahead start is what stops a fast next from hitting a wall
     * while similar songs are still on the wire. What arrives is the user's choice twice over - songs
     * or a whole album ([AutoFillKind]), chosen by what the server calls similar or by the artist,
     * genre or decade ([AutoFillBasis]) - and every route here reads the library, so this never makes
     * octo-fiesta download a provider track.
     */
    private fun autoFill(item: MediaItem?) {
        val p = nori.settings.value
        if (item == null || item.isRadio || player.repeatMode != Player.REPEAT_MODE_OFF || !p.autoFill) return
        if (item.toSong().isExternal) return
        // Still plenty left: nothing to do. One or none left: fetch now so the next press has somewhere to go.
        if (songsAfter() > 1) return
        if (autoFillInFlight) return
        // Both read off the queue before anything suspends: the player belongs to this looper, and what
        // follows runs on an IO thread.
        val seed = item.toSong()
        val queued = (0 until player.mediaItemCount).mapTo(HashSet()) { player.getMediaItemAt(it).mediaId }
        val played = dev.nori.music.ffi.queueAlbums(List(player.mediaItemCount) { player.getMediaItemAt(it).mediaId }).toHashSet()
        autoFillInFlight = true
        scope.launch {
            val fresh = runCatching {
                withContext(Dispatchers.IO) {
                    if (p.autoFillKind == AutoFillKind.ALBUMS) nextAlbum(seed, p.autoFillBasis, queued, played)
                    else nextSongs(seed, p.autoFillBasis).filter { it.id !in queued && !it.isExternal }.take(15)
                }
            }.getOrDefault(emptyList())
            // Player work stays on this scope's main dispatcher.
            if (fresh.isNotEmpty() && songsAfter() <= 1) {
                player.addMediaItems(items(fresh))
                refreshUpcoming()
                val still = pendingNext && player.currentMediaItem?.mediaId == pendingNextFrom
                pendingNext = false
                pendingNextFrom = null
                if (still && player.hasNextMediaItem()) player.seekToNextMediaItem()
            } else {
                pendingNext = false
                pendingNextFrom = null
            }
            autoFillInFlight = false
        }
    }

    /** How many songs still follow the current one in play order (shuffle included, repeat off). */
    private fun songsAfter(): Int {
        val t = player.currentTimeline
        if (t.isEmpty || player.currentMediaItemIndex == C.INDEX_UNSET) return 0
        var n = 0
        var i = player.currentMediaItemIndex
        while (true) {
            i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, player.shuffleModeEnabled)
            if (i == C.INDEX_UNSET) break
            n++
        }
        return n
    }

    /** Next with nothing after: kick autofill and take the skip when songs land. */
    private fun fillThenNext() {
        if (player.hasNextMediaItem()) {
            pendingNext = false
            pendingNextFrom = null
            player.seekToNextMediaItem()
            return
        }
        val p = nori.settings.value
        if (!p.autoFill || player.repeatMode != Player.REPEAT_MODE_OFF) return
        pendingNext = true
        pendingNextFrom = player.currentMediaItem?.mediaId
        autoFill(player.currentMediaItem)
    }

    /** The decade [seed] belongs to, for the era basis; empty when the server gave no year. */
    private fun era(seed: Song): IntRange? = seed.year.toInt().takeIf { it > 0 }?.let { (it / 10 * 10)..(it / 10 * 10 + 9) }

    /** Loose songs to carry on with. Whatever the basis, the order they come back in is kept. */
    private suspend fun nextSongs(seed: Song, basis: AutoFillBasis): List<Song> = runCatching {
        when (basis) {
            AutoFillBasis.SIMILAR -> nori.library.similarSongs(seed.id, 25)
            // The artist's best-known songs first, then the rest of their records, so a long evening
            // does not stop after ten tracks.
            AutoFillBasis.ARTIST -> nori.library.topSongs(seed.artist).first().ifEmpty {
                seed.artistId?.let { id -> nori.library.artist(id).first().albums.take(3).flatMap { nori.library.albumSongs(it.id) } }.orEmpty()
            }
            AutoFillBasis.GENRE -> seed.genre?.let { nori.library.songsByGenre(it, 100).shuffled() }.orEmpty()
            // Out of the offline index rather than the server: nothing in Subsonic asks for a decade of songs.
            AutoFillBasis.ERA -> era(seed)?.let { nori.library.browseSongs("playCount", true, false, it, 0, 100).shuffled() }.orEmpty()
        }
    }.getOrDefault(emptyList())

    /**
     * One whole album, in its own order, for someone who listens to records. The album is picked from the
     * same four bases; one already in the queue is passed over, so an evening moves on rather than
     * playing the same record twice.
     */
    private suspend fun nextAlbum(seed: Song, basis: AutoFillBasis, queued: Set<String>, played: Set<String>): List<Song> {
        val candidates = runCatching {
            when (basis) {
                // The records the songs the server calls similar come from.
                AutoFillBasis.SIMILAR -> nori.library.similarSongs(seed.id, 50).filterNot { it.isExternal }.mapNotNull { it.albumId }.distinct()
                AutoFillBasis.ARTIST -> seed.artistId?.let { id -> nori.library.artist(id).first().albums.filterNot { it.isExternal }.map { it.id } }.orEmpty()
                AutoFillBasis.GENRE -> seed.genre?.let { g -> nori.library.albums(AlbumSort.BY_GENRE, 30, genre = g).first().filterNot { it.isExternal }.map { it.id }.shuffled() }.orEmpty()
                AutoFillBasis.ERA -> era(seed)?.let { years -> nori.library.albumsByYear(years.first, years.last, 30).first().filterNot { it.isExternal }.map { it.id }.shuffled() }.orEmpty()
            }
        }.getOrDefault(emptyList())
        // A single is an album as far as the server is concerned, and stopping the evening on one track
        // is not what "carry on with albums" means: the first record with a side to it wins, and a short
        // one is only taken if nothing else is on offer.
        var short = emptyList<Song>()
        for (pick in candidates.filter { it != seed.albumId && it !in played }.take(6)) {
            val songs = runCatching { nori.library.albumSongs(pick).filter { it.id !in queued && !it.isExternal } }.getOrDefault(emptyList())
            if (songs.size >= 3) return songs
            if (songs.size > short.size) short = songs
        }
        return short
    }

    // ---- the queue outlives the process ----

    private fun scheduleSave() {
        main.removeCallbacks(saveQueue)
        main.postDelayed(saveQueue, 1500)
    }

    private fun persistQueue(push: Boolean) {
        main.removeCallbacks(saveQueue)
        // The songs themselves are the core's (crates/core/src/queue.rs); the queue is its ids.
        val ids = List(player.mediaItemCount) { player.getMediaItemAt(it).mediaId }
        val index = player.currentMediaItemIndex.coerceAtLeast(0)
        val current = player.currentMediaItem?.mediaId
        val position = player.currentPosition.coerceAtLeast(0)
        scope.launch(Dispatchers.IO) {
            runCatching { nori.core.queueSave(ids, index.toUInt(), position.toULong()) }
            val songs = ids.filterNot { it.startsWith(RADIO_PREFIX) }
            if (push && songs.isNotEmpty() && nori.settings.value.scrobble) {
                runCatching { nori.library.pushQueue(songs, current, position) }
            }
        }
    }

    private fun restoreQueue() = scope.launch {
        val q = withContext(Dispatchers.IO) { runCatching { nori.core.loadQueue() }.getOrNull() } ?: return@launch
        if (q.songs.isEmpty() || player.mediaItemCount > 0) return@launch
        // Not prepared: nothing touches the network until the user presses play.
        player.setMediaItems(items(q.songs), q.index.toInt().coerceIn(0, q.songs.lastIndex), q.positionMs.toLong())
    }

    /** Songs as the player's items, handed to the core in one call (see MediaItems.toMediaItems). */
    private fun items(songs: List<Song>): List<MediaItem> = songs.toMediaItems { nori.library.coverUrl(it.coverArt, NOTIFICATION_ART) }
    private fun item(s: Song): MediaItem = items(listOf(s)).first()

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
                val (pause, left) = dev.nori.music.ffi.sleepAfter(args.getInt(ARG_SONGS).coerceAtLeast(0).toUInt(), args.getBoolean(ARG_END_OF_TRACK))
                player.pauseAtEndOfMediaItems = pause != 0u
                sleepAfterSongs = left.toInt()
                val minutes = args.getInt(ARG_MINUTES)
                // An alarm, not a Handler: with offloaded playback the CPU sleeps and uptime stops counting.
                if (minutes > 0) dev.nori.music.ffi.sleepDelay(minutes.toUInt()).let { (delay, slack) ->
                    alarms.setWindow(AlarmManager.ELAPSED_REALTIME_WAKEUP, SystemClock.elapsedRealtime() + delay, slack, "nori.sleep", sleepAlarm, main)
                }
            }
            if (command.customAction == CMD_FAVOURITE) {
                val item = player.currentMediaItem?.takeUnless { it.isRadio }
                if (item != null) {
                    val on = !currentStarred(item)
                    // The same path as the app's heart: the mark goes up at once (and redraws both hearts),
                    // the request runs on an IO thread inside Library, and a failure puts the mark back.
                    scope.launch { runCatching { nori.library.star(StarKind.SONG, item.mediaId, on) }.onFailure { android.util.Log.w("nori", "star from the notification failed: $it") } }
                }
            }
            if (command.customAction == CMD_SHUFFLE) player.shuffleModeEnabled = !player.shuffleModeEnabled
            if (command.customAction == CMD_FILL_NEXT) fillThenNext()
            if (command.customAction == CMD_TUNING) {
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
            MediaSession.MediaItemsWithStartPosition(items(q.songs), q.index.toInt(), q.positionMs.toLong())
        }

        override fun onGetLibraryRoot(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, params: LibraryParams?) =
            Futures.immediateFuture(LibraryResult.ofItem(folder("root", "nori"), params))

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

    private fun folder(id: String, title: String, subtitle: String? = null, art: String? = null): MediaItem = MediaItem.Builder().setMediaId(id).setMediaMetadata(
        MediaMetadata.Builder().setTitle(title).setArtist(subtitle).setArtworkUri(art?.let(android.net.Uri::parse))
            .setIsBrowsable(true).setIsPlayable(false).setMediaType(MediaMetadata.MEDIA_TYPE_FOLDER_MIXED).build()
    ).build()

    private suspend fun children(parent: String): List<MediaItem> {
        val lib = nori.library
        val (kind, arg) = parent.substringBefore(':') to parent.substringAfter(':', "")
        return when (kind) {
            "root" -> listOf(folder("albums:RECENT", "Recently played"), folder("albums:NEWEST", "Recently added"), folder("albums:FREQUENT", "Most played"),
                folder("playlists", "Playlists"), folder("starred", "Favourites"), folder("random", "Random"), folder("downloads", "Downloads"))
            "albums" -> lib.albums(AlbumSort.valueOf(arg), size = 40).first().map { folder("album:${it.id}", it.name, it.artist, lib.coverUrl(it.coverArt, 300)) }
            "album" -> items(lib.albumSongs(arg))
            "playlists" -> lib.playlists().first().map { folder("playlist:${it.id}", it.name, "${it.songCount} songs", lib.coverUrl(it.coverArt, 300)) }
            "playlist" -> items(lib.playlistSongs(arg))
            "starred" -> items(lib.starred().first().songs)
            "random" -> items(lib.randomSongs(50))
            "downloads" -> items(nori.downloads.state.value.done)
            else -> emptyList()
        }
    }
}
