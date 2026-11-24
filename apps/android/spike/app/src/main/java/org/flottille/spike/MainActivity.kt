package org.flottille.spike

import android.app.Activity
import android.content.SharedPreferences
import android.os.Bundle
import android.widget.TextView

/**
 * The spike's one screen: what it did, as text. Started with the invitation's code and the
 * gate's address (`adb shell am start -n org.flottille.spike/.MainActivity --es code <code>
 * --es gate http://<address>:7470`), it joins through the hub, then registers with the
 * distributor installed and gives the runner its endpoint.
 */
class MainActivity : Activity() {
    private lateinit var view: TextView
    private val changed = SharedPreferences.OnSharedPreferenceChangeListener { _, _ -> runOnUiThread { draw() } }

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        view = TextView(this).apply { setPadding(32, 96, 32, 32); textSize = 16f }
        setContentView(view)
        val notifications = android.Manifest.permission.POST_NOTIFICATIONS
        if (checkSelfPermission(notifications) != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(notifications), 1)
        }
        val code = intent.getStringExtra("code")
        val gate = intent.getStringExtra("gate")
        if (code != null && gate != null) {
            Store.prefs(this).edit().clear().putString("gate", gate.trimEnd('/')).commit()
            Thread { join(code) }.start()
        }
    }

    private fun join(code: String) {
        val context = applicationContext
        try {
            val invitation = Invitation.decode(code)
            val words = invitation.words(context, Keys.devicePublic(context))
            Store.prefs(context).edit().putString("words", words.joinToString(" ")).commit()
            Store.status(context, "joining ${invitation.hubUrl}: ${words.joinToString(" ")}")
            Hub.join(context, invitation)
            Store.status(context, "joined as a controller")
            val distributors = Push.distributors(context)
            if (distributors.isEmpty()) {
                Store.status(context, "joined, but no UnifiedPush distributor is installed")
                return
            }
            Push.register(context, distributors.first())
        } catch (error: Exception) {
            Store.status(context, "the join failed: ${error.message}")
        }
    }

    override fun onResume() {
        super.onResume()
        Store.prefs(this).registerOnSharedPreferenceChangeListener(changed)
        draw()
    }

    override fun onPause() {
        Store.prefs(this).unregisterOnSharedPreferenceChangeListener(changed)
        super.onPause()
    }

    private fun draw() {
        val prefs = Store.prefs(this)
        view.text = buildString {
            appendLine("Flottille push spike")
            appendLine()
            appendLine("Device ${Keys.devicePublic(this@MainActivity).hex().take(16)}")
            prefs.getString("words", null)?.let { appendLine("Words: $it") }
            appendLine("Status: ${prefs.getString("status", "not joined")}")
            appendLine()
            append(prefs.getString("log", ""))
        }
    }
}
