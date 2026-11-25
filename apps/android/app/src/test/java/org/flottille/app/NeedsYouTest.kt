package org.flottille.app

import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import java.time.ZoneOffset
import org.flottille.core.Ask
import org.flottille.core.AskOption
import org.flottille.core.AskQuestion
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.PhoneView
import org.flottille.core.Project
import org.flottille.core.Reply
import org.flottille.core.Request
import org.flottille.core.SessionRow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Needs you (`docs/remote.md`, "The Android app"), the destination the app opens on: what waits for
 * the user on every machine, answered on its card. One test for each scenario of #1443, named after it.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w360dp-h900dp-xhdpi", application = android.app.Application::class)
class NeedsYouTest {
    @get:Rule
    val rule = createComposeRule()

    private val opened = mutableListOf<Pair<String, String>>()

    private fun start(phone: FakePhone) {
        rule.setContent {
            FlottilleApp(
                phone = phone,
                deviceName = "pixel",
                zone = ZoneOffset.UTC,
                onOpenSession = { machine, agent -> opened += machine to agent },
            )
        }
    }

    private fun request(machine: String, number: Long, agent: String, call: String, at: Long = 0, session: String = agent, child: String = "") =
        Request(
            machine = machine,
            number = number,
            agent = agent,
            tool = "run",
            kind = "call",
            call = call,
            base = false,
            judgement = "",
            why = "run asks in Ask mode",
            always = "",
            source = "asked by the conversation · #$number",
            says = emptyList(),
            at = at,
            session = session,
            child = child,
        )

    private fun ask(machine: String, agent: String, asked: Long, question: String, at: Long = 0) =
        Ask(
            machine = machine,
            agent = agent,
            asked = asked,
            question = question,
            questions = listOf(AskQuestion(question, listOf(AskOption("Yes", ""), AskOption("No", "")), multiple = false)),
            at = at,
        )

    private fun joined(requests: List<Request> = emptyList(), asks: List<Ask> = emptyList()) = PhoneView(
        role = "controller",
        name = "pixel",
        machines = listOf(Machine("server", MachineState.Online, 0), Machine("laptop", MachineState.Online, 0)),
        projects = listOf(
            Project(
                "alpha",
                listOf(
                    SessionRow("server", "a1", "Fix the build", "working", 300, waiting = requests.isNotEmpty() || asks.isNotEmpty()),
                    SessionRow("laptop", "b1", "Write the notes", "idle", 200),
                ),
            ),
        ),
        requests = requests,
        asks = asks,
    )

    private fun tagged(prefix: String) =
        SemanticsMatcher("its tag starts with $prefix") {
            it.config.getOrNull(SemanticsProperties.TestTag)?.startsWith(prefix) == true
        }

    /** The tags of the cards of Needs you, from the top down. */
    private fun cards(): List<String> =
        rule.onAllNodes(tagged("needs:"))
            .fetchSemanticsNodes()
            .sortedBy { it.positionInRoot.y }
            .map { it.config[SemanticsProperties.TestTag] }

    @Test
    fun the_app_opens_on_needs_you() {
        start(FakePhone(joined()))

        // Three destinations in the bottom bar, each with its icon and its label; Needs you is on.
        rule.onNodeWithTag("tab:needs-you").assertTextContains("Needs you")
        rule.onNodeWithTag("tab:sessions").assertTextContains("Sessions")
        rule.onNodeWithTag("tab:machines").assertTextContains("Machines")
        listOf("needs-you", "sessions", "machines").forEach {
            rule.onNodeWithTag("tab-icon:$it", useUnmergedTree = true).assertIsDisplayed()
        }
        rule.onNodeWithTag("tab:needs-you").assert(SemanticsMatcher.expectValue(SemanticsProperties.Selected, true))
        rule.onNodeWithTag("tab:sessions").assert(SemanticsMatcher.expectValue(SemanticsProperties.Selected, false))
        rule.onNodeWithTag("needs-you").assertIsDisplayed()

        // The tab change is animated: the page that leaves fades out while the other fades in.
        rule.mainClock.autoAdvance = false
        rule.onNodeWithTag("tab:sessions").performClick()
        rule.mainClock.advanceTimeByFrame()
        rule.mainClock.advanceTimeBy(60)
        rule.onNodeWithTag("needs-you").assertExists()
        rule.mainClock.advanceTimeBy(1000)
        rule.onNodeWithTag("needs-you").assertDoesNotExist()
        rule.onNodeWithTag("session:server:a1").assertIsDisplayed()
    }

