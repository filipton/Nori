package dev.nori.music.app

/**
 * The handles a test harness drives the app by. The app fills them in while it is running; the debug
 * build's broadcast receiver calls them. Empty in a release build, where nothing ever registers or
 * invokes them, so this costs three null fields.
 *
 * Driving the app by tapping screen coordinates is what made every earlier check flaky: the on-screen
 * keyboard swallowed taps, a sleeping device returned stale layouts, and a title added to a screen
 * moved every row below it. These go straight at the app instead.
 */
object TestHooks {
    /** Navigate to a route, e.g. "library", "settings/audio", "album/<id>". */
    @Volatile var open: ((String) -> Unit)? = null

    /** A one-line JSON snapshot: route, what is playing, and the settings a test cares about. */
    @Volatile var state: (() -> String)? = null

    /** Flip one named setting, e.g. "limiter" to true. Returns false for a name it does not know. */
    @Volatile var set: ((String, String) -> Boolean)? = null

    /** Play something by id: "song:<id>", "album:<id>", or "search:<text>" for the first hit. */
    @Volatile var play: ((String) -> Unit)? = null

    /** Sign in to a server: "url|user|password". */
    @Volatile var login: ((String) -> Unit)? = null

    /** Anything that is neither navigation nor settings: "download search:x", "star", "pause", "next". */
    @Volatile var act: ((String) -> Unit)? = null
}
