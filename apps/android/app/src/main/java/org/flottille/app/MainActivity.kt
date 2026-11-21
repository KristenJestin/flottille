package org.flottille.app

import android.app.Application
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
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
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val phone = (application as FlottilleApplication).phone
        setContent { FlottilleApp(phone = phone, deviceName = Build.MODEL.orEmpty().ifBlank { "phone" }) }
    }
}
