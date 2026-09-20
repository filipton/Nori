package dev.flint.music.playback

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
import dev.flint.music.Flint
import dev.flint.music.data.AlbumSort
import dev.flint.music.data.StarKind
import dev.flint.music.ffi.PlayQueue
import dev.flint.music.ffi.Song
import dev.flint.music.settings.AutoFillBasis
import dev.flint.music.settings.AutoFillKind
import dev.flint.music.settings.Prefs
import dev.flint.music.settings.ReplayGainMode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.guava.future
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.min
import kotlin.math.pow

/**
 * The one place audio is played. Built for the screen-off case: decoding is
 * offloaded to the audio DSP when the device can, buffers are large so the
 * radio works in bursts, and nothing here polls or ticks while music plays.
 */
@UnstableApi
class PlaybackService : MediaLibraryService() {
    companion object {
        const val CMD_SLEEP = "flint.sleep"
        const val CMD_TUNING = "flint.tuning"
        /** The notification's and lock screen's heart: favourite or unfavourite the current song. */
        const val CMD_FAVOURITE = "flint.favourite"
        /** The notification's and lock screen's shuffle toggle. */
        const val CMD_SHUFFLE = "flint.shuffle"
        /** Broadcast inside the package on every track or play-state change; what a home-screen widget listens to. */
        const val ACTION_STATE = "dev.flint.music.STATE"
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

    private lateinit var flint: Flint
    private lateinit var player: ExoPlayer
    private lateinit var session: MediaLibrarySession
    private lateinit var scrobbler: Scrobbler
    private val equalizer = Equalizer()
    private var hiRes = false
    private var burst: BurstSink? = null
    /** Kept so a freshly measured track can have its transition planned again; see AutoMixPrefetch. */
    private var transitionSink: TransitionSink? = null
    @Suppress("DEPRECATION")
    private val wifiLock by lazy { applicationContext.getSystemService(WifiManager::class.java).createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "flint:loading").apply { setReferenceCounted(false) } }
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
    private val analysisWorker = java.util.concurrent.Executors.newSingleThreadExecutor { Thread(it, "flint-analysis").apply { priority = Thread.MIN_PRIORITY } }
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
    /** A band is being moved: trade the deep buffer for immediate response. */
    private var tuning = false
    /** Tuning has ended; rebuild with the deep buffer when the music is next paused, where it is silent. */
    private var deepAtNextPause = false
    /**
     * A settings change needs a new sink chain but the music is playing through the old one.
     * Rebuilding mid-track cuts the song, so it waits for the next boundary instead; see
     * [reconfigureSink]. A silent path never waits (see there); the deep buffer's return goes
     * at the next pause instead.
     */
    private var chainSwapPending = false
    /**
     * The last thing that happens before the AudioTrack exists, and the only place that knows exactly what it
     * will be: encoding, rate and whether the stream is being offloaded. Preferred mixer attributes are read
     * by the framework when the track is built, so a DAC can only be engaged from here - setting them
     * afterwards, as this used to, changes nothing about the track already playing.
     */
    private val tracks = DefaultAudioSink.AudioTrackProvider { config, attrs, sessionId, context ->
        flint.dac.onFormat(config.sampleRate, config.encoding)
        val track = DefaultAudioSink.AudioTrackProvider.DEFAULT.getAudioTrack(config, attrs, sessionId, context)
        // Pin the track to the DAC as well: bit-perfect attributes apply to one device, and letting Android
        // pick the route again afterwards is how the two end up disagreeing.
        flint.dac.preferredDevice()?.let { runCatching { track.setPreferredDevice(it) } }
        flint.dac.onTrack(config.sampleRate, config.encoding, config.offload)
        android.util.Log.i("flint", "AudioTrack ${config.sampleRate} Hz enc=${config.encoding} buffer=${config.bufferSize} offload=${config.offload} usb=${flint.outputs.usb.value}")
        track
    }

    private val shallowBuffer = DefaultAudioTrackBufferSizeProvider.Builder().build()
    private val deepBuffer = DefaultAudioTrackBufferSizeProvider.Builder().setTargetPcmBufferDurationUs(BurstSink.BUFFER_US).setMaxPcmBufferDurationUs(BurstSink.BUFFER_US).build()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val main = Handler(Looper.getMainLooper())
    private val served = LruCache<String, MediaItem>(500)
    private val saveQueue = Runnable { persistQueue(push = false) }
    private val sleepAlarm = AlarmManager.OnAlarmListener { player.pause() }

