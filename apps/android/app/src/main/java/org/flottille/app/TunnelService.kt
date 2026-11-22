package org.flottille.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.flottille.core.OpenedTunnel
import org.flottille.core.Phone
import org.flottille.core.PhoneException
import org.flottille.core.Tunnel

/**
 * What the session page asks of the system for a run's page (`docs/remote.md`, "Tunnels", "On the
 * phone"): an address handed to the user's browser, with its tunnel kept alive meanwhile. The
 * screens hold this, so that their tests play it.
 */
interface TunnelHost {
    /** Hands [opened]'s address to the user's browser and keeps its tunnel with a foreground service. */
    fun keepForBrowser(opened: OpenedTunnel)

    /** An address that is no run's (a page of the web) opens as it is, in the user's browser. */
    fun browse(url: String)

    object None : TunnelHost {
        override fun keepForBrowser(opened: OpenedTunnel) = Unit

        override fun browse(url: String) = Unit
    }
}

/**
 * The system's: the service of [TunnelService], then the user's browser (the default handler of the
 * address). [launch] is the browser's own call, which a test replaces. The notifications' permission is asked once, at the first
 * Open in browser, and the tunnel is kept whatever the answer (the service says so in the task
 * manager even when its notification is hidden).
 */
class SystemTunnelHost(
    private val activity: ComponentActivity,
    private val launch: (String) -> Boolean = { url -> startBrowser(activity, url) },
) : TunnelHost {
    private var waiting: (() -> Unit)? = null

    // The registry's own, not tied to the activity's lifecycle: this is built with the screens, which
    // a test (and a recreated activity) may start after the activity did.
    private val permission = activity.activityResultRegistry.register(
        "tunnel-notifications",
        ActivityResultContracts.RequestPermission(),
    ) {
        waiting?.invoke()
        waiting = null
    }

    override fun keepForBrowser(opened: OpenedTunnel) {
        // The core reads an address through `localhost`, so this holds; checked all the same, since
        // the system's default handler is handed the address: only a web address is.
        if (!isWebAddress(opened.url)) return
        val go = {
            TunnelService.keep(activity, opened.tunnel)
            launch(opened.url)
            Unit
        }
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            waiting = go
            permission.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            go()
        }
    }

    override fun browse(url: String) {
        // Only a page of the web: an address of another scheme (`intent:`, `market:`, `file:`…) would
        // start another app on the word of a run's page or an agent's link.
        if (isWebAddress(url)) launch(url)
    }

    private companion object {
        fun startBrowser(context: Context, url: String): Boolean =
            try {
                context.startActivity(
                    Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
                true
            } catch (none: android.content.ActivityNotFoundException) {
                false
            }
    }
}

/**
 * Keeps the tunnels of "Open in browser" while the app is in the background (`docs/remote.md`,
 * "Tunnels"): a foreground service with one notification that says which tunnels it holds and has
 * Close. It holds the core's connection (`Phone.keepConnected`) so that the links stay up, and ends
 * when its tunnels have: closed from the notification, the run ended, the link dropped. An end that
 * is not the user's own is said in a notification of its own, which stays.
 */
