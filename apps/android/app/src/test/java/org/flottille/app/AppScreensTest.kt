package org.flottille.app

import androidx.compose.foundation.clickable
import androidx.compose.foundation.text.BasicText
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTextReplacement
import java.time.Instant
import java.time.ZoneOffset
import org.flottille.core.HubCard
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.Member
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
 * The screens of the app (`docs/remote.md`, "The Android app", "Pairing") against a fake core: what
 * the core says is what is drawn, what the user does reaches the core as the screens ask it. One
 * test for each scenario of #825, named after it.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w360dp-h900dp-xhdpi", application = android.app.Application::class)
class AppScreensTest {
    @get:Rule
    val rule = createComposeRule()

    private fun at(iso: String) = Instant.parse(iso).toEpochMilli()

    private val now = at("2026-11-21T22:30:00Z")
    private val since = at("2026-11-21T22:14:00Z")
    private val opened = mutableListOf<Pair<String, String>>()

    private fun start(phone: FakePhone) {
        rule.setContent {
            FlottilleApp(
                phone = phone,
                deviceName = "pixel",
                zone = ZoneOffset.UTC,
                now = { now },
                scanner = { onCode ->
                    BasicText("Scan the fake code", Modifier.clickable { onCode("CODE-1") })
                },
                onOpenSession = { machine, agent -> opened += machine to agent },
            )
        }
    }

    private fun machine(name: String, state: MachineState = MachineState.Online, why: String = "") =
        Machine(name, state, since, why)

    private fun session(
        machine: String,
        agent: String,
        title: String = "Session $agent",
        state: String = "idle",
        last: Long = 0,
        waiting: Boolean = false,
    ) = SessionRow(machine, agent, title, state, last, waiting)

    private fun joined(
        machines: List<Machine> = emptyList(),
        projects: List<Project> = emptyList(),
        members: List<Member> = emptyList(),
        hub: String = "",
    ) = PhoneView(
        role = "controller",
        name = "pixel",
        hub = hub,
        machines = machines,
        projects = projects,
        members = members,
    )

    private fun tagged(prefix: String) =
        SemanticsMatcher("its tag starts with $prefix") {
            it.config.getOrNull(SemanticsProperties.TestTag)?.startsWith(prefix) == true
        }

    /** The tags of the project headers and the session rows, from the top of the list down. */
    private fun listed(): List<String> =
        rule.onAllNodes(tagged("project:") or tagged("session:"))
            .fetchSemanticsNodes()
            .sortedBy { it.positionInRoot.y }
            .map { it.config[SemanticsProperties.TestTag] }

    private fun top(tag: String): Float = rule.onNodeWithTag(tag).fetchSemanticsNode().positionInRoot.y

    @Test
    fun joining_by_qr_shows_the_hub_s_card_then_the_machines() {
        val phone = FakePhone()
        phone.afterJoin = joined(machines = listOf(machine("server")))
        start(phone)

        rule.onNodeWithText("Connect to your hub").assertIsDisplayed()
        rule.onNodeWithText("Scan the fake code").performClick()
        // The hub's card, before anything is sent: its address and its key's fingerprint.
        rule.onNodeWithText("https://hub.example.net").assertIsDisplayed()
        rule.onNodeWithText("SHA256:q1w2e3r4").assertIsDisplayed()
        assertTrue(phone.joins.isEmpty())

        rule.onNodeWithText("Join").performClick()
        assertEquals(listOf(FakePhone.Joined("CODE-1", "pixel", "controller")), phone.joins)
        // The result, said: the role the hub gave.
        rule.onNodeWithText("made this phone a controller", substring = true).assertIsDisplayed()

        rule.onNodeWithText("Continue").performClick()
        rule.onNodeWithTag("tab:machines").performClick()
        rule.onNodeWithTag("machine:server").assertIsDisplayed()
    }

    @Test
    fun the_hub_s_card_warns_when_its_link_is_in_clear() {
        val phone = FakePhone()
        phone.cards["CODE-HTTP"] = HubCard("http://192.0.2.7:8080", "SHA256:a1b2c3d4")
        start(phone)

        // A hub reached by https says nothing; one reached by http says its link is in clear.
        rule.onNodeWithTag("code-field").performTextInput("CODE-1")
        rule.onNodeWithText("Use this code").performClick()
        rule.onNodeWithTag("clear-warning").assertDoesNotExist()
        rule.onNodeWithText("Back").performClick()

        rule.onNodeWithTag("code-field").performTextInput("CODE-HTTP")
        rule.onNodeWithText("Use this code").performClick()
        rule.onNodeWithText("http://192.0.2.7:8080").assertIsDisplayed()
        rule.onNodeWithTag("clear-warning").assertIsDisplayed()
        rule.onNodeWithText("in clear", substring = true).assertIsDisplayed()
    }