    @Test
    fun a_request_is_answered_on_its_card() {
        val phone = FakePhone(joined(requests = listOf(request("server", 4, "a1", "cargo test -p flottille"))))
        start(phone)

        // The whole call in sight, with the session and the machine it waits on.
        val card = rule.onNodeWithTag("needs:request:server:4")
        card.assertTextContains("cargo test -p flottille", substring = true)
        card.assertTextContains("Fix the build", substring = true)
        card.assertTextContains("server", substring = true)
        rule.onNodeWithTag("allow:4").performClick()
        rule.waitForIdle()
        assertEquals(listOf(Triple("server", 4L, true)), phone.allowed)
        // The machine decided it: the card leaves.
        phone.state.value = phone.state.value.copy(requests = emptyList())
        rule.waitForIdle()
        rule.onNodeWithTag("needs:request:server:4").assertDoesNotExist()
    }

    @Test
    fun a_question_is_answered_on_its_card() {
        val phone = FakePhone(joined(asks = listOf(ask("laptop", "b1", 7, "Ship it tonight?"))))
        start(phone)

        rule.onNodeWithTag("needs:ask:laptop:7").assertTextContains("Ship it tonight?", substring = true)
        rule.onNodeWithTag("needs:ask:laptop:7").assertTextContains("Write the notes", substring = true)
        rule.onNodeWithTag("option:0:Yes").performClick()
        rule.onNodeWithTag("ask-answer").performClick()
        rule.waitForIdle()
        assertEquals(listOf(FakePhone.AskAnswered("laptop", "b1", 7, listOf(Reply(listOf("Yes"), "")))), phone.askAnswers)
    }

    @Test
    fun what_waits_on_every_machine_is_shown_newest_first() {
        val phone = FakePhone(
            joined(
                requests = listOf(request("server", 4, "a1", "cargo build", at = 100), request("server", 5, "a1", "git push", at = 300)),
                asks = listOf(ask("laptop", "b1", 7, "Ship it tonight?", at = 200)),
            ),
        )
        start(phone)

        assertEquals(listOf("needs:request:server:5", "needs:ask:laptop:7", "needs:request:server:4"), cards())
    }

    /** A button of the card [card], whose own tag ([tag]) another machine's card may have too. */
    private fun inCard(card: String, tag: String) = rule.onNode(hasTestTag(tag) and hasAnyAncestor(hasTestTag(card)))

    @Test
    fun what_waits_on_two_machines_is_ordered_by_when_it_came() {
        // Neither the machine's name nor the numbers (a request's and an ask's are two counters)
        // say which came first: the time each came does.
        val phone = FakePhone(
            joined(
                requests = listOf(request("server", 9, "a1", "cargo build", at = 100), request("laptop", 2, "b1", "git push", at = 300)),
                asks = listOf(ask("laptop", "b1", 50, "Ship it tonight?", at = 200), ask("server", "a1", 1, "Squash first?", at = 400)),
            ),
        )
        start(phone)

        assertEquals(
            listOf("needs:ask:server:1", "needs:request:laptop:2", "needs:ask:laptop:50", "needs:request:server:9"),
            cards(),
        )
    }

    @Test
    fun a_child_s_request_is_under_its_session() {
        val phone = FakePhone(
            joined(requests = listOf(request("server", 4, "c7", "cargo test", session = "a1", child = "Check the tests"))),
        )
        start(phone)

        // Headed by its session, as on the session page, and the child that asked said.
        val card = rule.onNodeWithTag("needs:request:server:4")
        card.assertTextContains("Fix the build", substring = true)
        card.assertTextContains("Check the tests", substring = true)
        rule.onNodeWithText("cargo test", useUnmergedTree = true).performClick()
        rule.waitForIdle()
        assertEquals(listOf("server" to "a1"), opened)
    }

    @Test
    fun the_same_number_on_two_machines_is_two_cards() {
        val phone = FakePhone(
            joined(requests = listOf(request("server", 4, "a1", "cargo build", at = 100), request("laptop", 4, "b1", "git push", at = 200))),
        )
        start(phone)

        assertEquals(listOf("needs:request:laptop:4", "needs:request:server:4"), cards())
        // Allow on the laptop's reaches the laptop alone.
        inCard("needs:request:laptop:4", "allow:4").performClick()
        rule.waitForIdle()
        assertEquals(listOf(Triple("laptop", 4L, true)), phone.allowed)
        inCard("needs:request:server:4", "deny:4").performClick()
        rule.waitForIdle()
        assertEquals(listOf(Triple("laptop", 4L, true), Triple("server", 4L, false)), phone.allowed)
    }

