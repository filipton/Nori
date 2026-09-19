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
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.graphics.graphicsLayer
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

/**
 * Navigation as the screens see it; they never touch the NavController. Going anywhere puts the player
 * away first, the way an artist tapped on Apple's player drops it and opens the artist underneath.
 */
class Nav(private val c: NavHostController, private val sheet: PlayerSheet) {
    fun go(route: String) { if (sheet.isOpen) sheet.close(); c.navigate(route) }
    fun album(id: String) = go("album/${Uri.encode(id)}")
    fun artist(id: String) = go("artist/${Uri.encode(id)}")
    fun playlist(id: String) = go("playlist/${Uri.encode(id)}")
    fun genre(name: String) = go("genre/${Uri.encode(name)}")
    fun folder(id: String) = go("folder/${Uri.encode(id)}")
    fun decade(year: Int) = go("decade/$year")
    fun smart(id: String) = go("smart/${Uri.encode(id)}")
    fun smartEdit(id: String) = go("smartEdit/${Uri.encode(id.ifEmpty { "new" })}")
    fun stats() = go("stats")
    /** A settings group, optionally landing on one row of it (from the settings search). */
    fun settingsGroup(id: String, key: String = "") = go("settings/$id?key=${Uri.encode(key)}")
    fun player() = sheet.open()
    fun equalizer() = go("equalizer")
    fun autoEq() = go("autoeq")
    fun back() { if (sheet.isOpen) sheet.close() else c.popBackStack() }
    /**
     * A tab always lands on that tab's own page. It used to save the stack it popped and restore it on
     * the way back, which meant tapping Home from an album popped the album and then put it straight
     * back - the tab looked dead. Nothing above the tab roots survives a tab tap now.
     */
    fun tab(route: String) {
        if (sheet.isOpen) sheet.close()
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
    androidx.compose.runtime.SideEffect { AppMotion.force = prefs.ignoreSystemMotion }
    FlintTheme(prefs) {
        if (!prefs.loggedIn) { LoginScreen(settings); return@FlintTheme }

        val controller = rememberNavController()
        val sheetScope = androidx.compose.runtime.rememberCoroutineScope()
        val sheet = remember { PlayerSheet(sheetScope) }
        sheet.plain = reduceMotion()
        val nav = remember(controller) { Nav(controller, sheet) }
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
            dev.flint.music.app.TestHooks.open = { route -> if (route == "player") sheet.open() else nav.go(route) }
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
                """{"route":"${if (sheet.isOpen) "player" else controller.currentBackStackEntry?.destination?.route}",""" +
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
            LocalPlayerSheet provides sheet,
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
            Box(Modifier.fillMaxSize().onGloballyPositioned { sheet.rootHeight = it.size.height.toFloat() }) {
              // Everything under the player. Once the player covers it completely it is not drawn at all:
              // a layer at zero alpha is skipped, so a page left animating underneath costs nothing.
              Box(Modifier.fillMaxSize().graphicsLayer { alpha = if (sheet.progress.value >= 1f) 0f else 1f }) {
              CompositionLocalProvider(LocalStarMarks provides marks, LocalChromeInset provides chromeHeight) {
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
              Box(
                  Modifier.align(Alignment.BottomCenter)
                      .onGloballyPositioned { chromeHeight = with(density) { it.size.height.toDp() } },
              ) { BottomChrome(player, actions, route, tabs, nav::tab, nav::player) }
              }
              PlayerLayer(sheet) { CompositionLocalProvider(LocalStarMarks provides marks) { PlayerScreen(player, actions) } }
              SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).padding(bottom = chromeHeight))
            }
            SheetBack(sheet)
            }
            menuSong?.let { SongMenu(it, actions, onDismiss = { menuSong = null }, player = player.takeIf { menuFromPlayer }) }
        }
    }
}

/**
 * Back puts the player away while it is up. Registered after the NavHost's own, so it goes first; and
 * its own small scope, so the sheet opening and closing recomposes this and not the whole app.
 */
@Composable
private fun SheetBack(sheet: PlayerSheet) {
    androidx.activity.compose.BackHandler(sheet.isOpen) { sheet.close() }
}

/**
 * The player sheet over everything else, present only while it is open or moving. The page behind
 * darkens as it rises; the sheet's top corners round off while it is in flight and square up once it
 * fills the screen. All of it is read in the draw phase, so a drag redraws two layers and recomposes
 * nothing.
 */
@Composable
private fun PlayerLayer(sheet: PlayerSheet, content: @Composable () -> Unit) {
    val shown by remember { androidx.compose.runtime.derivedStateOf { sheet.progress.value > 0f || sheet.progress.targetValue > 0f } }
    // Built once, shortly after the app is up, and kept: opening the player then only moves it. Until
    // then, and on a phone too slow to have got there, the first open builds it.
    var warm by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { kotlinx.coroutines.delay(1500); warm = true }
    if (!shown && !warm) return
    val radius = with(androidx.compose.ui.platform.LocalDensity.current) { 14.dp.toPx() }
    if (shown) Box(
        Modifier.fillMaxSize().drawBehind {
            drawRect(androidx.compose.ui.graphics.Color.Black.copy(alpha = 0.45f * sheet.progress.value))
        },
    )
    Box(
        Modifier.fillMaxSize().graphicsLayer {
            // Put away, it is parked a whole screen below the bottom edge: nothing drawn, and nothing to
            // catch a touch meant for the mini player (a transparent layer would still be hit).
            val parked = sheet.progress.value == 0f && sheet.progress.targetValue == 0f
            translationY = if (parked) sheet.rootHeight * 2f + 1f else sheet.offset()
            alpha = if (parked) 0f else 1f
            val r = radius * (1f - sheet.progress.value).coerceIn(0f, 1f) * 4f
            shape = androidx.compose.foundation.shape.RoundedCornerShape(topStart = r.coerceAtMost(radius), topEnd = r.coerceAtMost(radius))
            clip = true
        },
    ) {
        CompositionLocalProvider(LocalChromeInset provides 0.dp, LocalPlayerShown provides shown) { content() }
    }
}
