package org.flottille.app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import androidx.core.app.NotificationManagerCompat

/** Whether Android lets the app notify, and lets it run asleep (exempted from battery optimisation). */
data class BackgroundState(val notifications: Boolean, val exempted: Boolean)

/**
 * What Settings asks of the system so that the phone is told when an agent needs the user
 * (`docs/remote.md`, "Notifications"). The screens hold this, so that their tests play it.
 */
interface Background {
    fun state(): BackgroundState

    /** The system's page of the app's notifications. */
    fun allowNotifications()

    /** The system's question: exempt the app from battery optimisation. */
    fun exempt()

    object None : Background {
        override fun state() = BackgroundState(notifications = true, exempted = true)

        override fun allowNotifications() = Unit

        override fun exempt() = Unit
    }
}

class SystemBackground(private val activity: Activity) : Background {
    override fun state() = BackgroundState(
        notifications = NotificationManagerCompat.from(activity).areNotificationsEnabled(),
        exempted = activity.getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(activity.packageName),
    )

    override fun allowNotifications() {
        start(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName))
    }

    override fun exempt() {
        // The direct question; a system without it has the list of the apps instead.
        @Suppress("BatteryLife")
        val asked = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${activity.packageName}"))
        if (!start(asked)) start(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
    }

    private fun start(intent: Intent): Boolean =
        try {
            activity.startActivity(intent)
            true
        } catch (none: ActivityNotFoundException) {
            false
        }
}
