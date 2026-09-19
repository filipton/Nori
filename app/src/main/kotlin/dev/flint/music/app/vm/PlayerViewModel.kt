package dev.flint.music.app.vm

import android.app.Application
import android.media.AudioManager
import dev.flint.music.ffi.Lyrics
import dev.flint.music.data.FoundLyrics
import dev.flint.music.data.LyricsSource
import dev.flint.music.playback.PlayerState
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.stateIn
import androidx.lifecycle.viewModelScope

class PlayerViewModel(app: Application) : FlintViewModel(app) {
    private val player = flint.player
    val state: StateFlow<PlayerState> = player.state

    /** Just the id, so a list can highlight its playing row without observing the whole player. */
    val currentId: StateFlow<String?> = state.map { it.current?.id }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /** Just the play/pause flag, for the same reason: the marked row's bars move only while it sounds. */
    val sounding: StateFlow<Boolean> = state.map { it.playing }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), false)

    /** Lyrics of whatever is playing; fetched only while a lyrics view is collecting. */
    @OptIn(ExperimentalCoroutinesApi::class)
    val lyrics: StateFlow<Load<FoundLyrics>> = state.map { it.current }.distinctUntilChanged { a, b -> a?.id == b?.id }
        .flatMapLatest { song ->
            val p = flint.settings.value
            if (song == null) flowOf(FoundLyrics(Lyrics(synced = false, wordTimed = false, lines = emptyList()), LyricsSource.SERVER))
            else flint.library.lyricsFor(song, p.thirdPartyLookups && p.lyricsLrclib)
        }.asLoad()

    /** Pull, do not push: the UI reads this on its own clock while the seek bar is on screen. */
    val positionMs: Long get() = player.positionMs

    fun connect() = player.connect()
    fun toggle() = player.toggle()
    fun next() = player.next()
    fun previous() = player.previous()
    fun seekTo(ms: Long) = player.seekTo(ms)
    fun skipTo(index: Int) = player.skipTo(index)
    fun remove(index: Int) = player.remove(index)
    fun move(from: Int, to: Int) = player.move(from, to)
    fun clearQueue() = player.clear()
    fun toggleShuffle() = player.setShuffle(!state.value.shuffle)
    fun cycleRepeat() = player.cycleRepeat()
    fun sleep(minutes: Int, endOfTrack: Boolean = false, songs: Int = 0) = player.sleep(minutes, endOfTrack, songs)

    /**
     * The phone's music-stream volume, for the player's volume slider. Read live (hardware keys can
     * move it under us) and written without flags, so dragging it never pops a system UI over the art.
     */
    private val audio = app.getSystemService(AudioManager::class.java)
    fun volumeFraction(): Float {
        val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC).takeIf { it > 0 } ?: return 0f
        return audio.getStreamVolume(AudioManager.STREAM_MUSIC) / max.toFloat()
    }
    fun setVolumeFraction(f: Float) {
        val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC).takeIf { it > 0 } ?: return
        audio.setStreamVolume(AudioManager.STREAM_MUSIC, (f.coerceIn(0f, 1f) * max).toInt().coerceIn(0, max), 0)
    }
}
