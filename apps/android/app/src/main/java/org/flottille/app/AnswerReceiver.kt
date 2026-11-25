package org.flottille.app

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout
import org.flottille.core.MachineState
import org.flottille.core.Notice
import org.flottille.core.PhoneException

/**
 * Allow or Deny from a notification, with the app killed and the phone asleep (`docs/remote.md`,
 * "Notifications"): the connection is held while the answer goes, the machine is waited for until
 * it is linked, and the answer is signed and sent within the seconds Android gives a receiver. Not
 * exported: only the app's own notifications reach it.
 */
class AnswerReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val machine = intent.getStringExtra(EXTRA_MACHINE) ?: return
        val number = intent.getLongExtra(EXTRA_NUMBER, 0)
        val notification = intent.getLongExtra(EXTRA_NOTIFICATION, 0)
        val allow = intent.getBooleanExtra(EXTRA_ALLOW, false)
        val app = context.applicationContext as FlottilleApplication
        val phone = app.phone
        // The service comes back with the process, and keeps the phone reachable after the answer.
        HubService.start(context)
        val pending = goAsync()
        CoroutineScope(Dispatchers.Default).launch {
            val hold = phone.keepConnected()
            try {
                // The answer within the wait too: past the seconds Android gives, the process may go.
                withTimeout(ANSWER_WAIT) {
                    phone.views().first { view -> view.machines.any { it.name == machine && it.state == MachineState.Online } }
                    phone.answer(machine, number, allow)
                }
                app.notices.handled(machine, notification)
            } catch (refused: PhoneException) {
                // Decided already, elsewhere: nothing waits any more.
                if (refused.gone) app.notices.handled(machine, notification) else app.notices.failed(machine, notification, refused.why, phone.view)
            } catch (late: TimeoutCancellationException) {
                app.notices.failed(machine, notification, "$machine could not be reached", phone.view)
            } finally {
                hold.close()
                pending.finish()
            }
        }
    }

    companion object {
        /** Less than the ten seconds Android gives a receiver that went on in the background. */
        private const val ANSWER_WAIT = 8_000L
        private const val ACTION_ANSWER = "org.flottille.app.ANSWER"
        private const val EXTRA_MACHINE = "org.flottille.app.ANSWER_MACHINE"
        private const val EXTRA_NUMBER = "org.flottille.app.ANSWER_NUMBER"
        private const val EXTRA_NOTIFICATION = "org.flottille.app.ANSWER_NOTIFICATION"
        private const val EXTRA_ALLOW = "org.flottille.app.ANSWER_ALLOW"

        fun pending(context: Context, notice: Notice, allow: Boolean): PendingIntent =
            PendingIntent.getBroadcast(
                context,
                0,
                Intent(context, AnswerReceiver::class.java)
                    .setAction(ACTION_ANSWER)
                    .setData(noticeUri(notice, if (allow) "allow" else "deny"))
                    .putExtra(EXTRA_MACHINE, notice.machine)
                    .putExtra(EXTRA_NUMBER, notice.request)
                    .putExtra(EXTRA_NOTIFICATION, notice.notification)
                    .putExtra(EXTRA_ALLOW, allow),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
    }
}
