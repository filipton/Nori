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
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DefaultAudioSink
import androidx.media3.exoplayer.audio.DefaultAudioTrackBufferSizeProvider
import androidx.media3.exoplayer.DecoderReuseEvaluation
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.extractor.DefaultExtractorsFactory
import androidx.media3.datasource.DataSourceBitmapLoader
import androidx.media3.session.CacheBitmapLoader
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
import dev.flint.music.ffi.PlayQueue
import dev.flint.music.ffi.Song
import dev.flint.music.settings.Prefs
import dev.flint.music.settings.withSound
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
        /** Broadcast inside the package on every track or play-state change; what a home-screen widget listens to. */
        const val ACTION_STATE = "dev.flint.music.STATE"
        const val EXTRA_TITLE = "title"
        const val EXTRA_ARTIST = "artist"
        const val EXTRA_PLAYING = "playing"
        const val ARG_ON = "on"
        const val ARG_MINUTES = "minutes"
        const val ARG_END_OF_TRACK = "endOfTrack"
        const val ARG_SONGS = "songs"
    }

    private lateinit var flint: Flint
    private lateinit var player: ExoPlayer
    private lateinit var session: MediaLibrarySession
    private lateinit var scrobbler: Scrobbler
    private val equalizer = Equalizer()
    private var hiRes = false
    private var burst: BurstSink? = null
    @Suppress("DEPRECATION")
    private val wifiLock by lazy { applicationContext.getSystemService(WifiManager::class.java).createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "flint:loading").apply { setReferenceCounted(false) } }
    private var offloaded = false
    /** Items from the current one onwards, as the playback thread may ask about them (decoding runs ahead). */
    @Volatile private var upcoming: List<MediaItem> = emptyList()
    private val analysisWorker = java.util.concurrent.Executors.newSingleThreadExecutor { Thread(it, "flint-analysis").apply { priority = Thread.MIN_PRIORITY } }
    private lateinit var precacher: Precacher
    private val precache = Runnable { precacheAhead() }
    /** What the volume should be once no fade is running: 1, or the ReplayGain attenuation. */
    private var targetVolume = 1f
    private var fade: Runnable? = null
    private var errorsInARow = 0
    /** Sleep timer "after N songs": transitions still to go. */
    private var sleepAfterSongs = 0
    /** The equalizer screen is open: trade the deep buffer for immediate response. */
    private var tuning = false
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
                        .setEnableFloatOutput(enableFloatOutput).setEnableAudioTrackPlaybackParams(enableAudioTrackPlaybackParams).build()
                ).also { burst = it }, transitions)
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
        player.addListener(listener)
        player.addAnalyticsListener(formats)
        player.addAudioOffloadListener(object : ExoPlayer.AudioOffloadListener {
            override fun onOffloadedPlayback(offloaded: Boolean) { this@PlaybackService.offloaded = offloaded; updateBurst() }
        })

        flint.dac.onChanged = { applyAudio(flint.settings.value); applyGain() }
        flint.dac.start()
        flint.outputs.start()
        // Plugging in headphones or a DAC swaps the whole sound chain, if a profile is bound to it.
        scope.launch {
            flint.outputs.current.collect { output ->
                if (!flint.settings.value.profilePerOutput) return@collect
                val sound = withContext(Dispatchers.IO) { runCatching { flint.core.profileForOutput(output) }.getOrNull() }?.let { dev.flint.music.settings.Sound.fromJson(it.json) } ?: return@collect
                flint.settings.update { it.withSound(sound) }
            }
        }
        applyAudio(flint.settings.value)
        scope.launch {
            var last = flint.settings.value
            flint.settings.prefs.collect { p ->
                if (p.copy(replayGain = last.replayGain, preampDb = last.preampDb, untaggedGainDb = last.untaggedGainDb, scrobblePercent = last.scrobblePercent, listPrefs = last.listPrefs, homeRows = last.homeRows, pinnedPlaylists = last.pinnedPlaylists) != last) applyAudio(p)
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
        setMediaNotificationProvider(
            androidx.media3.session.DefaultMediaNotificationProvider.Builder(this).build()
                .apply { setSmallIcon(dev.flint.music.core.R.drawable.ic_notification) },
        )
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
        precacher.release()
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
            announce()
            scrobbler.onPlaying(isPlaying)
            if (!isPlaying && !player.playWhenReady) persistQueue(push = true)
        }

        override fun onShuffleModeEnabledChanged(on: Boolean) = refreshUpcoming()
        override fun onRepeatModeChanged(mode: Int) = refreshUpcoming()

        override fun onTimelineChanged(timeline: androidx.media3.common.Timeline, reason: Int) {
            refreshUpcoming()
            if (reason == Player.TIMELINE_CHANGE_REASON_PLAYLIST_CHANGED) scheduleSave()
        }

        override fun onPlayerError(error: androidx.media3.common.PlaybackException) {
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

    private val formats = object : AnalyticsListener {
        override fun onAudioInputFormatChanged(t: AnalyticsListener.EventTime, format: Format, reuse: DecoderReuseEvaluation?) {
            // The sink is opened at the source rate, as 16-bit PCM or float; the DAC has to be set to exactly that.
            flint.dac.onFormat(format.sampleRate, if (hiRes) AudioFormat.ENCODING_PCM_FLOAT else AudioFormat.ENCODING_PCM_16BIT)
        }
    }

    private fun updateBurst() { burst?.enabled = !offloaded && !tuning }

    private fun announce() {
        val m = player.currentMediaItem?.mediaMetadata
        sendBroadcast(android.content.Intent(ACTION_STATE).setPackage(packageName)
            .putExtra(EXTRA_TITLE, m?.title?.toString()).putExtra(EXTRA_ARTIST, m?.artist?.toString()).putExtra(EXTRA_PLAYING, player.isPlaying))
    }

    /** A processor joins or leaves the chain, and the buffer depth changes, only when the sink is configured again. */
    private fun reconfigureSink() {
        if (player.playbackState != Player.STATE_IDLE) { player.stop(); player.prepare() }
    }

    /** DSP, crossfade, speed, offload and bit-perfect constrain each other; this is where that is decided. */
    private fun applyAudio(p: Prefs) {
        flint.dac.setEnabled(p.bitPerfect)
        val untouched = hiRes || flint.dac.state.value.bitPerfect
        val processing = p.dsp && !untouched
        equalizer.setChain(if (p.eqEnabled) p.eqBands else emptyList(), p.effectivePreampDb, p.crossfeedDb)
        equalizer.setOutput(p.balance, p.mono, p.limiterThresholdDb, 120f, if (p.limiter) 5f else 0f)
        transitionsOff = untouched
        player.skipSilenceEnabled = p.skipSilence && !untouched
        player.playbackParameters = androidx.media3.common.PlaybackParameters(p.speed, p.pitch)
        // Offload hands the compressed stream to the audio chip, so it is only possible while the app needs no samples.
        val offload = p.offload && !processing && p.crossfadeSec == 0 && !p.autoMix && !p.skipSilence && p.speed == 1f && p.pitch == 1f
        player.trackSelectionParameters = player.trackSelectionParameters.buildUpon().setAudioOffloadPreferences(
            AudioOffloadPreferences.Builder()
                .setAudioOffloadMode(if (offload) AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_ENABLED else AudioOffloadPreferences.AUDIO_OFFLOAD_MODE_DISABLED)
                .setIsGaplessSupportRequired(true).build()
        ).build()
        if (equalizer.enabled != processing) {
            equalizer.enabled = processing
            reconfigureSink()
        }
    }

    @Volatile private var transitionsOff = false

    /** Mirrors the player's shuffle flag for the audio thread, which may not ask the player itself. */
    @Volatile private var shuffling = false

    private fun refreshUpcoming() {
        shuffling = player.shuffleModeEnabled
        val t = player.currentTimeline
        if (t.isEmpty || player.currentMediaItemIndex == C.INDEX_UNSET) { upcoming = emptyList(); return }
        val list = ArrayList<MediaItem>(8)
        var i = player.currentMediaItemIndex
        while (i != C.INDEX_UNSET && list.size < 8) {
            list += player.getMediaItemAt(i)
            i = t.getNextWindowIndex(i, player.repeatMode, player.shuffleModeEnabled)
        }
        upcoming = list
    }

    /**
     * Plans each transition when the sink reaches a track: from the stored analyses (AutoMix) or, without them,
     * a plain crossfade. The Rust planner decides; this only gathers its inputs.
     */
    private val transitions = object : TransitionSink.Listener {
        override fun planFor(outgoingId: String): TransitionSink.Plan? {
            val p = flint.settings.value
            if (transitionsOff || (!p.autoMix && p.crossfadeSec == 0)) return null
            val order = upcoming
            val at = order.indexOfFirst { it.mediaId == outgoingId }.takeIf { it >= 0 } ?: return null
            val out = order[at]
            val next = order.getOrNull(at + 1) ?: return null
            if (out.isRadio || next.isRadio) return null
            val settings = dev.flint.music.ffi.AutoMixSettings(
                maxTransitionS = (if (p.autoMix) p.autoMixMaxS else p.crossfadeSec).toFloat(),
                beatMatch = p.autoMix && p.autoMixBeatMatch, maxTempoChangePct = p.autoMixMaxTempoPct,
                bassSwap = p.autoMix && p.autoMixBassSwap, filterEffects = p.autoMix && p.autoMixFilters,
                keepPitch = p.autoMixKeepPitch, sameAlbumInOrder = p.crossfadeKeepAlbums && followsOnAlbum(out, next),
                matchLoudness = false,
            )
            val a = if (p.autoMix) runCatching { flint.core.analysisGet(out.mediaId) }.getOrNull() else null
            val b = if (p.autoMix) runCatching { flint.core.analysisGet(next.mediaId) }.getOrNull() else null
            val outMs = (out.mediaMetadata.durationMs ?: 0L)
            val inMs = (next.mediaMetadata.durationMs ?: 0L)
            if (outMs <= 0 || inMs <= 0) return null
            val plan = dev.flint.music.ffi.planTransition(a, b, outMs, inMs, settings)
            if (plan.kind == dev.flint.music.ffi.TransitionKind.GAPLESS) return null
            android.util.Log.i("flint", "transition ${out.mediaMetadata.title} -> ${next.mediaMetadata.title}: ${plan.kind} ${plan.durationMs} ms at ${plan.outStartMs}, tempo x${"%.3f".format(plan.tempoRatio)} (${plan.reason})")
            return TransitionSink.Plan(
                incomingId = next.mediaId, outStartUs = plan.outStartMs * 1000, durationUs = plan.durationMs * 1000, inSkipUs = plan.inStartMs * 1000,
                mixer = dev.flint.music.ffi.automixMixerParams(plan).toFloatArray(), tempoRatio = plan.tempoRatio.toFloat(),
                keepPitch = plan.keepPitch, rampUs = plan.tempoRampMs * 1000,
            )
        }

        override fun wantsAnalysis(songId: String): Boolean {
            if (!flint.settings.value.autoMix || songId.startsWith(RADIO_PREFIX) || songId.startsWith("ext-")) return false
            return runCatching { flint.core.analysisGet(songId) == null }.getOrDefault(false)
        }

        override fun analysed(songId: String, handle: Long, frames: Long, sampleRate: Int) {
            analysisWorker.execute {
                try {
                    // Only a track heard from start to end: the duration has to agree with what the server says.
                    val expectedMs = upcoming.firstOrNull { it.mediaId == songId }?.mediaMetadata?.durationMs ?: 0L
                    val heardMs = frames * 1000 / sampleRate.coerceAtLeast(1)
                    if (expectedMs <= 0 || kotlin.math.abs(heardMs - expectedMs) <= 3000) {
                        val a = flint.core.analysisFinishStream(songId, handle)
                        android.util.Log.i("flint", "analysed $songId: ${a?.let { "%.2f bpm (%.2f), key %s, heard %d ms of %d ms, %d frames at %d Hz".format(it.bpm, it.bpmConfidence, dev.flint.music.ffi.automixKeyName(it.key), it.durationMs, expectedMs, frames, sampleRate) } ?: "too short"}")
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

        override fun seekTo(positionMs: Long) = softly { super.seekTo(positionMs) }
        override fun seekTo(mediaItemIndex: Int, positionMs: Long) = softly { super.seekTo(mediaItemIndex, positionMs) }
        override fun seekToNext() = softly { super.seekToNext() }
        override fun seekToNextMediaItem() = softly { super.seekToNextMediaItem() }
        override fun seekToPreviousMediaItem() = softly { super.seekToPreviousMediaItem() }
        override fun seekToPrevious() = softly { if (flint.settings.value.previousAlwaysSkips && hasPreviousMediaItem()) super.seekToPreviousMediaItem() else super.seekToPrevious() }
    }

    private fun precacheAhead() {
        val p = flint.settings.value
        val count = if (flint.http.metered) p.precacheMobile else p.precacheWifi
        // The player itself already buffers the very next track; this covers the ones after it.
        val from = player.currentMediaItemIndex + 2
        if (count <= 1 || player.shuffleModeEnabled) return precacher.cancel()
        precacher.update((from until minOf(from + count - 1, player.mediaItemCount)).map(player::getMediaItemAt))
    }

    /**
     * Keeps the music going past the end of the queue. Runs once, when the last
     * song starts: the radio is up for that song anyway. Only library songs come
     * back from getSimilarSongs2, so this never makes octo-fiesta download anything.
     */
    private fun autoFill(item: MediaItem?) {
        if (item == null || item.isRadio || player.hasNextMediaItem() || player.repeatMode != Player.REPEAT_MODE_OFF || !flint.settings.value.autoFill) return
        val seed = item.toSong()
        if (seed.isExternal) return
        scope.launch {
            val more = runCatching { flint.library.similarSongs(seed.id, 25) }.getOrNull().orEmpty()
            val queued = (0 until player.mediaItemCount).mapTo(HashSet()) { player.getMediaItemAt(it).mediaId }
            val fresh = more.filter { it.id !in queued && !it.isExternal }.take(15)
            if (fresh.isNotEmpty() && !player.hasNextMediaItem()) player.addMediaItems(fresh.map(::item))
        }
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

    private fun item(s: Song): MediaItem = s.toMediaItem(flint.library.coverUrl(s.coverArt, 512))

    // ---- session: custom commands, Android Auto browsing, voice search ----

    private inner class Callback : MediaLibrarySession.Callback {
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
            val commands = MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS.buildUpon().add(SessionCommand(CMD_SLEEP, Bundle.EMPTY)).add(SessionCommand(CMD_TUNING, Bundle.EMPTY)).build()
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
            if (command.customAction == CMD_TUNING && tuning != args.getBoolean(ARG_ON)) {
                tuning = args.getBoolean(ARG_ON)
                updateBurst()
                reconfigureSink()
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
