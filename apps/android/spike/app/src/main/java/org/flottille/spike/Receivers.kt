package org.flottille.spike

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.PowerManager
import android.os.SystemClock
import androidx.work.Data
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject

/** The request's notification: drawn from the push alone, Allow and Deny on it. */
object Notifications {
    private const val CHANNEL = "requests"

    fun show(context: Context, number: Long, text: String, actions: Boolean = true) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Requests", NotificationManager.IMPORTANCE_HIGH),
        )
        fun answer(allow: Boolean): PendingIntent = PendingIntent.getBroadcast(
            context,
            (number * 2 + if (allow) 1 else 0).toInt(),
            Intent(context, AnswerReceiver::class.java).putExtra("number", number).putExtra("allow", allow),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val builder = android.app.Notification.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle("Request #$number")
            .setContentText(text)
            .setCategory(android.app.Notification.CATEGORY_MESSAGE)
            .setOnlyAlertOnce(true)
        if (actions) {
            builder.addAction(android.app.Notification.Action.Builder(null, "Allow", answer(true)).build())
            builder.addAction(android.app.Notification.Action.Builder(null, "Deny", answer(false)).build())
        }
        manager.notify(number.toInt(), builder.build())
    }

    fun cancel(context: Context, number: Long) =
        context.getSystemService(NotificationManager::class.java).cancel(number.toInt())
}

fun idle(context: Context): Boolean = context.getSystemService(PowerManager::class.java).isDeviceIdleMode

/** What a distributor sends: the endpoint, then the pushes. */
class PushReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val prefs = Store.prefs(context)
        if (intent.getStringExtra("token") != prefs.getString("token", null)) {
            Store.log(context, "a push intent for another token: ignored")
            return
        }
        when (intent.action) {
            "org.unifiedpush.android.connector.NEW_ENDPOINT" -> {
                val endpoint = intent.getStringExtra("endpoint") ?: return
                prefs.edit().putString("endpoint", endpoint).commit()
                Store.log(context, "endpoint $endpoint")
                val pending = goAsync()
                Thread {
                    try {
                        val params = Push.subscription(context, endpoint)
                        Gate.post(context, "/register", Gate.signed(context, "push.register", params))
                        Store.status(context, "registered: the runner pushes to ${android.net.Uri.parse(endpoint).host}")
                    } catch (error: Exception) {
                        Store.status(context, "push.register failed: ${error.message}")
                    } finally {
                        pending.finish()
                    }
                }.start()
            }
            "org.unifiedpush.android.connector.MESSAGE" -> {
                val received = System.currentTimeMillis()
                val bytes = intent.getByteArrayExtra("bytesMessage")
                    ?: intent.getStringExtra("message")?.toByteArray()
                    ?: return
                Push.ack(context, intent.getStringExtra("id"))
                val message = try {
                    JSONObject(String(Push.decrypt(context, bytes)))
                } catch (error: Exception) {
                    Store.log(context, "a push that does not decrypt: ${error.message}")
                    return
                }
                val number = message.optLong("id")
                if (message.optString("kind") != "request") return
                // Drawn at once from the kind and the id: the details come if the network lets.
                Notifications.show(context, number, "An agent asks for something")
                val notified = System.currentTimeMillis()
                prefs.edit()
                    .putLong("t.$number.received", received)
                    .putLong("t.$number.notified", notified)
                    .putBoolean("t.$number.idle", idle(context))
                    .commit()
                Store.log(context, "push for request #$number, idle ${idle(context)}, drawn in ${notified - received} ms")
                val read = OneTimeWorkRequestBuilder<ReadWorker>()
                    .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
                    .setInputData(Data.Builder().putLong("number", number).build())
                    .build()
                WorkManager.getInstance(context).enqueueUniqueWork("read-$number", ExistingWorkPolicy.REPLACE, read)
            }
            "org.unifiedpush.android.connector.UNREGISTERED",
            "org.unifiedpush.android.connector.REGISTRATION_FAILED",
            -> Store.status(context, "the distributor said ${intent.action?.substringAfterLast('.')}")
        }
    }
}

