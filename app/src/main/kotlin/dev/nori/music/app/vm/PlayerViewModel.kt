package dev.nori.music.app.vm

import android.app.Application
import android.media.AudioManager
import dev.nori.music.ffi.Lyrics
import dev.nori.music.data.FoundLyrics
import dev.nori.music.data.LyricsSource
import dev.nori.music.playback.PlayerState
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.stateIn
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.onStart

class PlayerViewModel(app: Application) : NoriViewModel(app) {
    private val player = nori.player
    val state: StateFlow<PlayerState> = player.state
    /** Where a seek asked to go, while it is still being watched into place; the seek bar holds this. */
    val pendingSeek: StateFlow<Long?> = player.pendingSeek

    /** Just the id, so a list can highlight its playing row without observing the whole player. */
    val currentId: StateFlow<String?> = state.map { it.current?.id }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /** Just the play/pause flag, for the same reason: the marked row's bars move only while it sounds. */
    val sounding: StateFlow<Boolean> = state.map { it.playing }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), false)

    /**
     * Lyrics of whatever is playing; fetched only while a lyrics view is collecting. Each song starts
     * from Loading, so the view shows its loader and then the new words, instead of holding the last
     * song's lyrics on screen while the next ones are fetched.
     */
    @OptIn(ExperimentalCoroutinesApi::class)
    val lyrics: StateFlow<Load<FoundLyrics>> = state.map { it.current }.distinctUntilChanged { a, b -> a?.id == b?.id }
        .flatMapLatest { song ->
            val p = nori.settings.value
            val found = if (song == null) flowOf(FoundLyrics(Lyrics(synced = false, wordTimed = false, lines = emptyList()), LyricsSource.SERVER))
            else nori.library.lyricsFor(song, p.thirdPartyLookups && p.lyricsLrclib)
            found.map<FoundLyrics, Load<FoundLyrics>> { Load.Ready(it) }
                .onStart { emit(Load.Loading) }
                .catch { emit(Load.Failed(it.message ?: it.javaClass.simpleName)) }
        }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), Load.Loading)

    init {
        // The artwork either side of what is playing, fetched before it is asked for. A skip used to
        // show an empty sleeve for as long as the server took to render the next cover - on a slow one,
        // seconds. Same sizes and requests as the player and the rows, so a warmed cover is a cache hit.
        // How far ahead is the user's (Settings, "Covers fetched ahead"); already-cached ones cost a
        // memory lookup and nothing else.
        viewModelScope.launch {
            kotlinx.coroutines.flow.combine(state, nori.settings.prefs.map { it.coversAhead }.distinctUntilChanged()) { s, ahead ->
                // Both neighbours first, as a skip would reach them (shuffle included), and then outwards
                // in both directions a step at a time. Backwards as well as forwards: going back through
                // a queue is as ordinary as going on, and with only the one song behind warmed, the
                // second swipe back always waited on the server.
                val out = ArrayList<Int>()
                out += s.previousIndex
                out += s.nextIndex
                for (d in 2..ahead) { out += s.index + d; out += s.index - d }
                out.take(if (ahead == 0) 1 else ahead * 2)
                    .distinct().filter { it != s.index }.mapNotNull { s.queue.getOrNull(it)?.coverArt }
            }.distinctUntilChanged()
                .collect { arts ->
                    val context = getApplication<Application>()
                    val loader = coil3.SingletonImageLoader.get(context)
                    arts.filterNot { it.startsWith("ext-") || it.startsWith("pl-") }.forEach { art ->
                        for (size in intArrayOf(320, 800)) {
                            loader.enqueue(coil3.request.ImageRequest.Builder(context).data(nori.library.coverUrl(art, size)).size(size).build())
                        }
                    }
                }
        }
    }

    /** Pull, do not push: the UI reads this on its own clock while the seek bar is on screen. */
    val positionMs: Long get() = player.positionMs

    fun connect() = player.connect()
    fun toggle() = player.toggle()
    fun next() = player.next()
    fun previous() = player.previous()
    fun previousItem() = player.previousItem()
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
    /**
     * The music stream's volume, pushed the moment it changes. It used to be read once a second by the
     * player screen, so a press of the volume keys took up to a second to show on the slider, and the
     * screen ticked for as long as it was open. Android announces every change with a broadcast - the
     * one ExoPlayer's own volume tracking listens to - and this listens only while the screen collects
     * it: nothing registered, and nothing running, once the player is closed.
     */
    val volume: StateFlow<Float> = kotlinx.coroutines.flow.callbackFlow {
        val context = getApplication<Application>()
        trySend(volumeFraction())
        val receiver = object : android.content.BroadcastReceiver() {
            override fun onReceive(c: android.content.Context, intent: android.content.Intent) {
                val stream = intent.getIntExtra("android.media.EXTRA_VOLUME_STREAM_TYPE", AudioManager.STREAM_MUSIC)
                if (stream == AudioManager.STREAM_MUSIC) trySend(volumeFraction())
            }
        }
        androidx.core.content.ContextCompat.registerReceiver(
            context, receiver, android.content.IntentFilter("android.media.VOLUME_CHANGED_ACTION"),
            androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        // That broadcast is not public API, and a hardened or future Android may stop delivering it. The
        // system also writes every volume to its settings store half a second or so later, which any app
        // may watch; so if the broadcast never comes, the slider is late rather than frozen.
        val observer = object : android.database.ContentObserver(android.os.Handler(android.os.Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) { trySend(volumeFraction()) }
        }
        context.contentResolver.registerContentObserver(android.provider.Settings.System.CONTENT_URI, true, observer)
        awaitClose {
            context.unregisterReceiver(receiver)
            context.contentResolver.unregisterContentObserver(observer)
        }
    }.distinctUntilChanged().stateIn(viewModelScope, SharingStarted.WhileSubscribed(1_000), volumeFraction())

    fun volumeFraction(): Float {
        val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC).takeIf { it > 0 } ?: return 0f
        return audio.getStreamVolume(AudioManager.STREAM_MUSIC) / max.toFloat()
    }
    fun setVolumeFraction(f: Float) {
        val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC).takeIf { it > 0 } ?: return
        // Nearest step, not the one below: truncating made the bar jump back a notch every time it was let go.
        audio.setStreamVolume(AudioManager.STREAM_MUSIC, kotlin.math.round(f.coerceIn(0f, 1f) * max).toInt().coerceIn(0, max), 0)
    }
}
