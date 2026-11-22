package org.flottille.app

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertDoesNotExist
import androidx.compose.ui.test.hasAnyDescendant
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import java.time.Instant
import java.time.ZoneOffset
import org.flottille.core.Conversation
import org.flottille.core.Entry
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.OpenedTunnel
import org.flottille.core.PhoneView
import org.flottille.core.Project
import org.flottille.core.Run
import org.flottille.core.SessionRow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** What the system is asked for a run's page, kept. */
class FakeTunnelHost : TunnelHost {
    val kept = mutableListOf<OpenedTunnel>()
    val browsed = mutableListOf<String>()

    override fun keepForBrowser(opened: OpenedTunnel) {
        kept += opened
    }

    override fun browse(url: String) {
        browsed += url
    }
}

/**
 * A run's page and the links an agent shows (#831, `docs/remote.md`, "Tunnels", "On the phone"),
 * against a fake core: Open reads the address through a tunnel in the app, Open in browser hands it
 * to the user's browser and keeps the tunnel (the system's side is the instrumentation tests'),
 * a page of the web opens as it is, a viewer has no tunnel to open.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w360dp-h900dp-night-xhdpi", application = android.app.Application::class)
class TunnelPageTest {
    @get:Rule
    val rule = createComposeRule()

    private val since = Instant.parse("2026-11-21T22:14:00Z").toEpochMilli()
    private val host = FakeTunnelHost()
    private val dev = Run("r1", "pnpm dev", "http://localhost:5173/app")

    private fun phone(
        runs: List<Run> = listOf(dev),
        entries: List<Entry> = listOf(Entry(1, "user", "Start the app")),
        role: String = "controller",
    ): FakePhone {
        val phone = FakePhone(
            PhoneView(
                role = role,
                name = "pixel",
                machines = listOf(Machine("server", MachineState.Online, since)),
                projects = listOf(Project("app", listOf(SessionRow("server", "a1", "Fix the login", "idle", 5)))),
            ),
        )
        phone.pages["a1"] = Conversation("server", "a1", "Fix the login", "auto", false, false, false, false, entries, emptyList(), runs)
        return phone
    }

    private fun start(phone: FakePhone) {
        rule.setContent {
            FlottilleApp(phone = phone, deviceName = "pixel", zone = ZoneOffset.UTC, tunnels = host)
        }
        rule.onNodeWithTag("session:server:a1").performClick()
        rule.waitForIdle()
    }

    /** What the node tagged [tag] says, in a text of its own. */
    private fun says(tag: String, text: String) {
        rule.onNode(hasTestTag(tag) and hasAnyDescendant(hasText(text, substring = true))).assertExists()
    }

    @Test
    fun a_run_s_page_opens_in_the_app_through_a_tunnel() {
        val phone = phone()
        start(phone)
        rule.onNodeWithTag("run:r1").assertIsDisplayed()
        rule.onNodeWithTag("run-open:r1").performClick()
        rule.waitForIdle()
        // The core was asked for the run's own address, then the web view reads the address through the tunnel.
        assertEquals(listOf(Triple("server", "r1", "http://localhost:5173/app")), phone.opened)
        rule.onNodeWithTag("browser").assertIsDisplayed()
        rule.onNodeWithTag("browser-page").assertIsDisplayed()
        // The tunnel is said in the run's row, with Close; closing the browser leaves it to the row.
        says("tunnel:r1", "localhost:41001 → server")
        rule.onNodeWithTag("browser-close").performClick()
        rule.waitForIdle()
        rule.onNodeWithTag("browser-page").assertDoesNotExist()
        assertTrue(host.kept.isEmpty() && host.browsed.isEmpty())
    }

    @Test
    fun open_in_browser_hands_the_local_address_to_the_system_and_keeps_the_tunnel() {
        val phone = phone()
        start(phone)
        rule.onNodeWithTag("run-browser:r1").performClick()
        rule.waitForIdle()
        assertEquals(1, host.kept.size)
        assertEquals("http://localhost:41001/app", host.kept.single().url)
        assertEquals("t1", host.kept.single().tunnel.tunnel)
        // The in-app browser stays closed: the page is the user's browser's.
        rule.onNodeWithTag("browser-page").assertDoesNotExist()
    }

    @Test
    fun a_link_an_agent_shows_opens_like_a_run_s_address() {
        val shown = Entry(2, "link", "The app", emptyList(), "http://localhost:5173/")
        val phone = phone(runs = emptyList(), entries = listOf(Entry(1, "user", "Start the app"), shown))
        start(phone)
        rule.onNodeWithTag("link:2").assertIsDisplayed()
        rule.onNodeWithTag("link-open:2").performClick()
        rule.waitForIdle()
        // A link names no run: the core takes the one that serves its port.
        assertEquals(listOf(Triple("server", null, "http://localhost:5173/")), phone.opened)
        rule.onNodeWithTag("browser-page").assertIsDisplayed()
    }

    @Test
    fun a_page_of_the_web_opens_as_it_is() {
        val shown = Entry(2, "link", "The docs", emptyList(), "https://example.org/docs")
        start(phone(runs = emptyList(), entries = listOf(shown)))
        rule.onNodeWithTag("link-browser:2").assertDoesNotExist()
        rule.onNodeWithTag("link-open:2").performClick()
        rule.waitForIdle()
        assertEquals(listOf("https://example.org/docs"), host.browsed)
        rule.onNodeWithTag("browser-page").assertDoesNotExist()
    }

    @Test
    fun a_viewer_s_page_draws_no_tunnel() {
        val shown = Entry(2, "link", "The app", emptyList(), "http://localhost:5173/")
        start(phone(role = "viewer", entries = listOf(shown)))
        rule.onNodeWithTag("run:r1").assertDoesNotExist()
        rule.onNodeWithTag("link-open:2").assertDoesNotExist()
        rule.onNodeWithTag("link-browser:2").assertDoesNotExist()
    }

    @Test
    fun a_tunnel_the_machine_refuses_is_said_and_opens_nothing() {
        val phone = phone()
        phone.refusals["openAddress"] = "refused: port 5173 is none of run r1's"
        start(phone)
        rule.onNodeWithTag("run-open:r1").performClick()
        rule.waitForIdle()
        says("toast", "Not opened: refused: port 5173 is none of run r1's")
        rule.onNodeWithTag("browser-page").assertDoesNotExist()
        // The button is there to be tried again.
        rule.onNodeWithTag("run-open:r1").assertIsDisplayed()
    }

    @Test
    fun the_browser_says_when_its_tunnel_ends() {
        val phone = phone()
        start(phone)
        rule.onNodeWithTag("run-open:r1").performClick()
        rule.waitForIdle()
        rule.onNodeWithTag("browser-ended").assertDoesNotExist()
        phone.tunnelEnds("t1", "run r1 ended")
        rule.waitForIdle()
        says("browser-ended", "The tunnel ended: run r1 ended")
    }

    @Test
    fun a_tunnel_held_to_a_run_is_closed_from_its_row() {
        val phone = phone()
        start(phone)
        rule.onNodeWithTag("run-browser:r1").performClick()
        rule.waitForIdle()
        rule.onNodeWithTag("tunnel-close:r1").performClick()
        rule.waitForIdle()
        assertEquals(listOf("t1"), phone.closedTunnels)
        rule.onNodeWithTag("tunnel:r1").assertDoesNotExist()
    }
}
