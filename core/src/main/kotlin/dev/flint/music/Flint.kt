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

/**
 * The object graph, built by hand: there are a dozen long-lived objects and a DI
 * framework would cost more at startup than it saves. The UI reaches everything
 * through here; nothing in this module knows a UI exists.
 */
@OptIn(UnstableApi::class)
class Flint private constructor(context: Context) {
    val settings = Settings(context)
    val core = Core(File(context.filesDir, "flint.db").path)
    val http = Http(context)
    val library = Library(core, http)
    val sources = MediaSources(context, core, http, settings)
    val downloads = Downloads(context, core, sources)
    val dac = BitPerfect(context)
    val player = PlayerConnection(context, this)

    init { settings.value.let { if (it.loggedIn) core.configure(it.serverUrl, it.user, it.password) } }

    /** Checks the credentials against the server before keeping them. */
    suspend fun login(url: String, user: String, password: String) {
        val old = settings.value
        val base = core.configure(url, user, password)
        try {
            library.ping()
        } catch (e: Exception) {
            if (old.loggedIn) core.configure(old.serverUrl, old.user, old.password)
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