    @Test
    fun a_pasted_code_is_read_like_a_scanned_one_and_a_refused_one_says_why() {
        val phone = FakePhone()
        start(phone)

        rule.onNodeWithTag("code-field").performTextInput("garbage")
        rule.onNodeWithText("Use this code").performClick()
        rule.onNodeWithText("this is not a Flottille pair code").assertIsDisplayed()
        rule.onNodeWithText("SHA256:q1w2e3r4").assertDoesNotExist()

        rule.onNodeWithTag("code-field").performTextReplacement("CODE-1")
        rule.onNodeWithText("Use this code").performClick()
        rule.onNodeWithText("https://hub.example.net").assertIsDisplayed()
    }

    @Test
    fun a_viewer_joins_as_a_viewer_under_the_name_the_user_confirmed() {
        val phone = FakePhone()
        start(phone)
        rule.onNodeWithText("Scan the fake code").performClick()

        rule.onNodeWithTag("device-name").assertTextContains("pixel")
        rule.onNodeWithTag("device-name").performTextReplacement("tablet")
        rule.onNodeWithText("View only").performClick()
        rule.onNodeWithText("Join").performClick()

        assertEquals(listOf(FakePhone.Joined("CODE-1", "tablet", "viewer")), phone.joins)
        rule.onNodeWithText("made this phone a viewer", substring = true).assertIsDisplayed()
    }

    @Test
    fun a_machine_s_state_is_said_offline_since_or_needs_an_update() {
        val tried = "laptop is unreachable: tried 192.0.2.10:7421 from pixel, no answer in 10 s"
        val update = "old runs Flottille 9.9.9 (protocol 99), this machine 0.4.1 (protocol 8): update one of them (flottille update)"
        val phone = FakePhone(
            joined(
                machines = listOf(
                    machine("server"),
                    machine("laptop", MachineState.Offline, tried),
                    machine("old", MachineState.NeedsUpdate, update),
                ),
            ),
        )
        start(phone)
        rule.onNodeWithTag("tab:machines").performClick()

        rule.onNodeWithTag("machine:server").assertTextContains("Online")
        // Offline says since when (the time of day), and which address was tried.
        rule.onNodeWithTag("machine:laptop").assertTextContains("Offline since 22:14")
        rule.onNodeWithTag("machine:laptop").assertTextContains(tried, substring = true)
        // A machine that needs an update is never said offline.
        rule.onNodeWithTag("machine:old").assertTextContains("Needs an update")
        rule.onNodeWithTag("machine:old").assertTextContains(update, substring = true)
        rule.onNode(hasTestTag("machine:old") and hasText("Offline", substring = true)).assertDoesNotExist()

        // The hub's own state: reachable, then not.
        rule.onNodeWithTag("hub").assertTextContains("Reachable", substring = true)
        phone.state.value = phone.state.value.copy(
            hub = "the hub at hub.example.net is unreachable since 22:20: Connection refused",
        )
        rule.onNodeWithTag("hub").assertTextContains("unreachable since 22:20", substring = true)
    }

    @Test
    fun the_sessions_of_every_machine_are_one_list_waiting_ones_first() {
        val phone = FakePhone(
            joined(
                machines = listOf(machine("laptop"), machine("server")),
                projects = listOf(
                    Project(
                        "alpha",
                        listOf(
                            session("server", "a2", last = 300),
                            session("laptop", "a1", state = "working", last = 100),
                            session("laptop", "a3", last = 50, waiting = true),
                        ),
                    ),
                    Project("beta", listOf(session("server", "b1", last = 10))),
                ),
            ),
        )
        start(phone)

        // One list, grouped by project; in a project the ones that wait for the user come first.
        assertEquals(
            listOf(
                "project:alpha",
                "session:laptop:a3",
                "session:server:a2",
                "session:laptop:a1",
                "project:beta",
                "session:server:b1",
            ),
            listed(),
        )
        // The machine is a tag on each row.
        rule.onNodeWithTag("session:laptop:a3").assertTextContains("laptop", substring = true)
        rule.onNodeWithTag("session:server:b1").assertTextContains("server", substring = true)
        rule.onNodeWithTag("session:laptop:a3").assertTextContains("Waiting for you", substring = true)
    }