    override fun onCreate() {
        super.onCreate()
        Equalizer.active = equalizer
        flint = Flint.get(this)
        scrobbler = Scrobbler(flint, scope)

        val renderers = object : DefaultRenderersFactory(this) {
            override fun buildAudioSink(context: android.content.Context, enableFloatOutput: Boolean, enableAudioTrackPlaybackParams: Boolean): AudioSink =
                TransitionSink(BurstSink(
                    DefaultAudioSink.Builder(context).setAudioProcessors(arrayOf(equalizer))
                        // Formats the DSP cannot take (FLAC on most phones) are decoded on the CPU; see BurstSink.
                        .setAudioTrackBufferSizeProvider { min, encoding, mode, frameSize, rate, bitrate, speed ->
                            // Deep for bursts; shallow only while the equalizer screen is open, so that moving a band is heard at once.
                            (if (tuning) shallowBuffer else deepBuffer).getBufferSizeInBytes(min, encoding, mode, frameSize, rate, bitrate, speed)
                        }
                        .setEnableFloatOutput(enableFloatOutput).setEnableAudioTrackPlaybackParams(enableAudioTrackPlaybackParams)
                        .setAudioTrackProvider(tracks).build()
                ).also { burst = it }, transitions).also { transitionSink = it }
        }
        hiRes = flint.settings.value.hiRes
        renderers.setEnableAudioFloatOutput(hiRes)
        player = ExoPlayer.Builder(this, renderers)
            .setMediaSourceFactory(DefaultMediaSourceFactory(this, DefaultExtractorsFactory().setConstantBitrateSeekingEnabled(true)).setDataSourceFactory(flint.sources.factory))
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
                DefaultLoadControl.Builder().setBufferDurationsMs(60_000, 600_000, 1_000, 2_000)
                    .setTargetBufferBytes(minOf(48, getSystemService(android.app.ActivityManager::class.java).memoryClass / 4).coerceAtLeast(16) * 1024 * 1024).setPrioritizeTimeOverSizeThresholds(false).build()
            )
            .build()
        precacher = Precacher(flint.sources)
        analyser = AutoMixPrefetch(flint.sources, { flint.core }) { transitionSink?.replan() }
        player.addListener(listener)
        player.addAudioOffloadListener(object : ExoPlayer.AudioOffloadListener {
            override fun onOffloadedPlayback(offloaded: Boolean) { this@PlaybackService.offloaded = offloaded; updateBurst() }
        })

        // Fired from the audio device callback (main) and from the audio track provider (playback thread);
        // the player may only be touched on the main looper.
        flint.dac.onChanged = { main.post { applyAudio(flint.settings.value); applyGain() } }
        flint.dac.start()
        flint.outputs.start()
        // Plugging in headphones or a DAC swaps the whole sound chain, if a profile is bound to it.
        scope.launch {
            // A device arriving or leaving changes what the audio chain may do (see applyAudio), whether or
            // not the user binds sound profiles to outputs.
            flint.outputs.usb.collect { applyAudio(flint.settings.value) }
        }
        scope.launch {
            // The device's own sound (a bound profile or AutoEQ curve), or the sound from before it came back.
            flint.outputs.current.collect { output -> flint.deviceSound.onOutput(output) }
        }
        applyAudio(flint.settings.value)
        scope.launch {
            var last = flint.settings.value
            flint.settings.prefs.collect { p ->
                if (p.copy(replayGain = last.replayGain, preampDb = last.preampDb, untaggedGainDb = last.untaggedGainDb, scrobblePercent = last.scrobblePercent, listPrefs = last.listPrefs, homeRows = last.homeRows, pinnedPlaylists = last.pinnedPlaylists, autoEqAuto = last.autoEqAuto) != last) applyAudio(p)
                if (p.replayGain != last.replayGain || p.preampDb != last.preampDb || p.untaggedGainDb != last.untaggedGainDb) applyGain()
                last = p
            }
        }

