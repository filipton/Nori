package dev.flint.music.app.vm

import android.app.Application
import dev.flint.music.ffi.Lyrics
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

    /** Lyrics of whatever is playing; fetched only while a lyrics view is collecting. */
    @OptIn(ExperimentalCoroutinesApi::class)
    val lyrics: StateFlow<Load<Lyrics>> = state.map { it.current?.id }.distinctUntilChanged()
        .flatMapLatest { if (it == null) flowOf(Lyrics(false, emptyList())) else flint.library.lyrics(it) }.asLoad()

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
}