    @Test
    fun a_request_answered_elsewhere_while_tapping_says_so_and_folds() {
        val phone = FakePhone(joined(requests = listOf(request("server", 4, "a1", "cargo build"))))
        phone.refusals["answer"] = "request #4 is decided already"
        phone.gone += "answer"
        start(phone)
        rule.waitForIdle()
        rule.mainClock.autoAdvance = false

        rule.onNodeWithTag("allow:4").performClick()
        rule.mainClock.advanceTimeBy(300)
        rule.onNodeWithTag("needs:request:server:4").assertTextContains("request #4 is decided already", substring = true)
        rule.mainClock.advanceTimeBy(6_000)
        rule.onNodeWithTag("needs:request:server:4").assertDoesNotExist()
    }

    @Test
    fun a_card_that_folds_answers_nothing() {
        val phone = FakePhone(joined(requests = listOf(request("server", 4, "a1", "cargo build"), request("server", 5, "a1", "git push", at = 1))))
        start(phone)
        rule.waitForIdle()
        rule.mainClock.autoAdvance = false

        // Answered in the window: while its card folds, its buttons are off.
        phone.state.value = phone.state.value.copy(requests = listOf(request("server", 5, "a1", "git push", at = 1)))
        rule.mainClock.advanceTimeByFrame()
        rule.mainClock.advanceTimeBy(40)
        inCard("needs:request:server:4", "allow:4").assertIsNotEnabled()
        inCard("needs:request:server:4", "deny:4").assertIsNotEnabled()
    }

    @Test
    fun the_line_of_nothing_waits_fades_in_and_out() {
        val phone = FakePhone(joined())
        start(phone)
        rule.waitForIdle()
        rule.mainClock.autoAdvance = false

        // Something comes: the line fades out while the card comes, not at once.
        phone.state.value = phone.state.value.copy(requests = listOf(request("server", 4, "a1", "cargo build")))
        rule.mainClock.advanceTimeByFrame()
        rule.mainClock.advanceTimeBy(40)
        rule.onNodeWithTag("needs-you-empty").assertExists()
        rule.mainClock.advanceTimeBy(1_000)
        rule.onNodeWithTag("needs-you-empty").assertDoesNotExist()

        // It is answered: the card folds away, then the line fades in.
        phone.state.value = phone.state.value.copy(requests = emptyList())
        rule.mainClock.advanceTimeByFrame()
        rule.mainClock.advanceTimeBy(40)
        rule.onNodeWithTag("needs:request:server:4").assertExists()
        rule.mainClock.advanceTimeBy(2_000)
        rule.onNodeWithTag("needs-you-empty").assertIsDisplayed()
    }

    @Test
    fun a_card_answered_elsewhere_leaves() {
        val phone = FakePhone(joined(requests = listOf(request("server", 4, "a1", "cargo build"), request("server", 5, "a1", "git push", at = 1))))
        start(phone)
        rule.waitForIdle()
        rule.mainClock.autoAdvance = false

        // Answered in the window: the machine says it is decided, and its card folds away, not at once.
        phone.state.value = phone.state.value.copy(requests = listOf(request("server", 5, "a1", "git push", at = 1)))
        rule.mainClock.advanceTimeByFrame()
        rule.mainClock.advanceTimeBy(40)
        rule.onNodeWithTag("needs:request:server:4").assertExists()
        rule.mainClock.advanceTimeBy(1000)
        rule.onNodeWithTag("needs:request:server:4").assertDoesNotExist()
        rule.onNodeWithTag("needs:request:server:5").assertIsDisplayed()
        assertTrue(phone.allowed.isEmpty())
    }

    @Test
    fun needs_you_says_when_nothing_waits() {
        val phone = FakePhone(joined())
        start(phone)

        rule.onNodeWithTag("needs-you-empty").assertTextContains("Nothing waits for you.")
        assertTrue(cards().isEmpty())

        // Something comes to wait: the line goes, its card comes.
        phone.state.value = phone.state.value.copy(requests = listOf(request("server", 4, "a1", "cargo build")))
        rule.waitForIdle()
        rule.onNodeWithTag("needs-you-empty").assertDoesNotExist()
        rule.onNodeWithTag("needs:request:server:4").assertIsDisplayed()
    }

    @Test
    fun a_card_opens_its_session_on_a_tap_outside_its_buttons() {
        val phone = FakePhone(joined(requests = listOf(request("server", 4, "a1", "cargo build"))))
        start(phone)

        // A tap on a button answers and opens nothing.
        rule.onNodeWithTag("deny:4").performClick()
        rule.waitForIdle()
        assertEquals(listOf(Triple("server", 4L, false)), phone.allowed)
        assertTrue(opened.isEmpty())

        // A tap on the rest of the card opens its session.
        rule.onNodeWithText("cargo build", useUnmergedTree = true).performClick()
        rule.waitForIdle()
        assertEquals(listOf("server" to "a1"), opened)
        assertEquals(listOf("server" to "a1"), phone.follows)
    }
}
