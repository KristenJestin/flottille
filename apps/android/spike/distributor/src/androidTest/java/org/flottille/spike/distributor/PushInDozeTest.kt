package org.flottille.spike.distributor

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import java.net.HttpURLConnection
import java.net.URL
import java.util.regex.Pattern
import org.json.JSONArray
import org.json.JSONObject
import org.junit.AfterClass
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.BeforeClass
import org.junit.FixMethodOrder
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.MethodSorters

/**
 * The proof of #823 on an emulator: the app joins through the hub as a controller and registers
 * its endpoint; it is killed; the phone is forced into Doze; a request of the fake agent is held
 * and pushed; the notification shows Allow and Deny; Allow is answered from it.
 *
 * Run by `ci.sh` with the spike's server on the host (`-e gate http://10.0.2.2:7470`); the tests
 * run in order, each on what the one before left.
 */
@RunWith(AndroidJUnit4::class)
@FixMethodOrder(MethodSorters.NAME_ASCENDING)
class PushInDozeTest {
    companion object {
        const val APP = "org.flottille.spike"
        private lateinit var relay: Relay
        private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
        private val device get() = UiDevice.getInstance(instrumentation)
        private val gate get() = InstrumentationRegistry.getArguments().getString("gate", "http://10.0.2.2:7470")

        fun shell(command: String): String = device.executeShellCommand(command).trim().also {
            Log.i(Distributor.TAG, "$ $command\n$it")
        }

        fun http(method: String, path: String, timeoutMs: Int = 40_000): JSONObject {
            val connection = URL(gate + path).openConnection() as HttpURLConnection
            connection.requestMethod = method
            connection.connectTimeout = 5_000
            connection.readTimeout = timeoutMs
            if (method == "POST") {
                connection.doOutput = true
                connection.outputStream.use { it.write("{}".toByteArray()) }
            }
            val code = connection.responseCode
            val text = (if (code < 400) connection.inputStream else connection.errorStream).bufferedReader().readText()
            assertTrue("$path answered $code: $text", code < 400)
            return JSONObject(text)
        }

        fun events(): List<JSONObject> {
            val events: JSONArray = http("GET", "/state").getJSONArray("events")
            return (0 until events.length()).map { events.getJSONObject(it) }
        }

        fun eventually(what: String, seconds: Int, check: () -> Boolean) {
            val deadline = System.currentTimeMillis() + seconds * 1000L
            while (!check()) {
                assertTrue("$what did not happen in $seconds s", System.currentTimeMillis() < deadline)
                Thread.sleep(250)
            }
        }

        @BeforeClass
        @JvmStatic
        fun relayPushes() {
            val context = instrumentation.targetContext
            Distributor.prefs(context).edit().clear().putString("server", gate).commit()
            // The distributor is let out of Doze, as the user lets ntfy: the app is not.
            shell("dumpsys deviceidle whitelist +${context.packageName}")
            shell("pm grant $APP android.permission.POST_NOTIFICATIONS")
            relay = Relay(context).also { it.start() }
        }

        @AfterClass
        @JvmStatic
        fun leaveDoze() {
            relay.stopped = true
            shell("dumpsys deviceidle unforce")
            shell("dumpsys battery reset")
        }
    }

    @Test
    fun a_theAppJoinsAsAControllerAndRegistersItsEndpoint() {
        val code = http("GET", "/invitation").getString("code")
        shell("am start -W -n $APP/.MainActivity --es code $code --es gate $gate")
        eventually("the join's acceptance", 60) { events().any { it.has("accepted") } }
        val accepted = events().first { it.has("accepted") }
        val words = accepted.getJSONArray("words").let { list -> (0 until list.length()).map { list.getString(it) } }
        assertEquals("controller", accepted.getString("role"))
        // Both sides show the same four words.
        assertNotNull(
            "the app does not show the words ${words.joinToString(" ")}",
            device.wait(Until.findObject(By.textContains("Words: ${words.joinToString(" ")}")), 20_000),
        )
        eventually("push.register at the runner", 60) { http("GET", "/state").getBoolean("registered") }
        val registered = events().first { it.has("registered") }
        assertEquals("controller", registered.getString("role"))
        assertTrue(registered.getString("endpoint").startsWith("$gate/push/"))
    }

    @Test
    fun b_allowFromTheNotificationAnswersTheRequestWithTheAppKilledInDoze() {
        device.pressHome()
        Thread.sleep(1_000)
        shell("am kill $APP")
        eventually("the app's process to end", 10) { shell("pidof $APP").isEmpty() }
        shell("dumpsys battery unplug")
        // The emulator's image ships with deep Doze turned off; a phone has it on.
        shell("dumpsys deviceidle enable deep")
        assertTrue(shell("dumpsys deviceidle force-idle").contains("deep idle", ignoreCase = true))
        assertEquals("IDLE", shell("dumpsys deviceidle get deep"))

        val held = http("POST", "/hold")
        val number = held.getLong("number")
        assertTrue("the push server refused it: $held", held.getInt("status") in 200..299)

        device.openNotification()
        val allow = device.wait(Until.findObject(By.text(Pattern.compile("(?i)allow"))), 30_000)
        assertNotNull("no notification with Allow came for #$number", allow)
        assertNotNull("no Deny beside Allow", device.findObject(By.text(Pattern.compile("(?i)deny"))))
        assertTrue("the push did not start the app's process", shell("pidof $APP").isNotEmpty())
        assertEquals("the phone left Doze before the tap", "IDLE", shell("dumpsys deviceidle get deep"))
        allow!!.click()

        // Answered within the receiver's time: goAsync gives 10 s to a broadcast in the foreground.
        eventually("the answer of #$number", 10) {
            events().any { it.optString("call") == "approval.approve" && it.optJSONObject("params")?.optLong("number") == number && it.optBoolean("answered") }
        }
        eventually("the phone's report of #$number", 10) { events().any { it.has("report") } }
        val report = events().last { it.has("report") }.getJSONObject("report")
        Log.i(Distributor.TAG, "report: $report")
        assertTrue("the answer failed: $report", report.getBoolean("answered"))
        assertTrue("the phone was not in Doze at the push: $report", report.getBoolean("idleAtPush"))
        // The tap on the notification's action takes the phone out of Doze itself (idleAtTap is
        // false, the deep state ACTIVE after it): what is said here, not asserted.
        Log.i(Distributor.TAG, "after the tap: ${shell("dumpsys deviceidle get deep")}")
        device.pressHome()
    }
}
