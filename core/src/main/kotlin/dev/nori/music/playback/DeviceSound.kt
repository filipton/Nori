package dev.nori.music.playback

import android.content.Context
import dev.nori.music.ffi.AutoEqEntry
import dev.nori.music.ffi.Core
import dev.nori.music.ffi.SoundProfile
import dev.nori.music.ffi.parseEqPreset
import dev.nori.music.net.Http
import dev.nori.music.settings.Band
import dev.nori.music.settings.BandKind
import dev.nori.music.settings.Settings
import dev.nori.music.settings.Sound
import dev.nori.music.settings.withSound
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * Which sound each output device gets. A device can be given a saved profile, a flat sound, an AutoEQ
 * curve, or nothing; when it becomes the active output its sound is loaded, and when music goes back
 * to a device with nothing chosen the sound from before comes back. Headphones with nothing chosen and
 * a curve in the AutoEQ list get it offered, or applied straight away when [autoEqAuto] is on.
 *
 * Driven by [Outputs.current] from the playback service, so it works with the app's screens closed.
 * It adds no listener of its own: it runs once per device change, never while music plays.
 */
class DeviceSound(context: Context, private val settings: Settings, private val core: () -> Core, private val http: () -> Http) {
    private val store by lazy { context.getSharedPreferences("nori-devices", Context.MODE_PRIVATE) }
    private val lock = Mutex()

    /** Something to tell the user about the device that just connected. */
    sealed interface Notice { val output: String }
    /** Nothing is chosen for [output]; [entry] looks like it. Asking is the default. */
    data class Offer(override val output: String, val entry: AutoEqEntry) : Notice
    /** [curve] was applied and remembered for [output] without asking; [before] is what undo puts back. */
    data class Applied(override val output: String, val curve: String, val before: Sound, val created: Boolean) : Notice

    private val _notice = MutableStateFlow<Notice?>(null)
    val notice: StateFlow<Notice?> = _notice
    fun consume(n: Notice) { _notice.compareAndSet(n, null) }
    /** The last notice raised, seen or not: the test bridge answers it after the snackbar is gone. */
    @Volatile var lastNotice: Notice? = null
        private set
    private fun post(n: Notice) { lastNotice = n; _notice.value = n }

    private val _profiles = MutableStateFlow<List<SoundProfile>>(emptyList())
    /** The saved profiles with the devices each is bound to. Filled by [refresh]. */
    val profiles: StateFlow<List<SoundProfile>> = _profiles

    private val _quiet by lazy { MutableStateFlow(store.getStringSet("quiet", null).orEmpty()) }
    /** Devices the user said should never be offered a curve. */
    val quiet: StateFlow<Set<String>> get() = _quiet

    suspend fun refresh() {
        _profiles.value = io { runCatching { core().profiles() }.getOrDefault(emptyList()) }
    }

    /** The output that music now goes to. */
    suspend fun onOutput(output: String): Unit = lock.withLock { arrive(output) }

    private suspend fun arrive(output: String) {
        // A notice is about the device that just arrived; one left unseen for an earlier device is stale.
        _notice.value = null
        val p = settings.value
        val bound = io { runCatching { core().profileForOutput(output) }.getOrNull() }
        android.util.Log.i("nori", "device sound: $output -> ${bound?.name ?: "nothing chosen"}")
        if (bound != null) {
            if (p.profilePerOutput) Sound.fromJson(bound.json)?.let(::load)
            return
        }
        if (p.profilePerOutput) restore()
        if (output == Outputs.SPEAKER || output in quiet.value) return
        val entry = curvesFor(output).firstOrNull() ?: return
        if (!(p.autoEqAuto && p.profilePerOutput)) {
            post(Offer(output, entry))
            return
        }
        val before = Sound.of(settings.value)
        val created = runCatching { adopt(output, entry) }.getOrElse {
            // No network, or GitHub not answering: asking later is better than silently doing nothing.
            android.util.Log.w("nori", "autoeq for $output: ${it.message}")
            post(Offer(output, entry))
            return
        }
        android.util.Log.i("nori", "device sound: applied AutoEQ ${entry.name} to $output")
        post(Applied(output, entry.name, before, created))
    }

    /** The AutoEQ curves this output's own name points at, best first. Empty for the speaker, a nameless DAC, or no index. */
    suspend fun curvesFor(output: String, limit: Int = 5): List<AutoEqEntry> {
        val name = output.substringAfter(": ", "")
        if (name.isEmpty()) return emptyList()
        return io { runCatching { core().autoeqForDevice(name, limit.toUInt()) }.getOrDefault(emptyList()) }
    }

    /** Yes to an [Offer]. */
    suspend fun accept(offer: Offer) { lock.withLock { adopt(offer.output, offer.entry) } }

