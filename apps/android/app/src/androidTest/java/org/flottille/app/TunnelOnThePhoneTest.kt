package org.flottille.app

import android.Manifest
import android.app.ActivityManager
import android.app.Notification
import android.app.NotificationManager
import android.content.Context
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.test.platform.app.InstrumentationRegistry
import java.time.ZoneOffset
import org.flottille.core.Conversation
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.PhoneView
import org.flottille.core.Project
import org.flottille.core.Run
import org.flottille.core.SessionRow
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test

/**
 * A run's page on the emulator (#831, `docs/remote.md`, "Tunnels", "On the phone"): the screens, the
 * web view, the foreground service and its notification are the real ones; the core is played by
 * [PlayedPhone] (a real listener on the phone's loopback whose connections go to a fake server that
 * stands for the run's port, and the machine's side ending the tunnel by hand).
 */
class TunnelOnThePhoneTest {
    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context: Context = instrumentation.targetContext
    private val server = FakeRunServer()
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val launched = java.util.Collections.synchronizedList(mutableListOf<String>())

    private lateinit var phone: PlayedPhone
    private val application get() = context.applicationContext as FlottilleApplication

    @Before
    fun a_phone_that_may_notify() {
        // Granted from outside, as the user's tap on the system's dialog would.
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
        manager.cancelAll()
        val page = Conversation(
            "server", "a1", "Start the app", "auto", false, false, false, false, emptyList(), emptyList(),
            listOf(Run("r1", "pnpm dev", "http://localhost:${server.port}/app")),
        )
        phone = PlayedPhone(
            server,
            PhoneView(
                role = "controller",
                name = "pixel",
                machines = listOf(Machine("server", MachineState.Online, 0)),
                projects = listOf(Project("app", listOf(SessionRow("server", "a1", "Start the app", "idle", 5)))),
                conversation = page,
            ),
        )
        application.phone = phone
    }

    @After
    fun nothing_is_left_running() {
        context.stopService(android.content.Intent(context, TunnelService::class.java))
        manager.cancelAll()
        server.close()
    }

    private fun start() {
        val host = SystemTunnelHost(rule.activity, launch = { launched += it; true })
        rule.setContent { FlottilleApp(phone = phone, deviceName = "pixel", zone = ZoneOffset.UTC, tunnels = host) }
        rule.onNodeWithTag("session:server:a1").performClick()
        rule.waitForIdle()
    }

    private fun until(what: String, seconds: Int = 20, detail: () -> String = { "" }, holds: () -> Boolean) {
        val limit = System.currentTimeMillis() + seconds * 1_000L
        while (!holds()) {
            // The screens' clock moves while the test waits for them: the in-app browser is faded in
            // by it, and a web view that is not drawn does not load.
            rule.waitForIdle()
            check(System.currentTimeMillis() < limit) { "$what within $seconds s ${detail()}" }
            Thread.sleep(100)
        }
    }

    private fun kept(): Notification? = manager.activeNotifications.firstOrNull { it.id == TunnelService.KEPT_ID }?.notification

    private fun serviceRuns(): Boolean =
        @Suppress("DEPRECATION")
        (context.getSystemService(ActivityManager::class.java).getRunningServices(100))
            .any { it.service.className == TunnelService::class.java.name }