    @Test
    fun a_session_of_a_machine_offline_is_greyed_and_not_opened() {
        val phone = FakePhone(
            joined(
                machines = listOf(machine("server"), machine("laptop", MachineState.Offline, "laptop is offline")),
                projects = listOf(
                    Project("alpha", listOf(session("server", "s1", last = 2), session("laptop", "l1", last = 1))),
                ),
            ),
        )
        start(phone)

        rule.onNodeWithTag("session:laptop:l1").assertIsNotEnabled()
        rule.onNodeWithTag("session:laptop:l1").assert(
            SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "Machine offline"),
        )
        rule.onNodeWithTag("session:laptop:l1").performClick()
        assertTrue(opened.isEmpty())

        rule.onNodeWithTag("session:server:s1").assertIsEnabled()
        rule.onNodeWithTag("session:server:s1").performClick()
        assertEquals(listOf("server" to "s1"), opened)
    }

    @Test
    fun a_session_added_or_ended_animates_in_the_list() {
        val phone = FakePhone(
            joined(
                machines = listOf(machine("server")),
                projects = listOf(Project("alpha", listOf(session("server", "a1", last = 100), session("server", "a2", last = 50)))),
            ),
        )
        start(phone)
        rule.waitForIdle()
        rule.mainClock.autoAdvance = false

        // Added: a newer session joins above the others, which slide down to make room instead of
        // jumping.
        val before = top("session:server:a1")
        phone.state.value = phone.state.value.copy(
            projects = listOf(
                Project(
                    "alpha",
                    listOf(session("server", "a0", last = 200), session("server", "a1", last = 100), session("server", "a2", last = 50)),
                ),
            ),
        )
        rule.mainClock.advanceTimeByFrame()
        rule.mainClock.advanceTimeBy(60)
        val during = top("session:server:a1")
        rule.mainClock.advanceTimeBy(1000)
        val after = top("session:server:a1")
        assertTrue("a1 slides down: $before, then $during, then $after", before < during && during < after)
        rule.onNodeWithTag("session:server:a0").assertIsDisplayed()

        // Ended: it goes after the ones that go on, and the row below it slides up to its place.
        val a2Before = top("session:server:a2")
        phone.state.value = phone.state.value.copy(
            projects = listOf(
                Project(
                    "alpha",
                    listOf(session("server", "a0", last = 200), session("server", "a1", state = "ended", last = 300), session("server", "a2", last = 50)),
                ),
            ),
        )
        rule.mainClock.advanceTimeByFrame()
        rule.mainClock.advanceTimeBy(60)
        val a2During = top("session:server:a2")
        rule.mainClock.advanceTimeBy(1000)
        val a2After = top("session:server:a2")
        assertTrue("a2 slides up: $a2Before, then $a2During, then $a2After", a2Before > a2During && a2During > a2After)
        assertEquals(
            listOf("project:alpha", "session:server:a0", "session:server:a2", "session:server:a1"),
            listed(),
        )
    }

    @Test
    fun remove_this_device_asks_once() {
        val phone = FakePhone(
            joined(
                members = listOf(
                    Member("server", "runner", "server", "teal", you = false, addedBy = "the hub"),
                    Member("pixel", "controller", "phone", "blue", you = true, addedBy = "the hub"),
                ),
            ),
        )
        start(phone)
        rule.onNodeWithTag("open-settings").performClick()

        // This device's name and role, and the members; no way to add a device from a phone.
        rule.onNodeWithTag("this-device").assertTextContains("pixel", substring = true)
        rule.onNodeWithTag("this-device").assertTextContains("controller", substring = true)
        rule.onNodeWithTag("member:server").assertIsDisplayed()
        rule.onNodeWithTag("member:pixel").assertIsDisplayed()
        rule.onNodeWithText("Add a device").assertDoesNotExist()

        // Asked once: nothing is removed until the question is answered, and Cancel keeps it.
        rule.onNodeWithText("Remove this device").performClick()
        rule.onNodeWithText("Remove this device?").assertIsDisplayed()
        assertEquals(0, phone.leaves)
        rule.onNodeWithText("Cancel").performClick()
        rule.onNodeWithText("Remove this device?").assertDoesNotExist()
        assertEquals(0, phone.leaves)

        rule.onNodeWithText("Remove this device").performClick()
        rule.onNodeWithText("Remove").performClick()
        assertEquals(1, phone.leaves)
        // The phone is no member any more: it offers to join again, and says why.
        rule.onNodeWithText("Connect to your hub").assertIsDisplayed()
        rule.onNodeWithText("this phone was removed from the user's hub: pair it again").assertIsDisplayed()
    }
}

