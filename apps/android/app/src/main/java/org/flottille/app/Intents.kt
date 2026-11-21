package org.flottille.app

import android.content.ComponentName
import android.content.Intent

/**
 * The entry point of the app's own PendingIntents (a notification's, #828, #831): an alias of the
 * main activity that is not exported, so that only the app reaches it. A session is opened by an
 * intent that came through it, and by none other.
 */
const val OPEN_SESSION = "org.flottille.app.OpenSession"

/** What an intent asks of the app: a session to open, pictures to place. */
class Asked(val open: SessionKey?, val share: Boolean)

/**
 * What [intent] asks. The main activity is exported (the launcher, the share sheet), so any app
 * can send it an intent with any extra: the machine and the agent are honoured only when the
 * intent came through the app's own, unexported [OPEN_SESSION]. The share of pictures is read
 * once: an activity recreated (a rotation) holds the same intent again and does not read it again.
 */
fun asked(intent: Intent, packageName: String, recreated: Boolean): Asked {
    val machine = intent.getStringExtra(MainActivity.EXTRA_MACHINE)
    val agent = intent.getStringExtra(MainActivity.EXTRA_AGENT)
    val own = intent.component == ComponentName(packageName, OPEN_SESSION)
    val open = if (own && machine != null && agent != null) SessionKey(machine, agent) else null
    val sharing = intent.action == Intent.ACTION_SEND || intent.action == Intent.ACTION_SEND_MULTIPLE
    return Asked(open, share = sharing && !recreated)
}
