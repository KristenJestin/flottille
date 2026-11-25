package org.flottille.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.flottille.core.Notice
import org.flottille.core.Phone
import org.flottille.core.PhoneView

/**
 * Keeps the phone's connection to its hub while it is paired (`docs/remote.md`, "Notifications"):
 * no push service, the hub tells the phone over that connection when an agent needs the user. A
 * foreground service, whose permanent notification Android requires: silent, which the user may
 * hide, saying "Flottille is connected" or, when the hub cannot be reached, "Not connected to your
 * hub" (the core dials it again, with a backoff). What the runners tell is drawn by [Notices].
 */
class HubService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var hold: AutoCloseable? = null
    private var watching: Job? = null
    private var said: String? = null

    private val app get() = application as FlottilleApplication
    private val phone: Phone get() = app.phone

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        channels(this)
        ServiceCompat.startForeground(
            this,
            CONNECTED_ID,
            connected(phone.view.hub),
            if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0,
        )
        if (hold == null) hold = phone.keepConnected()
        if (watching == null) watching = scope.launch { watch() }
        // Killed by the system, it is started again: the phone stays reachable.
        return START_STICKY
    }

    private suspend fun watch() {
        phone.views().collect { view ->
            // Removed from the user's list: nothing to keep connected to.
            if (!view.joined) {
                stop()
                return@collect
            }
            app.notices.show(view)
            if (view.hub != said) {
                getSystemService(NotificationManager::class.java).notify(CONNECTED_ID, connected(view.hub))
            }
        }
    }

    private fun connected(hub: String): Notification {
        said = hub
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_CONNECTED)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle(if (hub.isEmpty()) "Flottille is connected" else "Not connected to your hub")
            .apply { if (hub.isNotEmpty()) setContentText(hub) }
            .setContentIntent(open)
            .setOngoing(true)
            .setSilent(true)
            .setOnlyAlertOnce(true)
            .build()
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
        hold = null
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        const val CHANNEL_CONNECTED = "hub-connected"
        const val CHANNEL_NEEDS_YOU = "needs-you"
        const val CONNECTED_ID = 8280

        /** Starts the service, or tells the one that runs: once the phone joined, at boot, at an answer. */
        fun start(context: Context) {
            ContextCompat.startForegroundService(context, Intent(context, HubService::class.java))
        }

        fun channels(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_CONNECTED, "Connection to your hub", NotificationManager.IMPORTANCE_MIN).apply {
                    setShowBadge(false)
                },
            )
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_NEEDS_YOU, "What your agents tell you", NotificationManager.IMPORTANCE_HIGH),
            )
        }
    }
}

/** At boot, and once the app was updated: a paired phone keeps its connection to the hub again. */
class StartReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val app = context.applicationContext as FlottilleApplication
        if (app.phone.view.joined) HubService.start(context)
    }
}

/**
 * What the runners told, drawn once each, for the life of the process (`docs/remote.md`,
 * "Notifications"): a request with Allow and Deny (Deny only on a proposal to write a connection,
 * neither on a `viewer`), a question, a report or a failure that opens its session. A dismissed one
 * is removed; one answered from its notification, swiped away or tapped is not drawn again. Allow
 * and Deny need the phone unlocked, and the lock screen shows no excerpt.
 */
class Notices(private val context: Context) {
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val drawn = mutableMapOf<String, Notice>()
    private val done = mutableSetOf<String>()

    @Synchronized
    fun show(view: PhoneView) {
        HubService.channels(context)
        view.dismissed.forEach { handled(it.machine, it.notification) }
        view.notices.forEach { notice ->
            val key = key(notice.machine, notice.notification)
            // Once: drawn again, one the user swiped away or tapped would ring a second time.
            if (key in done || key in drawn) return@forEach
            drawn[key] = notice
            manager.notify(notice.machine, notice.notification.toInt(), build(notice, view, said = null))
        }
    }

    /** Answered, opened or dismissed: removed, and not drawn again. */
    @Synchronized
    fun handled(machine: String, notification: Long) {
        if (done.add(key(machine, notification))) manager.cancel(machine, notification.toInt())
    }

    /** Allow or Deny did not reach the machine: the notification says why and opens the session. */
    @Synchronized
    fun failed(machine: String, notification: Long, why: String, view: PhoneView) {
        val notice = drawn[key(machine, notification)] ?: return
        manager.notify(machine, notification.toInt(), build(notice, view, said = "Not answered: $why"))
    }

    private fun build(notice: Notice, view: PhoneView, said: String?): Notification {
        val title = notice.title.ifEmpty {
            when (notice.kind) {
                "request" -> "A request waits for you"
                "question" -> "Asks you a question"
                "report" -> "Reported"
                else -> "Failed"
            }
        }
        val builder = NotificationCompat.Builder(context, HubService.CHANNEL_NEEDS_YOU)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle(title)
            .setContentText(said ?: listOf(notice.machine, notice.excerpt).filter { it.isNotEmpty() }.joinToString(" · "))
            .setContentIntent(opening(notice))
            .setAutoCancel(true)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(
                NotificationCompat.Builder(context, HubService.CHANNEL_NEEDS_YOU)
                    .setSmallIcon(android.R.drawable.stat_notify_sync)
                    .setContentTitle(title)
                    .setContentText(notice.machine)
                    .build(),
            )
            .setOnlyAlertOnce(true)
            .setCategory(if (notice.kind == "request" || notice.kind == "question") NotificationCompat.CATEGORY_REMINDER else NotificationCompat.CATEGORY_STATUS)
        val answerable = said == null && notice.kind == "request" && notice.request > 0 && view.role != "viewer"
        if (answerable) {
            val proposal = view.requests.any { it.machine == notice.machine && it.number == notice.request && it.proposesAConnection }
            // A proposal to write a connection is read on a computer before it is allowed, as in the window.
            if (!proposal) builder.addAction(unlocked("Allow", AnswerReceiver.pending(context, notice, allow = true)))
            builder.addAction(unlocked("Deny", AnswerReceiver.pending(context, notice, allow = false)))
        }
        return builder.build()
    }

    /** An answer that a phone left on a table does not give: Android asks for its unlock first. */
    private fun unlocked(title: String, answer: PendingIntent) =
        NotificationCompat.Action.Builder(0, title, answer).setAuthenticationRequired(true).build()

    /** Its session's page, through the app's own entry point; the app, for what is no session's. */
    private fun opening(notice: Notice): PendingIntent {
        val intent = if (notice.session.isEmpty()) {
            Intent(context, MainActivity::class.java)
        } else {
            Intent()
                .setComponent(ComponentName(context.packageName, OPEN_SESSION))
                .putExtra(MainActivity.EXTRA_MACHINE, notice.machine)
                .putExtra(MainActivity.EXTRA_AGENT, notice.session)
        }
        return PendingIntent.getActivity(
            context,
            0,
            intent.setData(noticeUri(notice, "open")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }

    private fun key(machine: String, notification: Long) = "$machine:$notification"
}

/**
 * What tells one notice's intents apart (`what` is `open`, `allow` or `deny`): Android keeps one
 * pending intent per intent and request code, and a code made from a hash could be another
 * notice's.
 */
fun noticeUri(notice: Notice, what: String): Uri = Uri.Builder()
    .scheme("flottille")
    .authority("notice")
    .appendPath(notice.machine)
    .appendPath(notice.notification.toString())
    .appendPath(what)
    .build()