        val open = packageManager.getLaunchIntentForPackage(packageName)?.let { PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE) }
        session = MediaLibrarySession.Builder(this, Controls(player), Callback())
            // Notification and lock-screen art: same connection pool as everything else, last bitmap kept, decoded no larger than needed.
            .setBitmapLoader(CacheBitmapLoader(DataSourceBitmapLoader.Builder(this).setDataSourceFactory(flint.sources.network).setMaximumOutputDimension(512).build()))
            // Controllers extrapolate the playhead themselves; a broadcast every few seconds is a wake-up for nothing.
            .setPeriodicPositionUpdateEnabled(false)
            .apply { open?.let(::setSessionActivity) }.build()
        // The notification and the lock screen carry the app's own mark, not media3's stock play circle.
        // Its id stays media3's default (1001): the download notification lives on 2001 so the two never replace each other.
        setMediaNotificationProvider(
            androidx.media3.session.DefaultMediaNotificationProvider.Builder(this)
                .setNotificationId(androidx.media3.session.DefaultMediaNotificationProvider.DEFAULT_NOTIFICATION_ID).build()
                .apply { setSmallIcon(dev.flint.music.core.R.drawable.ic_notification) },
        )
        // A heart changed anywhere in the app (or by the notification itself) redraws the notification's heart.
        // A StateFlow: it emits only on a change, so this is idle while music plays untouched.
        scope.launch { flint.library.starMarks.collect { refreshButtons() } }
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
        flint.dac.onChanged = {}
        flint.dac.stop()
        flint.outputs.stop()
        if (wifiLock.isHeld) wifiLock.release()
        analysisWorker.shutdown()
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
            if (chainSwapPending && !(reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT && player.repeatMode == Player.REPEAT_MODE_ONE)) {
                chainSwapPending = false
                android.util.Log.i("flint", "chain swap at the boundary")
                if (player.playbackState != Player.STATE_IDLE) { player.stop(); player.prepare() }
            }
            refreshButtons()
            if (item != null && flint.settings.value.skipExplicit && item.mediaMetadata.extras?.getString("explicit") == "explicit" && player.hasNextMediaItem()) return player.seekToNextMediaItem()
            if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT && item != null) return scrobbler.onTrack(item.toSong(), player.isPlaying)
            scrobbler.onTrack(item?.takeUnless { it.isRadio }?.toSong(), player.isPlaying)
            applyGain()
            scheduleSave()
            autoFill(item)
            announce()
            errorsInARow = 0
            refreshUpcoming()
            // A few seconds in, the current track has been fetched and the radio is still up: fetch ahead now.
            main.removeCallbacks(precache)
            main.postDelayed(precache, 6_000)
            if (sleepAfterSongs > 0 && --sleepAfterSongs == 0) player.pauseAtEndOfMediaItems = true
        }

        override fun onIsLoadingChanged(isLoading: Boolean) {
            if (isLoading && !wifiLock.isHeld) wifiLock.acquire() else if (!isLoading && wifiLock.isHeld) wifiLock.release()
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            if (!isPlaying && !player.playWhenReady && deepAtNextPause) {
                deepAtNextPause = false
                // Paused is silent: the deep buffer comes back at once rather than waiting a track.
                reconfigureSink(urgent = true)
            }
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
            // An output that refuses the offloaded stream fails here, and skipping to the next track would fail
            // the same way. Rebuild the chain on the CPU path instead and play the same track again.
            val sink = generateSequence(error.cause) { it.cause }.any {
                it is AudioSink.InitializationException || it is AudioSink.WriteException || it is AudioSink.ConfigurationException
            }
            if (sink && !offloadRefused) {
                android.util.Log.w("flint", "audio sink refused the stream, giving up offload", error)
                offloadRefused = true
                applyAudio(flint.settings.value)
                player.prepare()
                player.play()
                return
            }
            // One unplayable or unreachable track should not end the evening; three in a row probably means the server is gone.
            if (flint.settings.value.skipOnError && player.hasNextMediaItem() && ++errorsInARow <= 3) {
                player.seekToNextMediaItem()
                player.prepare()
                player.play()
            }
        }

        override fun onPlaybackStateChanged(state: Int) {
            if (state == Player.STATE_ENDED) scrobbler.onTrack(null, false)
        }
    }


    /** What the heart and shuffle buttons last showed, so an unrelated change does not rebuild the notification. */
    private var buttonsShown: String? = null

    private fun currentStarred(item: MediaItem): Boolean =
        flint.library.isStarred(StarKind.SONG, item.mediaId, item.mediaMetadata.extras?.getBoolean("starred") == true)

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

    private fun updateBurst() { burst?.enabled = !offloaded && !tuning }

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
            if (player.playbackState != Player.STATE_IDLE) { player.stop(); player.prepare() }
            return
        }
        if (offloaded && !offloadWanted) {
            android.util.Log.i("flint", "chain swap now: the offloaded path is ending")
            player.stop(); player.prepare()
            return
        }
        if (!chainSwapPending) android.util.Log.i("flint", "chain swap deferred to the next track")
        chainSwapPending = true
    }

    /** DSP, crossfade, speed, offload and bit-perfect constrain each other; this is where that is decided. */
    private fun applyAudio(p: Prefs) {
        flint.dac.setEnabled(p.bitPerfect)
        val untouched = hiRes || flint.dac.state.value.bitPerfect
        val processing = p.dsp && !untouched
        equalizer.setChain(if (p.eqEnabled) p.eqBands else emptyList(), p.effectivePreampDb, p.crossfeedDb)
        equalizer.setOutput(p.balance, p.mono, p.limiterThresholdDb, 120f, if (p.limiter) 5f else 0f)
        transitionsOff = untouched
        // The pinned output rate stands down with everything else that touches samples.
        transitionSink?.lockRate = !untouched
        player.skipSilenceEnabled = p.skipSilence && !untouched
        // Speed and pitch ride the player's parameters, which the sink's own stretcher hears live -
        // no rebuild, no gap. The one exception is an offloaded track: the chip plays what it was
        // given at 1x, so leaving offload still cuts (and the position is kept).
        val tempoChanged = appliedSpeed != p.speed || appliedPitch != p.pitch
        appliedSpeed = p.speed
        appliedPitch = p.pitch
        player.playbackParameters = androidx.media3.common.PlaybackParameters(p.speed, p.pitch)
        // Offload hands the compressed stream to the audio chip, so it is only possible while the app needs no samples.
        // It also only reaches the phone's own outputs: the chip has no path to a USB DAC, but Android still
        // opens the offloaded track and reports it playing, so the DAC just sits there in silence. Decode on
        // the CPU whenever anything USB is attached, and after the sink has failed to open once (offloadRefused).
        val usb = flint.outputs.usb.value
        val offload = p.offload && !processing && !usb && !offloadRefused &&
            p.crossfadeSec == 0 && !p.autoMix && !p.skipSilence && p.speed == 1f && p.pitch == 1f
        player.trackSelectionParameters = player.trackSelectionParameters.buildUpon().setAudioOffloadPreferences(
            AudioOffloadPreferences.Builder()
                .setAudioOffloadMode(if (offload) AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_ENABLED else AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_DISABLED)
                .setIsGaplessSupportRequired(true).build()
        ).build()
        // A renderer already playing keeps the path it was built with, so a DAC plugged in mid-song would
        // stay on the offloaded - and silent - one until the next track. Rebuild now; stop() keeps the position.
        val offloadChanged = offloadWanted != offload
        offloadWanted = offload
        if (equalizer.enabled != processing) {
            equalizer.enabled = processing
            reconfigureSink()
        } else if (offloadChanged && offloaded) reconfigureSink(urgent = true)
        else if (tempoChanged && offloaded) reconfigureSink(urgent = true)
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
        shuffling = player.shuffleModeEnabled
        val t = player.currentTimeline
        val was = upcoming
        if (t.isEmpty || player.currentMediaItemIndex == C.INDEX_UNSET) { upcoming = emptyList(); return }
        val list = ArrayList<MediaItem>(8)
        var i = player.currentMediaItemIndex
        while (i != C.INDEX_UNSET && list.size < 8) {
            list += player.getMediaItemAt(i)
            i = t.getNextWindowIndex(i, player.repeatMode, player.shuffleModeEnabled)
        }
        upcoming = list
        if (was.size != list.size || was.indices.any { was[it].mediaId != list[it].mediaId }) transitionSink?.replan()
    }

    /**
     * Plans each transition when the sink reaches a track: from the stored analyses (AutoMix) or, without them,
     * a plain crossfade. The Rust planner decides; this only gathers its inputs.
     */
    private val transitions = object : TransitionSink.Listener {
        override fun planFor(outgoingId: String): TransitionSink.Plan? {
            val p = flint.settings.value
            if (transitionsOff || (!p.autoMix && p.crossfadeSec == 0)) {
                android.util.Log.i("flint", "planFor: off (transitionsOff=$transitionsOff autoMix=${p.autoMix} crossfadeSec=${p.crossfadeSec})")
                return null
            }
            // Why a boundary passed without a transition is otherwise invisible, and every reason below
            // is a deliberate one - which is hard to tell apart from a broken feature without a word.
            val order = upcoming
            val at = order.indexOfFirst { it.mediaId == outgoingId }.takeIf { it >= 0 } ?: run {
                android.util.Log.i("flint", "planFor: $outgoingId not in the upcoming window")
                return null
            }
            val out = order[at]
            val next = order.getOrNull(at + 1) ?: run { android.util.Log.i("flint", "planFor: nothing after $outgoingId"); return null }
            if (out.isRadio || next.isRadio) { android.util.Log.i("flint", "planFor: radio"); return null }
            val settings = dev.flint.music.ffi.AutoMixSettings(
                maxTransitionS = (if (p.autoMix) p.autoMixMaxS else p.crossfadeSec).toFloat(),
                beatMatch = p.autoMix && p.autoMixBeatMatch, maxTempoChangePct = p.autoMixMaxTempoPct,
                bassSwap = p.autoMix && p.autoMixBassSwap, filterEffects = p.autoMix && p.autoMixFilters, echoOut = p.autoMix && p.autoMixEchoOut,
                keepPitch = p.autoMixKeepPitch, sameAlbumInOrder = p.crossfadeKeepAlbums && followsOnAlbum(out, next),
                matchLoudness = false,
            )
            val a = if (p.autoMix) runCatching { flint.core.analysisGet(out.mediaId) }.getOrNull() else null
            val b = if (p.autoMix) runCatching { flint.core.analysisGet(next.mediaId) }.getOrNull() else null
            val outMs = (out.mediaMetadata.durationMs ?: 0L)
            val inMs = (next.mediaMetadata.durationMs ?: 0L)
            if (outMs <= 0 || inMs <= 0) { android.util.Log.i("flint", "planFor: durations $outMs/$inMs"); return null }
            val plan = dev.flint.music.ffi.planTransition(a, b, outMs, inMs, settings)
            if (plan.kind == dev.flint.music.ffi.TransitionKind.GAPLESS) { android.util.Log.i("flint", "planFor: gapless (${plan.reason})"); return null }
            android.util.Log.i("flint", "transition ${out.mediaMetadata.title} -> ${next.mediaMetadata.title}: ${plan.kind} ${plan.durationMs} ms at ${plan.outStartMs}, tempo x${"%.3f".format(plan.tempoRatio)} (${plan.reason})")
            return TransitionSink.Plan(
                incomingId = next.mediaId, outStartUs = plan.outStartMs * 1000, durationUs = plan.durationMs * 1000, inSkipUs = plan.inStartMs * 1000,
                mixer = dev.flint.music.ffi.automixMixerParams(plan).toFloatArray(), tempoRatio = plan.tempoRatio.toFloat(),
                keepPitch = plan.keepPitch, rampUs = plan.tempoRampMs * 1000,
            )
        }

        override fun wantsAnalysis(songId: String): Boolean {
            if (!flint.settings.value.autoMix || songId.startsWith(RADIO_PREFIX) || songId.startsWith("ext-")) return false
            // Missing *or* measured by an older analyser: without the overlap windows the pair
            // gates read zeros and never fire, so old rows are measured again, not kept.
            return runCatching { flint.core.analysisMissing(listOf(songId)).isNotEmpty() }.getOrDefault(false)
        }

        override fun analysed(songId: String, handle: Long, frames: Long, sampleRate: Int) {
            analysisWorker.execute {
                try {
                    // Only a track heard from start to end: the duration has to agree with what the server says.
                    val expectedMs = upcoming.firstOrNull { it.mediaId == songId }?.mediaMetadata?.durationMs ?: 0L
                    val heardMs = frames * 1000 / sampleRate.coerceAtLeast(1)
                    if (expectedMs <= 0 || kotlin.math.abs(heardMs - expectedMs) <= 3000) {
                        val a = flint.core.analysisFinishStream(songId, handle)
                        android.util.Log.i("flint", "analysed $songId: ${a?.let { "%.2f bpm (conf %.2f, stab %.2f), key %s, heard %d ms of %d ms, %d frames at %d Hz".format(it.bpm, it.bpmConfidence, it.stability, dev.flint.music.ffi.automixKeyName(it.key), it.durationMs, expectedMs, frames, sampleRate) } ?: "too short"}")
                    }
                } catch (e: Exception) {
                    android.util.Log.w("flint", "analysis of $songId failed: $e")
                } finally {
                    AutoMixAnalyzer.destroy(handle)
                }
            }
        }
    }

    private fun nextItem(): MediaItem? = player.nextMediaItemIndex.takeIf { it != C.INDEX_UNSET }?.let(player::getMediaItemAt)
    private fun previousItem(): MediaItem? = player.previousMediaItemIndex.takeIf { it != C.INDEX_UNSET }?.let(player::getMediaItemAt)

    /** Two tracks are "the album in order" only when the queue is actually playing it in order: shuffle breaks that. */
    private fun followsOnAlbum(a: MediaItem?, b: MediaItem?): Boolean {
        // The flag, not the player: this runs on the audio thread while a buffer is being handed over,
        // and ExoPlayer throws if it is touched from anywhere but the thread that owns it.
        if (shuffling) return false
        val (x, y) = (a?.mediaMetadata?.extras ?: return false) to (b?.mediaMetadata?.extras ?: return false)
        val album = x.getString("albumId")
        return album != null && album == y.getString("albumId") && x.getInt("disc") == y.getInt("disc") && y.getInt("track") == x.getInt("track") + 1
    }

    /** ReplayGain as plain volume: costs nothing and survives offload. Attenuation only. */
    private fun applyGain() {
        val p = flint.settings.value
        val item = player.currentMediaItem?.takeUnless { it.isRadio }
        targetVolume = if (p.replayGain == ReplayGainMode.OFF || item == null || flint.dac.state.value.bitPerfect) 1f else {
            val g = item.toSong().replayGain
            val album = when (p.replayGain) {
                ReplayGainMode.ALBUM -> true
                // Inside an album played in order the album gain keeps the tracks' relative levels; elsewhere track gain evens things out.
                ReplayGainMode.AUTO -> followsOnAlbum(item, nextItem()) || followsOnAlbum(previousItem(), item)
                else -> false
            }
            val db = if (g == null) p.untaggedGainDb else ((if (album) g.albumGain ?: g.trackGain else g.trackGain ?: g.albumGain) ?: p.untaggedGainDb) + p.preampDb
            val peak = g?.let { if (album) it.albumPeak ?: it.trackPeak else it.trackPeak ?: it.albumPeak } ?: 0f
            var v = 10f.pow(db / 20f)
            if (peak > 0f) v = min(v, 1f / peak)
            v.coerceIn(0f, 1f)
        }
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
                player.volume = from + (to * targetVolume - from) * t
                if (t < 1f) main.postDelayed(this, 16) else { fade = null; then() }
            }
        }.also(main::post)
    }

    /** What the session (notification, headset, our UI, Android Auto) actually controls: the player plus the configured manners. */
    private inner class Controls(p: ExoPlayer) : androidx.media3.common.ForwardingPlayer(p) {
        private val ms get() = flint.settings.value.fadeMs

        override fun play() {
            if (ms > 0 && !wrappedPlayer.isPlaying) wrappedPlayer.volume = 0f
            super.play()
            if (ms > 0) ramp(1f, ms)
        }

        override fun pause() {
            if (ms > 0 && wrappedPlayer.isPlaying) ramp(0f, ms) { super.pause(); wrappedPlayer.volume = targetVolume } else super.pause()
        }

        private fun softly(action: () -> Unit) {
            if (ms > 0 && wrappedPlayer.isPlaying) { wrappedPlayer.volume = 0f; action(); ramp(1f, ms) } else action()
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

        override fun seekTo(positionMs: Long) = softly { super.seekTo(positionMs) }
        override fun seekTo(mediaItemIndex: Int, positionMs: Long) = softly { super.seekTo(mediaItemIndex, positionMs) }
        override fun seekToNext() = andPlay { softly { super.seekToNext() } }
        override fun seekToNextMediaItem() = andPlay { softly { super.seekToNextMediaItem() } }
        override fun seekToPreviousMediaItem() = andPlay { softly { super.seekToPreviousMediaItem() } }
        // Well into a song this goes back to 0:00 rather than to the song before (media3's own rule,
        // three seconds), which paused means: start this one again, from the top, playing.
        override fun seekToPrevious() = andPlay {
            softly { if (flint.settings.value.previousAlwaysSkips && hasPreviousMediaItem()) super.seekToPreviousMediaItem() else super.seekToPrevious() }
        }
    }

    /**
     * Play next and Add to queue, the way Apple does them: the songs go right after the playing one
     * ("next"), or after the songs added by hand before them ("last"), in the order given, and then the
     * queue carries on as it was. In the list itself they sit there too, so turning shuffle off keeps
     * them next. Under shuffle, ExoPlayer would drop each at a random place in the play order, so the
     * order is rebuilt with them where they belong and everything else where it was.
     */
    private fun upNext(items: List<MediaItem>) {
        val t = player.currentTimeline
        val cur = player.currentMediaItemIndex
        val last = items.first().queuedAs() == "last"
        fun runEnd(shuffled: Boolean): Int {
            var end = cur
            if (!last) return end
            var i = t.getNextWindowIndex(cur, Player.REPEAT_MODE_OFF, shuffled)
            while (i != C.INDEX_UNSET && player.getMediaItemAt(i).queuedAs() != null) { end = i; i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, shuffled) }
            return end
        }
        val at = runEnd(false) + 1
        if (!player.shuffleModeEnabled) return player.addMediaItems(at, items)
        val shift = { i: Int -> if (i >= at) i + items.size else i }
        val order = ArrayList<Int>(t.windowCount + items.size)
        var i = t.getFirstWindowIndex(true)
        while (i != C.INDEX_UNSET) { order += shift(i); i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, true) }
        order.addAll(order.indexOf(shift(runEnd(true))) + 1, items.indices.map { at + it })
        player.addMediaItems(at, items)
        player.setShuffleOrder(DefaultShuffleOrder(order.toIntArray(), SystemClock.elapsedRealtime()))
    }

    /**
     * Shuffle turned on: the playing song goes first, the songs added by hand after it keep their order,
     * and only the rest is shuffled. ExoPlayer's own order would leave the playing song somewhere in the
     * middle, so the songs before it in that order were never played, and it scatters the hand-added ones.
     */
    private fun shuffleAroundCurrent() {
        val n = player.mediaItemCount
        val cur = player.currentMediaItemIndex
        if (n < 2 || cur == C.INDEX_UNSET) return
        val kept = ArrayList<Int>().apply {
            add(cur)
            var i = cur + 1
            while (i < n && player.getMediaItemAt(i).queuedAs() != null) add(i++)
        }
        val rest = (0 until n).filterNot { it in kept }.shuffled()
        player.setShuffleOrder(DefaultShuffleOrder((kept + rest).toIntArray(), SystemClock.elapsedRealtime()))
    }

    private fun precacheAhead() {
        val p = flint.settings.value
        val count = if (flint.http.metered) p.precacheMobile else p.precacheWifi
        // The player itself already buffers the very next track, which is why this normally covers the
        // ones after it - except with a transition on. A mix needs that next track decodable a whole
        // crossfade before the player would otherwise want it, and audio that arrives too late to be
        // mixed into leaves a hole where the end of the song should be (see TransitionSink), so then it
        // is fetched here too. Nothing extra is fetched, only earlier: these are the same bytes the
        // player would ask for a minute later.
        val mixing = !transitionsOff && (p.crossfadeSec > 0 || p.autoMix)
        val first = if (mixing) 1 else 2
        // Shuffle moves the goalposts, so nothing is fetched deep into a queue about to be reordered -
        // but the next track is the next track whatever the order.
        val last = maxOf(if (player.shuffleModeEnabled) 0 else count, if (mixing) 1 else 0)
        if (last < first) return precacher.cancel()
        precacher.update(upcoming.drop(first).take(last - first + 1))
    }

    /**
     * Measures the track playing and the two after it, unless they have been measured before. AutoMix
     * plans a transition from both halves' analyses, and until this existed the only way to get one was
     * to have played the track through: the first time two songs met they were faded rather than mixed,
     * and the plan for the boundary the listener was already in the middle of arrived too late to use.
     * Off entirely when AutoMix is, and it never fetches anything (see AutoMixPrefetch).
     */
    private fun analyseAhead() {
        if (!flint.settings.value.autoMix) return analyser.cancel()
        analyser.update(upcoming.take(3).map { it.mediaId })
    }

    /**
     * Keeps the music going past the end of the queue. Runs once, when the last song starts: the radio
     * is up for that song anyway. What arrives is the user's choice twice over - songs or a whole album
     * ([AutoFillKind]), chosen by what the server calls similar or by the artist, genre or decade
     * ([AutoFillBasis]) - and every route here reads the library, so this never makes octo-fiesta
     * download a provider track.
     */
    private fun autoFill(item: MediaItem?) {
        val p = flint.settings.value
        if (item == null || item.isRadio || player.hasNextMediaItem() || player.repeatMode != Player.REPEAT_MODE_OFF || !p.autoFill) return
        val seed = item.toSong()
        if (seed.isExternal) return
        // Both read off the queue before anything suspends: the player belongs to this looper, and what
        // follows runs on an IO thread.
        val queued = (0 until player.mediaItemCount).mapTo(HashSet()) { player.getMediaItemAt(it).mediaId }
        val played = (0 until player.mediaItemCount).mapNotNullTo(HashSet()) { player.getMediaItemAt(it).mediaMetadata.extras?.getString("albumId") }
        scope.launch {
            val fresh = withContext(Dispatchers.IO) {
                if (p.autoFillKind == AutoFillKind.ALBUMS) nextAlbum(seed, p.autoFillBasis, queued, played)
                else nextSongs(seed, p.autoFillBasis).filter { it.id !in queued && !it.isExternal }.take(15)
            }
            if (fresh.isNotEmpty() && !player.hasNextMediaItem()) player.addMediaItems(fresh.map(::item))
        }
    }

    /** The decade [seed] belongs to, for the era basis; empty when the server gave no year. */
    private fun era(seed: Song): IntRange? = seed.year.toInt().takeIf { it > 0 }?.let { (it / 10 * 10)..(it / 10 * 10 + 9) }

    /** Loose songs to carry on with. Whatever the basis, the order they come back in is kept. */
    private suspend fun nextSongs(seed: Song, basis: AutoFillBasis): List<Song> = runCatching {
        when (basis) {
            AutoFillBasis.SIMILAR -> flint.library.similarSongs(seed.id, 25)
            // The artist's best-known songs first, then the rest of their records, so a long evening
            // does not stop after ten tracks.
            AutoFillBasis.ARTIST -> flint.library.topSongs(seed.artist).first().ifEmpty {
                seed.artistId?.let { id -> flint.library.artist(id).first().albums.take(3).flatMap { flint.library.albumSongs(it.id) } }.orEmpty()
            }
            AutoFillBasis.GENRE -> seed.genre?.let { flint.library.songsByGenre(it, 100).shuffled() }.orEmpty()
            // Out of the offline index rather than the server: nothing in Subsonic asks for a decade of songs.
            AutoFillBasis.ERA -> era(seed)?.let { flint.library.browseSongs("playCount", true, false, it, 0, 100).shuffled() }.orEmpty()
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
                AutoFillBasis.SIMILAR -> flint.library.similarSongs(seed.id, 50).filterNot { it.isExternal }.mapNotNull { it.albumId }.distinct()
                AutoFillBasis.ARTIST -> seed.artistId?.let { id -> flint.library.artist(id).first().albums.filterNot { it.isExternal }.map { it.id } }.orEmpty()
                AutoFillBasis.GENRE -> seed.genre?.let { g -> flint.library.albums(AlbumSort.BY_GENRE, 30, genre = g).first().filterNot { it.isExternal }.map { it.id }.shuffled() }.orEmpty()
                AutoFillBasis.ERA -> era(seed)?.let { years -> flint.library.albumsByYear(years.first, years.last, 30).first().filterNot { it.isExternal }.map { it.id }.shuffled() }.orEmpty()
            }
        }.getOrDefault(emptyList())
        // A single is an album as far as the server is concerned, and stopping the evening on one track
        // is not what "carry on with albums" means: the first record with a side to it wins, and a short
        // one is only taken if nothing else is on offer.
        var short = emptyList<Song>()
        for (pick in candidates.filter { it != seed.albumId && it !in played }.take(6)) {
            val songs = runCatching { flint.library.albumSongs(pick).filter { it.id !in queued && !it.isExternal } }.getOrDefault(emptyList())
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
        val songs = (0 until player.mediaItemCount).map(player::getMediaItemAt).filterNot { it.isRadio }.map { it.toSong() }
        val index = player.currentMediaItemIndex.coerceAtLeast(0)
        val position = player.currentPosition.coerceAtLeast(0)
        scope.launch(Dispatchers.IO) {
            runCatching { flint.core.saveQueue(PlayQueue(songs, index.toUInt(), position.toULong())) }
            if (push && songs.isNotEmpty() && flint.settings.value.scrobble) {
                runCatching { flint.library.pushQueue(songs.map { it.id }, songs.getOrNull(index)?.id, position) }
            }
        }
    }

    private fun restoreQueue() = scope.launch {
        val q = withContext(Dispatchers.IO) { runCatching { flint.core.loadQueue() }.getOrNull() } ?: return@launch
        if (q.songs.isEmpty() || player.mediaItemCount > 0) return@launch
        // Not prepared: nothing touches the network until the user presses play.
        player.setMediaItems(q.songs.map(::item), q.index.toInt().coerceIn(0, q.songs.lastIndex), q.positionMs.toLong())
    }

    private fun item(s: Song): MediaItem = s.toMediaItem(flint.library.coverUrl(s.coverArt, NOTIFICATION_ART))

    // ---- session: custom commands, Android Auto browsing, voice search ----

    private inner class Callback : MediaLibrarySession.Callback {
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
            val commands = MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS.buildUpon().add(SessionCommand(CMD_SLEEP, Bundle.EMPTY)).add(SessionCommand(CMD_TUNING, Bundle.EMPTY))
                .add(SessionCommand(CMD_FAVOURITE, Bundle.EMPTY)).add(SessionCommand(CMD_SHUFFLE, Bundle.EMPTY)).build()
            return MediaSession.ConnectionResult.AcceptedResultBuilder(session).setAvailableSessionCommands(commands).build()
        }

        override fun onCustomCommand(session: MediaSession, controller: MediaSession.ControllerInfo, command: SessionCommand, args: Bundle): ListenableFuture<SessionResult> {
            if (command.customAction == CMD_SLEEP) {
                val alarms = getSystemService(AlarmManager::class.java)
                alarms.cancel(sleepAlarm)
                sleepAfterSongs = args.getInt(ARG_SONGS)
                player.pauseAtEndOfMediaItems = args.getBoolean(ARG_END_OF_TRACK) || sleepAfterSongs == 1
                if (sleepAfterSongs == 1) sleepAfterSongs = 0 else if (sleepAfterSongs > 1) sleepAfterSongs--
                val minutes = args.getInt(ARG_MINUTES)
                // An alarm, not a Handler: with offloaded playback the CPU sleeps and uptime stops counting.
                if (minutes > 0) alarms.setWindow(AlarmManager.ELAPSED_REALTIME_WAKEUP, SystemClock.elapsedRealtime() + minutes * 60_000L, 15_000L, "flint.sleep", sleepAlarm, main)
            }
            if (command.customAction == CMD_FAVOURITE) {
                val item = player.currentMediaItem?.takeUnless { it.isRadio }
                if (item != null) {
                    val on = !currentStarred(item)
                    // The same path as the app's heart: the mark goes up at once (and redraws both hearts),
                    // the request runs on an IO thread inside Library, and a failure puts the mark back.
                    scope.launch { runCatching { flint.library.star(StarKind.SONG, item.mediaId, on) }.onFailure { android.util.Log.w("flint", "star from the notification failed: $it") } }
                }
            }
            if (command.customAction == CMD_SHUFFLE) player.shuffleModeEnabled = !player.shuffleModeEnabled
            if (command.customAction == CMD_TUNING) {
                val on = args.getBoolean(ARG_ON)
                // Rebuilding the sink to swap the deep buffer for a shallow one is a stop and a prepare -
                // an audible drop. So it happens only when a band is actually moving and the equalizer
                // is actually in the chain; with it off, or bypassed for a DAC, a change is inaudible
                // either way and there is nothing to rebuild for.
                if (on && !tuning && equalizer.enabled) {
                    tuning = true
                    updateBurst()
                    // Live tweaking needs the shallow buffer now; the cut is the price of it.
                    reconfigureSink(urgent = true)
                } else if (!on && tuning) {
                    // And not straight back either: leaving the screen would cut the song a second time.
                    // The shallow buffer costs some wakeups, not sound, so it lasts until the next pause.
                    tuning = false
                    updateBurst()
                    deepAtNextPause = true
                }
            }
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
        }

        override fun onAddMediaItems(session: MediaSession, controller: MediaSession.ControllerInfo, items: MutableList<MediaItem>): ListenableFuture<MutableList<MediaItem>> {
            val query = items.singleOrNull()?.requestMetadata?.searchQuery
            if (query != null) return scope.future { flint.library.search(query).songs.map(::item).toMutableList() }
            // Our own UI sends complete items. Android Auto sends bare ids of things it was shown earlier.
            if (items.all { it.mediaMetadata.title != null }) return Futures.immediateFuture(items.map { it.playable() }.toMutableList())
            return scope.future {
                items.mapNotNull { i -> served.get(i.mediaId) ?: withContext(Dispatchers.IO) { runCatching { flint.library.song(i.mediaId) }.getOrNull() }?.let(::item) }.toMutableList()
            }
        }

        override fun onPlaybackResumption(session: MediaSession, controller: MediaSession.ControllerInfo): ListenableFuture<MediaSession.MediaItemsWithStartPosition> = scope.future {
            val q = withContext(Dispatchers.IO) { flint.core.loadQueue() }
            MediaSession.MediaItemsWithStartPosition(q.songs.map(::item), q.index.toInt(), q.positionMs.toLong())
        }

        override fun onGetLibraryRoot(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, params: LibraryParams?) =
            Futures.immediateFuture(LibraryResult.ofItem(folder("root", "flint"), params))

        override fun onGetChildren(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, parentId: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> =
            scope.future {
                val children = runCatching { children(parentId) }.getOrDefault(emptyList())
                children.forEach { if (it.mediaMetadata.isPlayable == true) served.put(it.mediaId, it) }
                LibraryResult.ofItemList(children.drop(page * pageSize).take(pageSize), params)
            }

        override fun onSearch(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, query: String, params: LibraryParams?): ListenableFuture<LibraryResult<Void>> {
            scope.launch {
                val n = runCatching { flint.library.search(query).songs.size }.getOrDefault(0)
                session.notifySearchResultChanged(browser, query, n, params)
            }
            return Futures.immediateFuture(LibraryResult.ofVoid())
        }

        override fun onGetSearchResult(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, query: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> =
            scope.future {
                val songs = runCatching { flint.library.search(query).songs.map(::item) }.getOrDefault(emptyList())
                songs.forEach { served.put(it.mediaId, it) }
                LibraryResult.ofItemList(songs.drop(page * pageSize).take(pageSize), params)
            }
    }

    private fun folder(id: String, title: String, subtitle: String? = null, art: String? = null): MediaItem = MediaItem.Builder().setMediaId(id).setMediaMetadata(
        MediaMetadata.Builder().setTitle(title).setArtist(subtitle).setArtworkUri(art?.let(android.net.Uri::parse))
            .setIsBrowsable(true).setIsPlayable(false).setMediaType(MediaMetadata.MEDIA_TYPE_FOLDER_MIXED).build()
    ).build()

    private suspend fun children(parent: String): List<MediaItem> {
        val lib = flint.library
        val (kind, arg) = parent.substringBefore(':') to parent.substringAfter(':', "")
        return when (kind) {
            "root" -> listOf(folder("albums:RECENT", "Recently played"), folder("albums:NEWEST", "Recently added"), folder("albums:FREQUENT", "Most played"),
                folder("playlists", "Playlists"), folder("starred", "Favourites"), folder("random", "Random"), folder("downloads", "Downloads"))
            "albums" -> lib.albums(AlbumSort.valueOf(arg), size = 40).first().map { folder("album:${it.id}", it.name, it.artist, lib.coverUrl(it.coverArt, 300)) }
            "album" -> lib.albumSongs(arg).map(::item)
            "playlists" -> lib.playlists().first().map { folder("playlist:${it.id}", it.name, "${it.songCount} songs", lib.coverUrl(it.coverArt, 300)) }
            "playlist" -> lib.playlistSongs(arg).map(::item)
            "starred" -> lib.starred().first().songs.map(::item)
            "random" -> lib.randomSongs(50).map(::item)
            "downloads" -> flint.downloads.state.value.done.map(::item)
            else -> emptyList()
        }
    }
}
