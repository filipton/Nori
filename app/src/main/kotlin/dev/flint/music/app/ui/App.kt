package dev.flint.music.app.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.ui.platform.LocalContext
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.LibraryMusic
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.Surface
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.material3.MaterialTheme
import androidx.compose.foundation.background
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.Alignment
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.Box
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.PlayerViewModel
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.ffi.Song

/** Plain screens sit below the status bar; album, artist and playlist pages draw under it. */
@Composable
private fun Inset(content: @Composable () -> Unit) = androidx.compose.foundation.layout.Box(Modifier.statusBarsPadding()) { content() }

/** Navigation as the screens see it; they never touch the NavController. */
class Nav(private val c: NavHostController) {
    fun album(id: String) = c.navigate("album/${Uri.encode(id)}")
    fun artist(id: String) = c.navigate("artist/${Uri.encode(id)}")
    fun playlist(id: String) = c.navigate("playlist/${Uri.encode(id)}")
    fun genre(name: String) = c.navigate("genre/${Uri.encode(name)}")
    fun folder(id: String) = c.navigate("folder/${Uri.encode(id)}")
    fun decade(year: Int) = c.navigate("decade/$year")
    fun smart(id: String) = c.navigate("smart/${Uri.encode(id)}")
    fun smartEdit(id: String) = c.navigate("smartEdit/${Uri.encode(id.ifEmpty { "new" })}")
    fun stats() = c.navigate("stats")
    /** A settings group, optionally landing on one row of it (from the settings search). */
    fun settingsGroup(id: String, key: String = "") = c.navigate("settings/$id?key=${Uri.encode(key)}")
    fun player() = c.navigate("player") { launchSingleTop = true }
    fun equalizer() = c.navigate("equalizer")
    fun autoEq() = c.navigate("autoeq")
    fun back() { c.popBackStack() }
    /**
     * A tab always lands on that tab's own page. It used to save the stack it popped and restore it on
     * the way back, which meant tapping Home from an album popped the album and then put it straight
     * back - the tab looked dead. Nothing above the tab roots survives a tab tap now.
     */
    fun tab(route: String) {
        // Tapping Search is a request for the keyboard, whether or not the screen is already open -
        // and with launchSingleTop it is not recomposed, so nothing else would notice the tap.
        if (route == "search") searchTaps.intValue++
        c.navigate(route) {
            popUpTo(c.graph.startDestinationId)
            launchSingleTop = true
        }
    }
}

/** Counts taps on the search tab; the search field takes focus again on each one. */
private val searchTaps = androidx.compose.runtime.mutableIntStateOf(0)

@Composable
internal fun searchFocusKey(): Int = searchTaps.intValue

val LocalNav = staticCompositionLocalOf<Nav> { error("no nav") }
val LocalSongMenu = staticCompositionLocalOf<(Song) -> Unit> { {} }

/** The player's own ⋯: the same song menu, with the playback-wide entries the player needs. */
val LocalPlayerMenu = staticCompositionLocalOf<(Song) -> Unit> { {} }

private val tabs = listOf(
    Tab("home", "Home", Icons.Filled.Home),
    Tab("search", "Search", Icons.Filled.Search),
    Tab("library", "Library", Icons.Filled.LibraryMusic),
    Tab("settings", "Settings", Icons.Filled.Settings),
)

