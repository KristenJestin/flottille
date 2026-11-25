package org.flottille.app

import android.Manifest
import android.app.Application
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.flottille.core.FlottilleCore
import org.flottille.core.Phone
import org.flottille.core.RealPhone

/** Holds the phone's core for the life of the process: one core, whichever activity is on screen. */
class FlottilleApplication : Application() {
    private val real: Phone by lazy { RealPhone(FlottilleCore(this)) }
    private var played: Phone? = null

    /** What the runners told, drawn once each for the life of the process; a test starts its own. */
    lateinit var notices: Notices

    /** The core. A test that plays it (a fake core and a fake server) sets this before the screens start. */
    var phone: Phone
        get() = played ?: real
        set(value) {
            played = value
        }

    override fun onCreate() {
        super.onCreate()
        notices = Notices(this)
        // A panic of the core is written where the next start can read it.
        FlottilleCore.keepPanics(FlottilleCore.folder(this))
    }
}

/** The pictures shared to the app, waiting for the session they go to. */
class Incoming : ViewModel() {
    var shared by mutableStateOf(Pictured(emptyList()))
}

class MainActivity : ComponentActivity() {
    // Registered before the activity starts: the gallery, and the camera with its permission.
    private val pictures = SystemPictures(this)
    private val tunnels = SystemTunnelHost(this)
    private val background = SystemBackground(this)

    // Asked once the phone joined: without it, Android shows none of the notifications. Whatever the
    // answer, Settings says it and leads to the system's page.
    private val notifying = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}
    private var askedNotifying = false

    // What an intent asks of the app: a session to open (a notification's, #828), pictures to place
    // (the share sheet). Both wait here until the screens take them.
    private var open by mutableStateOf<SessionKey?>(null)
    // Held by a model, which a rotation keeps: the share is read once, so the activity must not
    // lose what it read.
    private val incoming: Incoming by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        pictures.clearLeftovers()
        val phone = (application as FlottilleApplication).phone
        // A recreated activity (a rotation) holds the same intent: what it shared is not read again.
        taken(intent, recreated = savedInstanceState != null)
        // A paired phone keeps its connection to the hub in a service, from its join on.
        askedNotifying = savedInstanceState != null
        lifecycleScope.launch {
            lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
                while (!phone.view.joined) delay(JOINED_CHECK)
                keepReachable()
            }
        }
        setContent {
            FlottilleApp(
                phone = phone,
                deviceName = Build.MODEL.orEmpty().ifBlank { "phone" },
                pictures = pictures,
                shared = incoming.shared.pictures,
                sharedNote = incoming.shared.refused,
                onSharedPlaced = { incoming.shared = Pictured(emptyList()) },
                open = open,
                tunnels = tunnels,
                background = background,
            )
        }
    }

    private fun keepReachable() {
        HubService.start(this)
        if (!askedNotifying && Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            askedNotifying = true
            notifying.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        taken(intent, recreated = false)
    }

    private fun taken(intent: Intent, recreated: Boolean) {
        val asked = asked(intent, packageName, recreated)
        asked.open?.let { open = it }
        if (asked.share) {
            // Read off the main thread: a picture can be megabytes. Nothing is sent: the pictures
            // wait for the user to say which session they go to.
            lifecycleScope.launch {
                incoming.shared = withContext(Dispatchers.IO) {
                    readPictures(sharedUris(intent), ResolverReader(contentResolver))
                }
            }
        }
    }

    companion object {
        /** An intent that opens a session's page names its machine and its agent. */
        const val EXTRA_MACHINE = "org.flottille.app.MACHINE"
        const val EXTRA_AGENT = "org.flottille.app.AGENT"
        private const val JOINED_CHECK = 2_000L
    }
}
