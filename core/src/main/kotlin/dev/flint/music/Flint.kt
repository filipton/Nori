package dev.flint.music

import android.content.Context
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import dev.flint.music.data.Library
import dev.flint.music.downloads.Downloads
import dev.flint.music.ffi.Core
import dev.flint.music.net.Http
import dev.flint.music.playback.BitPerfect
import dev.flint.music.playback.MediaSources
import dev.flint.music.playback.PlayerConnection
import dev.flint.music.settings.Settings
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The object graph, built by hand: there are a dozen long-lived objects and a DI
 * framework would cost more at startup than it saves. The UI reaches everything
 * through here; nothing in this module knows a UI exists.
 */
@OptIn(UnstableApi::class)
class Flint private constructor(context: Context) {
    val settings = Settings(context)

    // Everything below is built on first use. The application warms it up from a background thread, so by the
    // time anything needs the core it is normally there; the UI thread itself only ever needs [settings] and
    // the cheap shells ([library], [downloads], [player]) to draw its first frame.
    private val lazyCore = lazy {
        Core(File(context.filesDir, "flint.db").path).also { c -> settings.value.let { if (it.loggedIn) c.configure(it.serverUrl, it.user, it.password) } }
    }
    private val lazyHttp = lazy { Http(context) }
    private val lazySources = lazy { MediaSources(context, core, http, settings) }
    val core: Core by lazyCore
    val http: Http by lazyHttp
    val sources: MediaSources by lazySources
    val library = Library(lazyCore, lazyHttp)
    val downloads = Downloads(context, lazyCore, lazySources)
    val dac = BitPerfect(context)
    val player = PlayerConnection(context, this)

    /** Called off the main thread at process start. */
    fun warmUp() {
        val t = android.os.SystemClock.elapsedRealtime()
        core; http; sources
        android.util.Log.i("flint", "core ready in ${android.os.SystemClock.elapsedRealtime() - t} ms")
    }

    /** Checks the credentials against the server before keeping them. */
    suspend fun login(url: String, user: String, password: String) {
        val old = settings.value
        val base = withContext(Dispatchers.IO) { core.configure(url, user, password) }
        try {
            library.ping()
        } catch (e: Exception) {
            if (old.loggedIn) withContext(Dispatchers.IO) { core.configure(old.serverUrl, old.user, old.password) }
            throw e
        }
        settings.update { it.copy(serverUrl = base, user = user, password = password) }
    }

    fun logout() {
        player.clear()
        settings.update { it.copy(serverUrl = "", user = "", password = "") }
    }

    companion object {
        @Volatile private var instance: Flint? = null
        fun get(context: Context): Flint = instance ?: synchronized(this) { instance ?: Flint(context.applicationContext).also { instance = it } }
    }
}