    /** Undoes an [Applied]: the sound from before, nothing bound, and this device is not offered a curve again. */
    suspend fun undo(n: Applied): Unit = lock.withLock {
        io {
            runCatching {
                val c = core()
                c.profileBind(n.output, null)
                if (n.created && c.profiles().any { it.name == n.curve && it.outputs.isEmpty() }) c.profileDelete(n.curve)
            }
        }
        store.edit().remove(LOOSE).apply()
        settings.update { it.withSound(n.before) }
        setQuiet(n.output, true)
        refresh()
    }

    /** What the user picked for a device in the equalizer's device list. */
    sealed interface Choice {
        /** Nothing chosen: a matching AutoEQ curve is offered (or applied, with the setting on). */
        data object Automatic : Choice
        /** Nothing chosen and nothing offered. */
        data object Quiet : Choice
        /** The equalizer off on this device, everything else as it is now. */
        data object Flat : Choice
        data class Profile(val name: String) : Choice
        data class Curve(val entry: AutoEqEntry) : Choice
    }

    /** Gives [output] its own sound; if it is the device playing now, that sound is loaded straight away. */
    suspend fun assign(output: String, choice: Choice, current: String): Unit = lock.withLock {
        val live = output == current
        when (choice) {
            Choice.Automatic, Choice.Quiet -> {
                io { runCatching { core().profileBind(output, null) } }
                setQuiet(output, choice == Choice.Quiet)
                refresh()
                if (live) arrive(output)
            }
            Choice.Flat -> {
                io {
                    val c = core()
                    if (c.profiles().none { it.name == FLAT }) c.profileSave(SoundProfile(FLAT, Sound.of(settings.value).copy(eqEnabled = false).toJson(), emptyList()))
                }
                bind(output, FLAT, live)
            }
            is Choice.Profile -> bind(output, choice.name, live)
            is Choice.Curve -> if (live) adopt(output, choice.entry) else {
                save(choice.entry.name, fetch(choice.entry), output)
                bind(output, choice.entry.name, false)
            }
        }
    }

    private suspend fun bind(output: String, name: String, live: Boolean) {
        io { core().profileBind(output, name) }
        setQuiet(output, false)
        refresh()
        if (live) _profiles.value.firstOrNull { it.name == name }?.let { Sound.fromJson(it.json) }?.let(::load)
    }

    /**
     * Fetches [entry]'s curve, saves it as a profile named after it, binds it to [output] alone and loads it.
     * Returns whether the profile is new. Throws when the preset cannot be fetched.
     */
    private suspend fun adopt(output: String, entry: AutoEqEntry): Boolean {
        val sound = fetch(entry)
        val created = save(entry.name, sound, output)
        setQuiet(output, false)
        refresh()
        load(sound)
        return created
    }

    /** Saves [sound] as the profile [name], keeping the devices it already had, and binds [output] to it alone. True when it is new. */
    private suspend fun save(name: String, sound: Sound, output: String): Boolean = io {
        val c = core()
        val old = c.profiles().firstOrNull { it.name == name }
        c.profileSave(SoundProfile(name, sound.toJson(), old?.outputs.orEmpty()))
        c.profileBind(output, name)
        old == null
    }

    /** [entry]'s parametric curve on top of the sound as it is now. */
    private suspend fun fetch(entry: AutoEqEntry): Sound {
        val text = http().get(io { core().autoeqPresetUrl(entry) }).decodeToString()
        val preset = parseEqPreset(text)
        check(preset.bands.isNotEmpty()) { "that preset had no filters in it" }
        return Sound.of(settings.value).copy(
            eqEnabled = true, eqPreampDb = preset.preampDb,
            eqBands = preset.bands.map { Band(BandKind.entries[it.kind.ordinal], it.freq, it.gainDb, it.q) },
        )
    }

    /**
     * Loads a device's own sound. The first time one replaces a sound nobody bound to a device, that sound
     * is kept, so it comes back when the music goes to such a device again (the DAC unplugged, back to
     * the speaker).
     */
    private fun load(sound: Sound) {
        if (settings.value.profilePerOutput && !store.contains(LOOSE)) store.edit().putString(LOOSE, Sound.of(settings.value).toJson()).apply()
        settings.update { it.withSound(sound) }
    }

    private fun restore() {
        val json = store.getString(LOOSE, null) ?: return
        store.edit().remove(LOOSE).apply()
        Sound.fromJson(json)?.let { s -> settings.update { it.withSound(s) } }
    }

    private fun setQuiet(output: String, on: Boolean) {
        val next = if (on) _quiet.value + output else _quiet.value - output
        if (next == _quiet.value) return
        _quiet.value = next
        store.edit().putStringSet("quiet", next).apply()
    }

    /** A device the list no longer needs to show: its binding and its "never ask" go with it. */
    suspend fun forget(output: String): Unit = lock.withLock {
        io { runCatching { core().profileBind(output, null) } }
        setQuiet(output, false)
        refresh()
    }

    private suspend fun <T> io(block: suspend () -> T): T = withContext(Dispatchers.IO) { block() }

    companion object {
        const val FLAT = "Flat"
        private const val LOOSE = "looseSound"
    }
}
