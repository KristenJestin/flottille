package org.flottille.app

import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import java.time.Instant
import java.time.ZoneOffset
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.PhoneView
import org.flottille.core.Project
import org.flottille.core.SessionRow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The Sessions tab as decided (`docs/remote.md`, "The Android app"; #1441): the window's sidebar,
 * each machine, its projects folded or not, their sessions in one list that never jumps, led
 * sessions under their pilot, the ended ones in a group of their own. One test for each scenario.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w360dp-h900dp-xhdpi", application = android.app.Application::class)
class SessionsTabTest {
    @get:Rule
    val rule = createComposeRule()

    private val now = Instant.parse("2026-11-21T22:30:00Z").toEpochMilli()

    private fun start(phone: FakePhone) {
        rule.setContent {
            FlottilleApp(phone = phone, deviceName = "pixel", zone = ZoneOffset.UTC, now = { now })
        }
    }

    private fun session(
        agent: String,
        created: Long,
        machine: String = "server",
        state: String = "idle",
        last: Long = created,
        waiting: Boolean = false,
        pilot: String = "",
    ) = SessionRow(machine, agent, "Session $agent", state, last, waiting, created = created, pilot = pilot)

    private fun joined(vararg projects: Project) = PhoneView(
        role = "controller",
        name = "pixel",
        machines = listOf(Machine("server", MachineState.Online, now)),
        projects = projects.toList(),
    )

    private fun tagged(prefix: String) =
        SemanticsMatcher("its tag starts with $prefix") {
            it.config.getOrNull(SemanticsProperties.TestTag)?.startsWith(prefix) == true
        }

    /** The tags of the session rows on screen, from the top of the list down. */
    private fun rows(): List<String> =
        rule.onAllNodes(tagged("session:"))
            .fetchSemanticsNodes()
            .sortedBy { it.positionInRoot.y }
            .map { it.config[SemanticsProperties.TestTag] }

    private fun exists(tag: String) = rule.onAllNodes(SemanticsMatcher.expectValue(SemanticsProperties.TestTag, tag))
        .fetchSemanticsNodes().isNotEmpty()

    @Test
    fun an_ended_session_is_not_in_the_sessions_list() {
        val phone = FakePhone(
            joined(Project("alpha", listOf(session("a2", 200), session("a1", 100, state = "ended")))),
        )
        start(phone)
        rule.waitForIdle()

        // Not among the live ones: one folded group below every project says how many ended.
        assertEquals(listOf("session:server:a2"), rows())
        rule.onNodeWithTag("ended").assertTextContains("Ended", substring = true)
        rule.onNodeWithTag("ended").assertTextContains("1", substring = true)

        // Unfolded, the group lists it, its row saying whose it is.
        rule.onNodeWithTag("ended").performClick()
        rule.waitForIdle()
        assertEquals(listOf("session:server:a2", "session:server:a1"), rows())
        rule.onNodeWithTag("session:server:a1").assertTextContains("alpha", substring = true)
        val group = rule.onNodeWithTag("ended").fetchSemanticsNode().positionInRoot.y
        val ended = rule.onNodeWithTag("session:server:a1").fetchSemanticsNode().positionInRoot.y
        assertTrue("the ended row is in its group: $group, then $ended", ended > group)
    }

    @Test
    fun a_session_that_reports_keeps_its_place() {
        val phone = FakePhone(
            joined(Project("alpha", listOf(session("a3", 300), session("a2", 200), session("a1", 100)))),
        )
        start(phone)
        rule.waitForIdle()
        assertEquals(listOf("session:server:a3", "session:server:a2", "session:server:a1"), rows())

        // The oldest reports and waits for the user: its mark changes, its place does not, even
        // when the list it comes in puts it first.
        phone.state.value = joined(
            Project(
                "alpha",
                listOf(
                    session("a1", 100, state = "working", last = 900, waiting = true),
                    session("a3", 300),
                    session("a2", 200),
                ),
            ),
        )
        rule.waitForIdle()
        assertEquals(listOf("session:server:a3", "session:server:a2", "session:server:a1"), rows())
        rule.onNodeWithTag("mark:server:a1", useUnmergedTree = true).assert(hasContentDescription("Needs you", substring = true))
    }

    @Test
    fun a_project_folds_and_stays_folded() {
        val phone = FakePhone(
            joined(
                Project("alpha", listOf(session("a1", 100))),
                Project("beta", listOf(session("b1", 50))),
            ),
        )
        start(phone)
        rule.waitForIdle()
        // The machine is said once, above its projects, not on each row.
        rule.onNodeWithTag("machine:server").assertTextContains("server", substring = true)
        rule.onNodeWithTag("project:server/alpha").assertIsDisplayed()
        assertEquals(listOf("session:server:a1", "session:server:b1"), rows())

        rule.onNodeWithTag("project:server/alpha").performClick()
        rule.waitForIdle()
        assertEquals(listOf("session:server:b1"), rows())

        // Folded it stays: when its sessions report, and when the tab is left and come back to.
        phone.state.value = joined(
            Project("alpha", listOf(session("a1", 100, state = "working", last = 900), session("a0", 150))),
            Project("beta", listOf(session("b1", 50))),
        )
        rule.waitForIdle()
        assertEquals(listOf("session:server:b1"), rows())
        rule.onNodeWithTag("tab:machines").performClick()
        rule.waitForIdle()
        rule.onNodeWithTag("tab:sessions").performClick()
        rule.waitForIdle()
        assertEquals(listOf("session:server:b1"), rows())

        rule.onNodeWithTag("project:server/alpha").performClick()
        rule.waitForIdle()
        assertEquals(listOf("session:server:a0", "session:server:a1", "session:server:b1"), rows())
    }

    @Test
    fun a_led_session_is_under_its_pilot_folded_by_default() {
        val phone = FakePhone(
            joined(
                Project(
                    "alpha",
                    listOf(
                        session("l2", 400, state = "working", pilot = "p1"),
                        session("u1", 300),
                        session("l1", 200, pilot = "p1"),
                        session("p1", 100),
                    ),
                ),
            ),
        )
        start(phone)
        rule.waitForIdle()

        // Folded: the pilot's row says how many it leads and their state.
        assertEquals(listOf("session:server:u1", "session:server:p1"), rows())
        rule.onNodeWithTag("session:server:p1").assertTextContains("Leads 2", substring = true)
        rule.onNodeWithTag("session:server:p1").assertTextContains("1 running", substring = true)
        assertTrue(!exists("session:server:l1"))

        // Unfolded: its led sessions under it, indented, newest first.
        rule.onNodeWithTag("fold:server:p1").performClick()
        rule.waitForIdle()
        assertEquals(
            listOf("session:server:u1", "session:server:p1", "session:server:l2", "session:server:l1"),
            rows(),
        )
        val pilot = rule.onNodeWithTag("session:server:p1").fetchSemanticsNode().positionInRoot.x
        val led = rule.onNodeWithTag("session:server:l1").fetchSemanticsNode().positionInRoot.x
        assertTrue("indented under its pilot: $pilot, then $led", led > pilot)
    }
}