class TunnelService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val kept = LinkedHashMap<String, Tunnel>()
    private var hold: AutoCloseable? = null
    private var watching: Job? = null

    private val phone: Phone get() = (application as FlottilleApplication).phone

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_CLOSE -> closeAll()
            else -> intent?.let { tunnelOf(it) }?.let { keep(it) }
        }
        if (kept.isEmpty()) stop()
        return START_NOT_STICKY
    }

    private fun keep(tunnel: Tunnel) {
        channels()
        kept[tunnel.tunnel] = tunnel
        // The first tunnel makes the service foreground; the next ones change what it says.
        ServiceCompat.startForeground(
            this,
            KEPT_ID,
            notification(),
            if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0,
        )
        if (hold == null) hold = phone.keepConnected()
        if (watching == null) watching = scope.launch { watch() }
    }

    /** Until its tunnels have ended: each end is read in the view, with its reason. */
    private suspend fun watch() {
        phone.views().collect { view ->
            val ended = view.tunnelsEnded.filter { it.tunnel in kept }
            ended.forEach { end ->
                val tunnel = kept.remove(end.tunnel) ?: return@forEach
                if (end.reason != USER_CLOSED) says(tunnel, end.reason)
            }
            if (ended.isNotEmpty()) {
                if (kept.isEmpty()) stop() else refresh()
            }
        }
    }

    /** Close, from the notification: every tunnel the service keeps. */
    private fun closeAll() {
        kept.keys.toList().forEach { id ->
            try {
                phone.closeTunnel(id)
            } catch (refused: PhoneException) {
                // Already ended: the view says why.
            }
        }
        kept.clear()
    }

    private fun refresh() {
        getSystemService(NotificationManager::class.java).notify(KEPT_ID, notification())
    }

    private fun stop() {
        watching?.cancel()
        watching = null
        hold?.close()
        hold = null
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        hold?.close()
        scope.cancel()
        super.onDestroy()
    }

    private fun notification(): Notification {
        val close = PendingIntent.getService(
            this,
            0,
            Intent(this, TunnelService::class.java).setAction(ACTION_CLOSE),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_KEPT)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle(if (kept.size == 1) "Tunnel kept open" else "${kept.size} tunnels kept open")
            .setContentText(kept.values.joinToString(", ") { it.said() })
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(0, "Close", close)
            .build()
    }

    /** The tunnel ended and why: a notice that stays until the user sees it. */
    private fun says(tunnel: Tunnel, reason: String) {
        getSystemService(NotificationManager::class.java).notify(
            ENDED_ID + (tunnel.tunnel.hashCode() and 0xFFFF),
            NotificationCompat.Builder(this, CHANNEL_ENDED)
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setContentTitle("Tunnel to ${tunnel.machine} ended")
                .setContentText("${tunnel.said()}: $reason")
                .setAutoCancel(true)
                .build(),
        )
    }

    private fun channels() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_KEPT, "Tunnels kept open", NotificationManager.IMPORTANCE_LOW),
        )
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ENDED, "Tunnels that ended", NotificationManager.IMPORTANCE_DEFAULT),
        )
    }

    companion object {
        const val ACTION_CLOSE = "org.flottille.app.TUNNEL_CLOSE"
        const val CHANNEL_KEPT = "tunnels-kept"
        const val CHANNEL_ENDED = "tunnels-ended"
        const val KEPT_ID = 8310
        const val ENDED_ID = 8311

        /** What the core says of a tunnel the user closed: no notice for it. */
        const val USER_CLOSED = "closed by the user"
        private const val EXTRA_TUNNEL = "org.flottille.app.TUNNEL"
        private const val EXTRA_MACHINE = "org.flottille.app.TUNNEL_MACHINE"
        private const val EXTRA_RUN = "org.flottille.app.TUNNEL_RUN"
        private const val EXTRA_PORT = "org.flottille.app.TUNNEL_PORT"
        private const val EXTRA_LOCAL = "org.flottille.app.TUNNEL_LOCAL"

        /** Starts the service for [tunnel], or tells the one that runs. */
        fun keep(context: Context, tunnel: Tunnel) {
            ContextCompat.startForegroundService(
                context,
                Intent(context, TunnelService::class.java)
                    .putExtra(EXTRA_TUNNEL, tunnel.tunnel)
                    .putExtra(EXTRA_MACHINE, tunnel.machine)
                    .putExtra(EXTRA_RUN, tunnel.run)
                    .putExtra(EXTRA_PORT, tunnel.port)
                    .putExtra(EXTRA_LOCAL, tunnel.localPort),
            )
        }

        private fun tunnelOf(intent: Intent): Tunnel? {
            val id = intent.getStringExtra(EXTRA_TUNNEL) ?: return null
            return Tunnel(
                id,
                intent.getStringExtra(EXTRA_MACHINE).orEmpty(),
                intent.getStringExtra(EXTRA_RUN).orEmpty(),
                intent.getIntExtra(EXTRA_PORT, 0),
                intent.getIntExtra(EXTRA_LOCAL, 0),
            )
        }
    }
}

/** `localhost:5173 → server`, as the window says a tunnel. */
fun Tunnel.said(): String = "localhost:$localPort → $machine"