@Composable
fun App() {
    val settings: SettingsViewModel = viewModel()
    val prefs by settings.prefs.collectAsStateWithLifecycle()
    FlintTheme(prefs) {
        if (!prefs.loggedIn) { LoginScreen(settings); return@FlintTheme }

        val controller = rememberNavController()
        val nav = remember(controller) { Nav(controller) }
        val actions: ActionsViewModel = viewModel()
        val player: PlayerViewModel = viewModel()
        val snackbar = remember { SnackbarHostState() }
        var menuSong by remember { mutableStateOf<Song?>(null) }
        var menuFromPlayer by remember { mutableStateOf(false) }
        LaunchedEffect(Unit) { actions.messages.collect { snackbar.showSnackbar(it) } }
        // Headphones or a DAC connected and nothing is bound to them yet: offer their AutoEQ curve, once per device.
        val output by settings.currentOutput.collectAsStateWithLifecycle()
        val profiles by settings.profiles.collectAsStateWithLifecycle()
        LaunchedEffect(output) {
            if (output == dev.flint.music.playback.Outputs.SPEAKER || profiles.any { output in it.outputs }) return@LaunchedEffect
            val hit = settings.autoEqFor(output).firstOrNull() ?: return@LaunchedEffect
            val result = snackbar.showSnackbar("${hit.name} connected. Use its AutoEQ curve?", actionLabel = "Apply", withDismissAction = true, duration = androidx.compose.material3.SnackbarDuration.Long)
            if (result == androidx.compose.material3.SnackbarResult.ActionPerformed) settings.adoptAutoEq(hit, output)
        }
        val context = LocalContext.current
        LaunchedEffect(Unit) {
            actions.shares.collect { url ->
                context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, url), null))
            }
        }

        // The test bridge's handles, live for as long as the app is on screen. See TestHooks.
        val player2 = player
        androidx.compose.runtime.DisposableEffect(controller) {
            dev.flint.music.app.TestHooks.open = { route -> controller.navigate(route) }
            dev.flint.music.app.TestHooks.set = { name, value -> settings.setByName(name, value) }
            dev.flint.music.app.TestHooks.play = { what -> actions.playByRef(what) }
            dev.flint.music.app.TestHooks.login = { spec ->
                val (url, user, pass) = spec.split("|").let { Triple(it[0], it.getOrElse(1) { "" }, it.getOrElse(2) { "" }) }
                settings.login(settings.newProfile().copy(url = url, user = user, password = pass))
            }
            dev.flint.music.app.TestHooks.act = { what -> actions.testAction(what, player2) }
            dev.flint.music.app.TestHooks.state = {
                val st = player2.state.value
                val p = settings.prefs.value
                """{"route":"${controller.currentBackStackEntry?.destination?.route}",""" +
                    """"playing":${st.playing},"title":"${st.current?.title.orEmpty()}","artist":"${st.current?.artist.orEmpty()}",""" +
                    """"positionMs":${player2.positionMs},"durationMs":${st.durationMs},"queue":${st.queue.size},"index":${st.index},""" +
                    """"error":"${st.error.orEmpty()}","eq":${p.eqEnabled},"limiter":${p.limiter},"hiRes":${p.hiRes},""" +
                    """"dspActive":${dev.flint.music.playback.Equalizer.active != null},"gainReductionDb":${dev.flint.music.playback.Equalizer.active?.gainReductionDb ?: 0f},""" +
                    """"output":"${settings.currentOutput.value}","offload":${p.offload},"offloadWanted":${dev.flint.music.playback.PlaybackService.offloadWanted},"autoMix":${p.autoMix},"amoled":${p.amoled},""" +
                    """"mixing":${dev.flint.music.playback.TransitionSink.mixing},""" +
                    """"downloaded":${actions.downloads.value.done.size},"downloading":${actions.downloads.value.pending.size},""" +
                    """"sinkBytes":${dev.flint.music.playback.BurstSink.bytesWritten},""" +
                    dev.flint.music.Flint.get(context).dac.state.value.let { d ->
                        """"dac":"${d.device.orEmpty()}","bitPerfect":${d.bitPerfect},"dacModes":${d.modes.size},""" +
                            """"dacBlocked":"${d.blockedBy.orEmpty()}","dacTrack":"${d.track.orEmpty()}","""
                    } +
                    (actions.lastLyrics ?: (player2.lyrics.value as? dev.flint.music.app.vm.Load.Ready)?.data)?.let { f ->
                        """"lyricLines":${f.lyrics.lines.size},"lyricsSynced":${f.lyrics.synced},""" +
                            """"lyricsWordTimed":${f.lyrics.wordTimed},"lyricsSource":"${f.source}","""
                    }.orEmpty() +
                    // What the screen shows, mark included - not the snapshot the queue was painted with,
                    // which is what a favourite toggled this session no longer agrees with.
                    """"starred":${st.current?.let { actions.starMarks.value.effectiveStar(dev.flint.music.data.StarKind.SONG, it.id, it.starred) } ?: false},""" +
                    """"loggedIn":${p.loggedIn},"server":"${p.server?.url.orEmpty()}","loginError":"${settings.login.value.error.orEmpty().replace("\"", "'")}"}"""
            }
            onDispose {
                dev.flint.music.app.TestHooks.open = null
                dev.flint.music.app.TestHooks.state = null
                dev.flint.music.app.TestHooks.set = null
                dev.flint.music.app.TestHooks.play = null
            }
        }

        CompositionLocalProvider(
            LocalNav provides nav,
            LocalSongMenu provides { menuSong = it; menuFromPlayer = false },
            LocalPlayerMenu provides { menuSong = it; menuFromPlayer = true },
        ) {
            val route = controller.currentBackStackEntryAsState().value?.destination?.route
            // This session's star changes, so every heart prefers them over the snapshot it painted with.
            val marks by actions.starMarks.collectAsStateWithLifecycle()
            // The chrome floats over the page rather than ending it: the page fills the window, its colour
            // reaches the bottom edge, and the list scrolls under the mini player the way Apple's does.
            // Screens keep the last row reachable by adding LocalChromeInset to their content padding.
            var chromeHeight by remember { mutableStateOf(0.dp) }
            val density = androidx.compose.ui.platform.LocalDensity.current
            // This replaced a Scaffold when the chrome started floating, and with it went the two things
            // Scaffold quietly provided: something that paints the app's background (every screen was
            // showing the window's default grey, lighter than our own cards) and a content colour for
            // text that does not name one (which left titles rendering almost black).
            Surface(color = MaterialTheme.colorScheme.background, contentColor = MaterialTheme.colorScheme.onBackground) {
            Box(Modifier.fillMaxSize()) {
              CompositionLocalProvider(LocalStarMarks provides marks, LocalChromeInset provides if (route == "player") 0.dp else chromeHeight) {
                // One transition for the whole app, and a quiet one: pages slide a little and fade, the
                // way a push does on a phone. The default jumps and the horizontal slide across the
                // full width reads as a lurch on a large screen.
                val plain = reduceMotion()
                val slide = if (plain) 0 else 40
                val fast = if (plain) 90 else 220
                NavHost(
                    controller, "home",
                    enterTransition = {
                        androidx.compose.animation.slideInHorizontally(androidx.compose.animation.core.tween(fast)) { slide } +
                            androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(if (plain) 90 else 180))
                    },
                    exitTransition = { androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(if (plain) 70 else 140)) },
                    popEnterTransition = { androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(if (plain) 90 else 180)) },
                    popExitTransition = {
                        androidx.compose.animation.slideOutHorizontally(androidx.compose.animation.core.tween(if (plain) 90 else 200)) { slide } +
                            androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(if (plain) 70 else 160))
                    },
                ) {
                    composable("home") { Inset { HomeScreen(actions) } }
                    composable("search") { Inset { SearchScreen(actions) } }
                    composable("library") { Inset { LibraryScreen(actions) } }
                    composable("settings") { Inset { SettingsScreen(settings) } }
                    composable("settings/{id}?key={key}") { e ->
                        Inset { SettingsGroupScreen(settings, e.arguments!!.getString("id")!!, e.arguments?.getString("key").orEmpty()) }
                    }
                    composable("equalizer") { Inset { EqualizerScreen(settings) } }
                    composable("autoeq") { Inset { AutoEqScreen(settings) } }
                    composable(
                        "player",
                        // The mini player opens with an upward swipe, so the full player rises with it -
                        // the default sideways slide reads as a jump after a vertical gesture. Other
                        // routes keep the app-wide transition; back needs no theatre.
                        enterTransition = {
                            androidx.compose.animation.slideInVertically(androidx.compose.animation.core.tween(if (plain) 90 else 260)) { it } +
                                androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(if (plain) 90 else 180))
                        },
                        popExitTransition = {
                            androidx.compose.animation.slideOutVertically(androidx.compose.animation.core.tween(if (plain) 90 else 240)) { it } +
                                androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(if (plain) 70 else 160))
                        },
                    ) { PlayerScreen(player, actions) }
                    composable("album/{id}") { AlbumScreen(it.arguments!!.getString("id")!!, actions) }
                    composable("artist/{id}") { ArtistScreen(it.arguments!!.getString("id")!!, actions) }
                    composable("playlist/{id}") { PlaylistScreen(it.arguments!!.getString("id")!!, actions) }
                    composable("genre/{id}") { Inset { GenreScreen(it.arguments!!.getString("id")!!, actions) } }
                    composable("smart/{id}") { Inset { SmartScreen(it.arguments!!.getString("id")!!, actions) } }
                    composable("smartEdit/{id}") { Inset { SmartEditScreen(it.arguments!!.getString("id")!!.let { i -> if (i == "new") "" else i }) } }
                    composable("stats") { Inset { StatsScreen() } }
                    composable("folder/{id}") { Inset { FolderScreen(it.arguments!!.getString("id")!!, actions) } }
                    composable("decade/{year}") { Inset { SongsScreen(actions, it.arguments!!.getString("year")!!.toInt()) } }
                }
              }
              if (route != "player") Box(
                  Modifier.align(Alignment.BottomCenter)
                      .onGloballyPositioned { chromeHeight = with(density) { it.size.height.toDp() } },
              ) { BottomChrome(player, actions, route, tabs, nav::tab, nav::player) }
              SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).padding(bottom = chromeHeight))
            }
            }
            menuSong?.let { SongMenu(it, actions, onDismiss = { menuSong = null }, player = player.takeIf { menuFromPlayer }) }
        }
    }
}
