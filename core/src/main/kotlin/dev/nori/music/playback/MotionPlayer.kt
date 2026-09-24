package dev.nori.music.playback

import android.content.Context
import android.view.TextureView
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.hls.HlsMediaSource
import dev.nori.music.net.Http
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The moving cover's own player: muted, looping, video only, and nothing to do with the music. It asks
 * for no audio focus, has no media session and no wake lock, opens no audio track (the audio track type
 * is switched off, so no audio renderer is ever enabled), and is its own ExoPlayer, so it cannot touch
 * the one in PlaybackService.
 *
 * The ExoPlayer is built by the first [play] and let go of by [release]; the screen releases it when
 * the player is put away, when the app is left or the screen goes off, and when the switch goes off.
 * Main thread only, like every media3 player.
 *
 * Bytes come through the app's one OkHttp pool and the moving-cover cache (MediaSources.motionCache),
 * which matters more than it looks: a loop is the same few seconds over and over, and without the cache
 * every pass would fetch them again.
 */
@OptIn(UnstableApi::class)
class MotionPlayer(
    private val context: Context,
    private val http: Http,
    private val sources: MediaSources,
    /** A video answered with an HTTP error: it has gone, and the album should be looked up again. */
    private val onGone: (String) -> Unit,
) {
    private var player: ExoPlayer? = null
    private var view: TextureView? = null
    private var loaded: String? = null

    private val _ready = MutableStateFlow<String?>(null)

    /** The video whose first frame is on the surface now, or null. The screen fades in only after this. */
    val ready: StateFlow<String?> = _ready.asStateFlow()

    /**
     * The cache is only built on the playback thread, when the first bytes are wanted: it does a little
     * disk work, and the screen that calls [play] must not wait for it.
     */
    private val cached: DataSource.Factory by lazy {
        CacheDataSource.Factory()
            .setCache(sources.motionCache)
            .setUpstreamDataSourceFactory(OkHttpDataSource.Factory(http.callFactory))
            .setFlags(CacheDataSource.FLAG_IGNORE_CACHE_ON_ERROR)
    }
    private val bytes = DataSource.Factory { cached.createDataSource() }

    private val listener = object : Player.Listener {
        override fun onRenderedFirstFrame() {
            _ready.value = loaded
        }

        override fun onPlayerError(error: PlaybackException) {
            android.util.Log.w("nori", "motion artwork: ${error.errorCodeName}")
            val gone = loaded
            // Asked again on the next rest rather than never: the screen calls play() when it wants it.
            loaded = null
            if (error.errorCode == PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS && gone != null) onGone(gone)
        }
    }

    /** Where the video is drawn. The screen hands its surface over when it is composed. */
    fun show(target: TextureView) {
        if (target === view) return
        view?.let { player?.clearVideoTextureView(it) }
        view = target
        // A new surface has nothing on it until a frame is rendered there.
        _ready.value = null
        player?.setVideoTextureView(target)
    }

    /** [target] has left the screen; it is no longer drawn into, and not held on to. */
    fun hide(target: TextureView) {
        if (target !== view) return
        player?.clearVideoTextureView(target)
        view = null
        _ready.value = null
    }

    /** Plays [url] on a loop, from where it was if it is the one already loaded. */
    fun play(url: String) {
        val p = player ?: build().also { player = it }
        if (url != loaded || p.playbackState == Player.STATE_IDLE) {
            loaded = url
            _ready.value = null
            p.setMediaSource(HlsMediaSource.Factory(bytes).createMediaSource(MediaItem.fromUri(url)))
            p.prepare()
        }
        p.play()
    }

    /** Holds the frame on screen; nothing decodes while paused. */
    fun pause() {
        player?.pause()
    }

    /** Lets the ExoPlayer and its decoder go. The next [play] builds a new one. */
    fun release() {
        val p = player ?: return
        p.removeListener(listener)
        p.release()
        player = null
        loaded = null
        _ready.value = null
        live--
    }

    private fun build(): ExoPlayer = ExoPlayer.Builder(context).build().apply {
        // No sound at all: nothing is decoded for the audio track, and the volume is nought besides.
        volume = 0f
        repeatMode = Player.REPEAT_MODE_ONE
        trackSelectionParameters = trackSelectionParameters.buildUpon()
            .setTrackTypeDisabled(C.TRACK_TYPE_AUDIO, true)
            // The still cover is 800 px; a video larger than this is only more bytes and more decoding.
            // One fixed choice, not an adaptive one, so every loop and every visit is the same rendition
            // and comes out of the cache.
            .setMaxVideoSize(MAX_SIDE, MAX_SIDE)
            .setForceHighestSupportedBitrate(true)
            .build()
        addListener(listener)
        view?.let { setVideoTextureView(it) }
        live++
    }

    companion object {
        private const val MAX_SIDE = 1080

        /** How many of these players exist, for the test bridge: nought whenever the switch is off. */
        @Volatile var live = 0
            internal set
    }
}