/**
 * Reads the request once the push woke the app, in an expedited job: whether the network is
 * there in Doze for it is what this finds out. The notification is up already either way.
 */
class ReadWorker(context: Context, parameters: WorkerParameters) : Worker(context, parameters) {
    override fun doWork(): Result {
        val number = inputData.getLong("number", 0)
        val started = SystemClock.elapsedRealtime()
        val prefs = Store.prefs(applicationContext)
        try {
            val answer = Gate.call(applicationContext, "approval.list", JSONObject(), 8_000)
            val requests = answer.getJSONObject("answer").getJSONArray("requests")
            val request = (0 until requests.length()).map { requests.getJSONObject(it) }
                .firstOrNull { it.optLong("number") == number }
            val ms = SystemClock.elapsedRealtime() - started
            prefs.edit().putBoolean("t.$number.read", true).putLong("t.$number.readMs", ms)
                .putBoolean("t.$number.readIdle", idle(applicationContext)).commit()
            if (request != null && !prefs.getBoolean("t.$number.answered", false)) {
                Notifications.show(applicationContext, number, "${request.optString("tool")}: ${request.opt("call")}")
            }
            Store.log(applicationContext, "read #$number in $ms ms, idle ${idle(applicationContext)}")
        } catch (error: Exception) {
            prefs.edit().putBoolean("t.$number.read", false)
                .putString("t.$number.readError", error.toString()).commit()
            Store.log(applicationContext, "read #$number failed: $error")
        }
        return Result.success()
    }
}

/**
 * Allow or Deny, tapped on the notification: signed and answered within the receiver's time
 * (goAsync), the notification's own tap letting the app out of Doze for a few seconds.
 */
class AnswerReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val tapped = System.currentTimeMillis()
        val tappedClock = SystemClock.elapsedRealtime()
        val number = intent.getLongExtra("number", 0)
        val allow = intent.getBooleanExtra("allow", false)
        val idleAtTap = idle(context)
        val pending = goAsync()
        Thread {
            val prefs = Store.prefs(context)
            val report = JSONObject()
                .put("number", number)
                .put("action", if (allow) "allow" else "deny")
                .put("received", prefs.getLong("t.$number.received", 0))
                .put("notified", prefs.getLong("t.$number.notified", 0))
                .put("idleAtPush", prefs.getBoolean("t.$number.idle", false))
                .put("read", if (prefs.contains("t.$number.read")) prefs.getBoolean("t.$number.read", false) else JSONObject.NULL)
                .put("readMs", prefs.getLong("t.$number.readMs", -1))
                .put("readError", prefs.getString("t.$number.readError", null) ?: JSONObject.NULL)
                .put("tapped", tapped)
                .put("idleAtTap", idleAtTap)
            try {
                val method = if (allow) "approval.approve" else "approval.deny"
                val params = JSONObject().put("number", number)
                if (allow) params.put("discard", false)
                Gate.call(context, method, params)
                val answeredIn = SystemClock.elapsedRealtime() - tappedClock
                prefs.edit().putBoolean("t.$number.answered", true).commit()
                report.put("answered", true).put("answerMs", answeredIn)
                Notifications.cancel(context, number)
                Store.log(context, "#$number ${if (allow) "allowed" else "denied"} in $answeredIn ms, idle $idleAtTap")
            } catch (error: Exception) {
                report.put("answered", false).put("error", error.toString())
                    .put("answerMs", SystemClock.elapsedRealtime() - tappedClock)
                Notifications.show(context, number, "Not answered: ${error.message}", actions = true)
                Store.log(context, "#$number not answered: $error")
            }
            try {
                Gate.post(context, "/report", report.toString(), 3_000)
            } catch (error: Exception) {
                Store.log(context, "report of #$number not sent: $error")
            } finally {
                pending.finish()
            }
        }.start()
    }
}
