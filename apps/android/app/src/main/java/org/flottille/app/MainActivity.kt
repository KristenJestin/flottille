package org.flottille.app

import android.app.Application
import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.flottille.core.Picture
import org.flottille.core.FlottilleCore
import org.flottille.core.Phone
import org.flottille.core.RealPhone

/** Holds the phone's core for the life of the process: one core, whichever activity is on screen. */
class FlottilleApplication : Application() {
    val phone: Phone by lazy { RealPhone(FlottilleCore(this)) }

    override fun onCreate() {
        super.onCreate()
        // A panic of the core is written where the next start can read it.
        FlottilleCore.keepPanics(FlottilleCore.folder(this))
    }
}

class MainActivity : ComponentActivity() {
    // Registered before the activity starts: the gallery, and the camera with its permission.
    private val pictures = SystemPictures(this)

    // What an intent asks of the app: a session to open (a notification's, #828), pictures to place
    // (the share sheet). Both wait here until the screens take them.
    private var open by mutableStateOf<SessionKey?>(null)
    private var shared by mutableStateOf<List<Picture>>(emptyList())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val phone = (application as FlottilleApplication).phone
        taken(intent)
        setContent {
            FlottilleApp(
                phone = phone,
                deviceName = Build.MODEL.orEmpty().ifBlank { "phone" },
                pictures = pictures,
                shared = shared,
                onSharedPlaced = { shared = emptyList() },
                open = open,
            )
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        taken(intent)
    }

    private fun taken(intent: Intent) {
        val machine = intent.getStringExtra(EXTRA_MACHINE)
        val agent = intent.getStringExtra(EXTRA_AGENT)
        if (machine != null && agent != null) open = SessionKey(machine, agent)
        if (intent.action == Intent.ACTION_SEND || intent.action == Intent.ACTION_SEND_MULTIPLE) {
            // Read off the main thread: a picture can be megabytes. Nothing is sent: the pictures
            // wait for the user to say which session they go to.
            lifecycleScope.launch {
                shared = withContext(Dispatchers.IO) { sharedPictures(this@MainActivity, intent) }
            }
        }
    }

    companion object {
        /** An intent that opens a session's page names its machine and its agent. */
        const val EXTRA_MACHINE = "org.flottille.app.MACHINE"
        const val EXTRA_AGENT = "org.flottille.app.AGENT"
    }
}
