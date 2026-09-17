package dev.flint.music

import android.content.Context
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import dev.flint.music.data.Library
import dev.flint.music.downloads.Downloads
import dev.flint.music.ffi.Core
import dev.flint.music.ffi.CoreException
import dev.flint.music.ffi.ServerConfig
import dev.flint.music.net.Http
import dev.flint.music.playback.BitPerfect
import dev.flint.music.playback.MediaSources
import dev.flint.music.playback.PlayerConnection
import dev.flint.music.settings.ServerProfile
import dev.flint.music.settings.Settings
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/**
 * The object graph, built by hand: there are a dozen long-lived objects and a DI
 * framework would cost more at startup than it saves. The UI reaches everything
 * through here; nothing in this module knows a UI exists.
 */
@OptIn(UnstableApi::class)
class Flint private constructor(private val context: Context) {
    val settings = Settings(context)

    // Everything below is built on first use. The application warms it up from a background thread, so by the
    // time anything needs the core it is normally there; the UI thread itself only ever needs [settings] and
    // the cheap shells ([library], [downloads], [player]) to draw its first frame.
    private val lock = Any()
    @Volatile private var opened: Pair<String, Core>? = null
    private val lazyHttp = lazy { Http(context).also { it.configure(settings.value.server) } }
    private val lazySources = lazy { MediaSources(context, ::core, http, settings) { onSecondAddress } }

    /** The index of the active server profile; every profile has its own file. */
    val core: Core
        get() {
            val id = settings.value.activeServerId.ifEmpty { "default" }
            opened?.takeIf { it.first == id }?.let { return it.second }
            return synchronized(lock) {
                opened?.takeIf { it.first == id }?.second ?: open(id, settings.value.server).also { opened = id to it }
            }
        }

    val http: Http by lazyHttp
    val sources: MediaSources by lazySources
    val library = Library(::core, { http }, { settings.value.server?.musicFolderId.orEmpty() }, ::chooseAddress)
    val downloads = Downloads(context, ::core, lazySources)
    val dac = BitPerfect(context)
    val player = PlayerConnection(context, this)

    /** True while requests go to the profile's second address; stream quality is capped then. */
    @Volatile var onSecondAddress = false
        private set

    private fun open(id: String, profile: ServerProfile?): Core =
        Core(File(context.filesDir, if (id == "default") "flint.db" else "flint-$id.db").path).also { c -> profile?.let { c.configure(it.config()) } }

    private fun ServerProfile.config() = ServerConfig(url, user, password, apiKey.ifEmpty { null }, legacyAuth)

    /** Called off the main thread at process start. */
    fun warmUp() {
        val t = android.os.SystemClock.elapsedRealtime()
        core; http; sources
        android.util.Log.i("flint", "core ready in ${android.os.SystemClock.elapsedRealtime() - t} ms")
    }

    /**
     * A profile with two addresses: ask the first one, briefly; if it does not answer use the second.
     * Runs when the app comes to the foreground and after a request failed, never on a timer.
     * Returns true when the address in use changed.
     */
    suspend fun chooseAddress(): Boolean = withContext(Dispatchers.IO) {
        val p = settings.value.server ?: return@withContext false
        if (p.altUrl.isBlank()) return@withContext false
        val c = core
        c.useAddress(p.url)
        val firstAnswers = runCatching { c.parseStatus(http.get(c.url("ping", emptyList()), timeoutMs = 2500)) }.isSuccess
        if (!firstAnswers) c.useAddress(p.altUrl)
        val changed = onSecondAddress == firstAnswers
        onSecondAddress = !firstAnswers
        if (changed) library.onServerChanged()
        changed
    }

    /**
     * Checks the profile against the server before keeping it. Servers without token auth say so
     * (error 41) and are retried with legacy auth, which is then remembered.
     */
    suspend fun login(draft: ServerProfile): ServerProfile = withContext(Dispatchers.IO) {
        suspend fun attempt(p: ServerProfile): ServerProfile {
            http.configure(p)
            val probe = open(p.id, null)
            try {
                probe.configure(p.config())
                val first = runCatching { probe.parseStatus(http.get(probe.url("ping", emptyList()))) }
                if (first.isFailure && p.altUrl.isNotBlank()) {
                    probe.useAddress(p.altUrl)
                    probe.parseStatus(http.get(probe.url("ping", emptyList())))
                } else first.getOrThrow()
            } finally {
                probe.close()
            }
            return p
        }
        val old = settings.value.server
        val accepted = try {
            try {
                attempt(draft)
            } catch (e: CoreException.Api) {
                if (e.code == 41 && !draft.legacyAuth && draft.apiKey.isEmpty()) attempt(draft.copy(legacyAuth = true)) else throw e
            }
        } catch (e: Exception) {
            http.configure(old)
            throw e
        }
        activate(accepted)
        accepted
    }

    /** Makes [profile] the active server (adding or replacing it in the saved list). */
    fun activate(profile: ServerProfile) {
        player.clear()
        // The old core is dropped, not closed: a request may still be using it, and the cleaner frees it.
        synchronized(lock) { opened = null }
        settings.update { p -> p.copy(servers = p.servers.filterNot { it.id == profile.id } + profile, activeServerId = profile.id) }
        http.configure(profile)
        onSecondAddress = false
        library.onServerChanged()
    }

    /** Settings that do not need the server asked again: headers, Wi-Fi only, music folder, name. */
    fun updateServer(profile: ServerProfile) {
        settings.update { p -> p.copy(servers = p.servers.map { if (it.id == profile.id) profile else it }) }
        if (profile.id == settings.value.activeServerId) { http.configure(profile); library.onServerChanged() }
    }

    fun removeServer(id: String) {
        val wasActive = settings.value.activeServerId == id
        if (wasActive) { player.clear(); synchronized(lock) { opened = null } }
        settings.update { p ->
            val rest = p.servers.filterNot { it.id == id }
            p.copy(servers = rest, activeServerId = if (wasActive) rest.firstOrNull()?.id.orEmpty() else p.activeServerId)
        }
        File(context.filesDir, if (id == "default") "flint.db" else "flint-$id.db").let { f -> listOf("", "-wal", "-shm").forEach { File(f.path + it).delete() } }
        if (wasActive) { http.configure(settings.value.server); library.onServerChanged() }
    }

    fun logout() = settings.value.server?.let { removeServer(it.id) }

    companion object {
        @Volatile private var instance: Flint? = null
        fun get(context: Context): Flint = instance ?: synchronized(this) { instance ?: Flint(context.applicationContext).also { instance = it } }
    }
}
