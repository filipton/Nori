package dev.flint.music.app

import android.Manifest
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import dev.flint.music.Flint
import dev.flint.music.app.ui.App

class MainActivity : ComponentActivity() {
    private val askNotifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (Build.VERSION.SDK_INT >= 33 && savedInstanceState == null) askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        setContent { App() }
    }

    override fun onStart() {
        super.onStart()
        if (Flint.get(this).settings.value.loggedIn) Flint.get(this).player.connect()
    }

    override fun onStop() {
        super.onStop()
        // The service keeps playing on its own; holding a controller while hidden would only keep callbacks flowing.
        Flint.get(this).player.disconnect()
    }
}