    @Test
    fun a_run_s_page_opens_in_the_app_through_a_tunnel() {
        start()
        rule.onNodeWithTag("run-open:r1").performClick()
        // The web view asks the tunnel's local port; the run's server answers through it.
        until("the run's server was asked through the tunnel", seconds = 60, detail = {
            val failed = rule.onAllNodesWithTag("browser-failed").fetchSemanticsNodes().size
            val probe = phone.state.value.tunnels.firstOrNull()?.let { tunnel ->
                try {
                    java.net.URL("http://127.0.0.1:${tunnel.localPort}/probe").readText().take(40)
                } catch (broken: java.io.IOException) {
                    "probe failed: $broken"
                }
            }
            val webview = android.webkit.WebView.getCurrentWebViewPackage()?.let { "${it.packageName} ${it.versionName}" }
            "(probe: $probe, webview: $webview, requests: ${server.requests.toList()}, opened: ${phone.opened.toList()}, tunnels: ${phone.state.value.tunnels}, browser shown: " +
                "${rule.onAllNodesWithTag("browser-page").fetchSemanticsNodes().size}, failed message nodes: $failed, " +
                "texts: ${rule.onAllNodesWithTag("browser-failed").fetchSemanticsNodes().map { it.config.toString() }})"
        }) { server.requests.isNotEmpty() }
        assertEquals(listOf(Triple("server", "r1", "http://localhost:${server.port}/app")), phone.opened)
        val request = server.requests.first()
        assertTrue(request, request.startsWith("GET /app "))
        assertTrue("the app's web view asked: $request", request.contains("Mobile Safari") || request.contains("Chrome"))
        rule.onNodeWithTag("browser-page").assertIsDisplayed()
        // Nothing was handed to another app, and no service runs for a page in the app.
        assertTrue(launched.isEmpty())
        assertFalse(serviceRuns())
    }

    @Test
    fun closing_the_in_app_browser_stops_its_listener() {
        start()
        rule.onNodeWithTag("run-open:r1").performClick()
        until("the tunnel opened") { phone.state.value.tunnels.isNotEmpty() }
        val tunnel = phone.state.value.tunnels.single()
        assertTrue(phone.listening(tunnel))
        rule.onNodeWithTag("browser-close").performClick()
        // Another app of the phone could reach a listener left open: it goes with the page.
        until("the tunnel closed") { phone.closed.isNotEmpty() }
        assertEquals(listOf(tunnel.tunnel), phone.closed.toList())
        until("nothing listens any more") { !phone.listening(tunnel) }
    }

    @Test
    fun open_in_browser_keeps_the_tunnel_with_a_foreground_service() {
        start()
        rule.onNodeWithTag("run-browser:r1").performClick()
        until("the service's notification") { kept() != null }
        // The user's browser is given the local address, the tunnel's.
        val tunnel = phone.state.value.tunnels.single()
        assertEquals(listOf("http://localhost:${tunnel.localPort}/app"), launched.toList())
        val notification = kept()!!
        assertEquals("Close", notification.actions.single().title.toString())
        assertTrue(notification.flags and Notification.FLAG_ONGOING_EVENT != 0)
        assertTrue(serviceRuns())
        // The connection is held for the service, and the tunnel listens with the app in the background.
        assertEquals(1, phone.holds)
        assertTrue(phone.listening(tunnel))
    }

    @Test
    fun close_from_the_notification_ends_the_tunnel() {
        start()
        rule.onNodeWithTag("run-browser:r1").performClick()
        until("the service's notification") { kept() != null }
        val tunnel = phone.state.value.tunnels.single()
        kept()!!.actions.single { it.title == "Close" }.actionIntent.send()
        until("the tunnel closed") { phone.closed.isNotEmpty() }
        assertEquals(listOf(tunnel.tunnel), phone.closed.toList())
        until("the service to end") { !serviceRuns() && kept() == null }
        assertFalse(phone.listening(tunnel))
        assertEquals(0, phone.holds)
        // The user asked for it: no notice of an end.
        assertTrue(manager.activeNotifications.none { it.id >= TunnelService.ENDED_ID })
    }

    @Test
    fun the_tunnel_ends_with_its_run() {
        start()
        rule.onNodeWithTag("run-browser:r1").performClick()
        until("the service's notification") { kept() != null }
        val tunnel = phone.state.value.tunnels.single()
        phone.endTunnel(tunnel.tunnel, "run r1 ended")
        until("the service to end") { !serviceRuns() && kept() == null }
        // It says so: a notice of its own, which stays.
        until("the notice") { manager.activeNotifications.any { it.id >= TunnelService.ENDED_ID } }
        val notice = manager.activeNotifications.first { it.id >= TunnelService.ENDED_ID }.notification
        val text = notice.extras.getCharSequence(Notification.EXTRA_TEXT).toString()
        assertTrue(text, text.contains("run r1 ended"))
        assertFalse(phone.listening(tunnel))
        assertEquals(0, phone.holds)
    }
}
