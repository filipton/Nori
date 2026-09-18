package dev.flint.music.app.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.ui.platform.LocalContext
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.LibraryMusic
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
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
    fun player() = c.navigate("player") { launchSingleTop = true }
    fun equalizer() = c.navigate("equalizer")
    fun autoEq() = c.navigate("autoeq")
    fun back() { c.popBackStack() }
    fun tab(route: String) = c.navigate(route) {
        popUpTo(c.graph.startDestinationId) { saveState = true }
        launchSingleTop = true
        restoreState = true
    }
}

val LocalNav = staticCompositionLocalOf<Nav> { error("no nav") }
val LocalSongMenu = staticCompositionLocalOf<(Song) -> Unit> { {} }

private val tabs = listOf(Triple("home", "Home", Icons.Filled.Home), Triple("search", "Search", Icons.Filled.Search), Triple("library", "Library", Icons.Filled.LibraryMusic), Triple("settings", "Settings", Icons.Filled.Settings))

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

        CompositionLocalProvider(LocalNav provides nav, LocalSongMenu provides { menuSong = it }) {
            val route = controller.currentBackStackEntryAsState().value?.destination?.route
            Scaffold(
                snackbarHost = { SnackbarHost(snackbar) },
                bottomBar = {
                    if (route != "player") Column {
                        SelectionBar(actions)
                        MiniPlayer(player, onOpen = nav::player)
                        NavigationBar {
                            tabs.forEach { (r, label, icon) ->
                                NavigationBarItem(selected = route == r, onClick = { nav.tab(r) }, icon = { Icon(icon, label) }, label = { Text(label) })
                            }
                        }
                    }
                },
            ) { pad ->
                NavHost(controller, "home", Modifier.padding(bottom = pad.calculateBottomPadding())) {
                    composable("home") { Inset { HomeScreen(actions) } }
                    composable("search") { Inset { SearchScreen(actions) } }
                    composable("library") { Inset { LibraryScreen(actions) } }
                    composable("settings") { Inset { SettingsScreen(settings) } }
                    composable("equalizer") { Inset { EqualizerScreen(settings) } }
                    composable("autoeq") { Inset { AutoEqScreen(settings) } }
                    composable("player") { PlayerScreen(player, actions) }
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
            menuSong?.let { SongMenu(it, actions, onDismiss = { menuSong = null }) }
        }
    }
}
