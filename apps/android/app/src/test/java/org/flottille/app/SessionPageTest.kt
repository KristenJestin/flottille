package org.flottille.app

import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import java.time.Instant
import java.time.ZoneOffset
import org.flottille.app.ui.FillColour
import org.flottille.app.ui.Palette
import org.flottille.core.Ask
import org.flottille.core.AskOption
import org.flottille.core.AskQuestion
import org.flottille.core.Child
import org.flottille.core.Conversation
import org.flottille.core.Entry
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.PhoneView
import org.flottille.core.Picture
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

/** The pictures a fake phone's gallery and camera give, and what the app is asked for. */
class FakePictures(var gallery: List<Picture> = emptyList(), var camera: Picture? = null) : PictureSource {
    override fun gallery(done: (List<Picture>) -> Unit) = done(gallery)

    override fun camera(done: (Picture?) -> Unit) = done(camera)
}

/**
 * The session page (#826, `docs/design.md` §10, `docs/remote.md`, "The Android app") against a
 * fake core: what the core says is what is drawn, what the user does reaches the core as the page
 * asks it. One test for each scenario of the issue, named after it.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w360dp-h900dp-xhdpi", application = android.app.Application::class)
class SessionPageTest {
    @get:Rule
    val rule = createComposeRule()

    private val since = Instant.parse("2026-11-21T22:14:00Z").toEpochMilli()
    private val pictures = FakePictures()

    private fun phone(
        conversation: Conversation = page(),
        role: String = "controller",
        machine: Machine = Machine("server", MachineState.Online, since),
        requests: List<Request> = emptyList(),
        asks: List<Ask> = emptyList(),
        agent: String = "a1",
    ): FakePhone {
        val phone = FakePhone(
            PhoneView(
                role = role,
                name = "pixel",
                machines = listOf(machine),
                projects = listOf(
                    Project("app", listOf(SessionRow("server", "a1", "Fix the login", "idle", 5), SessionRow("server", "c1", "Check", "working", 4))),
                ),
                requests = requests,
                asks = asks,
            ),
        )
        phone.pages[agent] = conversation
        phone.pages["a1"] = conversation
        return phone
    }

    private fun start(phone: FakePhone, shared: List<Picture> = emptyList()) {
        rule.setContent {
            FlottilleApp(
                phone = phone,
                deviceName = "pixel",
                zone = ZoneOffset.UTC,
                pictures = pictures,
                shared = shared,
            )
        }
    }

    private fun open(agent: String = "a1") {
        rule.onNodeWithTag("session:server:$agent").performClick()
        rule.waitForIdle()
    }

    private fun page(
        entries: List<Entry> = listOf(
            Entry(1, "user", "Fix the login"),
            Entry(2, "agent", "On it."),
        ),
        mode: String = "auto",
        working: Boolean = false,
        child: Boolean = false,
        more: Boolean = false,
        children: List<Child> = emptyList(),
    ) = Conversation("server", "a1", "Fix the login", mode, working, child, false, more, entries, children)

    private fun request(number: Long = 4, call: String = "rm -rf build", base: Boolean = false) =
        Request("server", number, "a1", "run", call, base, "it deletes files", listOf("it deletes files"), emptyList())

    private fun ask(asked: Long = 12) = Ask(
        "server",
        "a1",
        asked,
        "1. Push?\n2. Title?",
        listOf(
            AskQuestion("Push the branch?", listOf(AskOption("Yes", "Squash first"), AskOption("No", "")), multiple = false),
            AskQuestion("Title of the pull request?", emptyList(), multiple = false),
        ),
    )

    private fun update(phone: FakePhone, change: (Conversation) -> Conversation) {
        phone.state.value = phone.state.value.copy(conversation = change(phone.state.value.conversation!!))
    }

    @Test
    fun the_conversation_follows_live() {
        val phone = phone(
            page(
                entries = listOf(
                    Entry(1, "user", "Fix the login"),
                    Entry(2, "thought", "Looking at the form"),
                    Entry(3, "tool", "Read login.rs"),
                    Entry(4, "agent", "On it."),
                ),
                more = true,
            ),
        )
        start(phone)
        open()

        assertEquals(listOf("server" to "a1"), phone.follows)
        rule.onNodeWithTag("entry:1").assertTextContains("Fix the login", substring = true)
        rule.onNodeWithTag("entry:4").assertTextContains("On it.", substring = true)
        // Its steps are folded as in the window: one line until a click unfolds it.
        rule.onNodeWithText("2 steps").assertIsDisplayed()
        rule.onNodeWithText("Read login.rs").assertDoesNotExist()
        rule.onNodeWithText("2 steps").performClick()
        rule.onNodeWithText("Read login.rs").assertIsDisplayed()

        // Live: the agent's next words come in.
        update(phone) { it.copy(entries = it.entries + Entry(5, "agent", "Done."), working = true) }
        rule.onNodeWithTag("entry:5").assertTextContains("Done.", substring = true)

        // A long conversation pages in: the earlier ones are asked for, never loaded whole.
        phone.earlier["a1"] = phone.state.value.conversation!!.copy(
            entries = listOf(Entry(0, "user", "Earlier")) + phone.state.value.conversation!!.entries,
            more = false,
        )
        rule.onNodeWithText("Earlier messages").performClick()
        assertEquals(listOf("server" to "a1"), phone.olders)
        rule.onNodeWithTag("entry:0").assertIsDisplayed()
        rule.onNodeWithText("Earlier messages").assertDoesNotExist()

        // Leaving the page ends the follow: it is live only while it is on screen.
        rule.onNodeWithText("Back").performClick()
        rule.waitForIdle()
        assertEquals(1, phone.unfollows)
    }

    @Test
    fun a_new_message_unfolds_and_fades_in() {
        val phone = phone()
        start(phone)
        open()
        rule.mainClock.autoAdvance = false
        update(phone) { it.copy(entries = it.entries + Entry(3, "agent", "A new message that is rather long, so that it wraps on a narrow phone")) }
        rule.mainClock.advanceTimeBy(60)
        val during = rule.onNodeWithTag("entry:3").fetchSemanticsNode().size.height
        rule.mainClock.advanceTimeBy(1000)
        val after = rule.onNodeWithTag("entry:3").fetchSemanticsNode().size.height
        assertTrue("it unfolds: $during, then $after", during < after)
    }

    @Test
    fun a_message_with_a_picture_is_sent() {
        val phone = phone()
        pictures.gallery = listOf(Picture("image/png", byteArrayOf(1, 2, 3), "shot.png"))
        start(phone)
        open()

        // Nothing is sent while there is nothing to send.
        rule.onNodeWithTag("send").assertIsNotEnabled()
        rule.onNodeWithText("Gallery").performClick()
        rule.onNodeWithTag("attached:0").assertIsDisplayed()
        rule.onNodeWithTag("composer").performTextInput("look at this")
        rule.onNodeWithTag("send").assertIsEnabled()
        rule.onNodeWithTag("send").performClick()
        rule.waitForIdle()

        assertEquals(1, phone.sent.size)
        assertEquals("look at this", phone.sent[0].text)
        assertEquals("shot.png", phone.sent[0].pictures.single().name)
        assertEquals(listOf<Byte>(1, 2, 3), phone.sent[0].pictures.single().bytes.toList())
        // The composer is empty again.
        rule.onNodeWithTag("attached:0").assertDoesNotExist()

        // The camera adds a picture the same way.
        pictures.camera = Picture("image/jpeg", byteArrayOf(9), "photo.jpg")
        rule.onNodeWithText("Camera").performClick()
        rule.onNodeWithTag("attached:0").assertIsDisplayed()
        rule.onNodeWithTag("send").performClick()
        rule.waitForIdle()
        assertEquals("photo.jpg", phone.sent[1].pictures.single().name)
    }

    @Test
    fun a_picture_over_the_window_s_limits_is_refused_in_the_composer() {
        val phone = phone()
        pictures.gallery = listOf(
            Picture("image/png", ByteArray(5 * 1024 * 1024 + 1), "huge.png"),
            Picture("image/bmp", byteArrayOf(1), "old.bmp"),
        )
        start(phone)
        open()
        rule.onNodeWithText("Gallery").performClick()
        // Said as a toast, and nothing joined the composer.
        rule.onNodeWithTag("toast").assertIsDisplayed()
        rule.onNodeWithTag("attached:0").assertDoesNotExist()
    }

    @Test
    fun a_picture_shared_to_the_app_waits_in_the_composer_and_is_never_sent_unasked() {
        val phone = phone()
        start(phone, shared = listOf(Picture("image/png", byteArrayOf(5), "shared.png")))
        // The user says where it goes: a session of the list.
        rule.onNodeWithText("Pick a session for the picture").assertIsDisplayed()
        open()
        rule.onNodeWithTag("attached:0").assertIsDisplayed()
        assertTrue(phone.sent.isEmpty())
        rule.onNodeWithTag("send").performClick()
        rule.waitForIdle()
        assertEquals("shared.png", phone.sent.single().pictures.single().name)
    }

    @Test
    fun an_ask_is_answered_from_its_card() {
        val phone = phone(asks = listOf(ask()))
        start(phone)
        open()

        // One question at a time, and one Answer for the card.
        rule.onNodeWithTag("ask:12").assertIsDisplayed()
        rule.onNodeWithText("Push the branch?").assertIsDisplayed()
        rule.onNodeWithText("Title of the pull request?").assertDoesNotExist()
        rule.onNodeWithText("1 of 2").assertIsDisplayed()
        rule.onNodeWithTag("ask-answer").assertIsNotEnabled()
        rule.onNodeWithText("Squash first").assertIsDisplayed()
        // Picking an option of a single-choice question moves to the next one.
        rule.onNodeWithTag("option:0:Yes").performClick()
        rule.waitForIdle()
        rule.onNodeWithText("2 of 2").assertIsDisplayed()
        rule.onNodeWithTag("ask-answer").assertIsNotEnabled()
        // A question with no options is answered in words.
        rule.onNodeWithTag("ask-words").performTextInput("Export orders as CSV")
        rule.onNodeWithTag("ask-answer").assertIsEnabled()
        rule.onNodeWithTag("ask-answer").performClick()
        rule.waitForIdle()

        assertEquals(
            listOf(
                FakePhone.AskAnswered(
                    "server",
                    "a1",
                    12,
                    listOf(Reply(listOf("Yes"), ""), Reply(emptyList(), "Export orders as CSV")),
                ),
            ),
            phone.askAnswers,
        )
    }

    @Test
    fun an_ask_the_machine_no_longer_waits_for_says_so_and_folds() {
        val phone = phone(asks = listOf(ask()))
        phone.refusals["answerAsk"] = "that question does not wait for an answer"
        start(phone)
        open()
        rule.onNodeWithTag("option:0:No").performClick()
        rule.onNodeWithTag("ask-words").performTextInput("x")
        rule.mainClock.autoAdvance = false
        rule.onNodeWithTag("ask-answer").performClick()
        rule.mainClock.advanceTimeBy(300)
        rule.onNodeWithText("that question does not wait for an answer").assertIsDisplayed()
        rule.mainClock.advanceTimeBy(6_000)
        rule.onNodeWithTag("ask:12").assertDoesNotExist()
    }

    @Test
    fun a_request_is_allowed_or_denied() {
        val phone = phone(requests = listOf(request(4, "rm -rf build", base = true), request(5, "fs_read {\"path\":\".env\"}")))
        start(phone)
        open()

        // The call as the daemon gave it, never re-summarised, and the strong warning of a base one.
        rule.onNodeWithTag("request:4").assertTextContains("rm -rf build", substring = true)
        rule.onNodeWithTag("request:4").assertTextContains("Always asks", substring = true)
        rule.onNodeWithTag("request:5").assertTextContains("fs_read {\"path\":\".env\"}", substring = true)
        rule.onAllNodesWithText("Always asks", substring = true).assertCountEquals(1)
    }

    @Test
    fun a_request_is_allowed_and_another_denied() {
        val phone = phone(requests = listOf(request(4), request(5)))
        start(phone)
        open()
        rule.onNodeWithTag("allow:4").performClick()
        rule.onNodeWithTag("deny:5").performClick()
        rule.waitForIdle()
        assertEquals(listOf(Triple("server", 4L, true), Triple("server", 5L, false)), phone.allowed)

        // The core drops what was answered; the card folds away.
        phone.state.value = phone.state.value.copy(requests = emptyList())
        rule.mainClock.advanceTimeBy(1_000)
        rule.onNodeWithTag("request:4").assertDoesNotExist()
    }

    @Test
    fun a_request_already_answered_says_so_and_folds() {
        val phone = phone(requests = listOf(request(4)))
        phone.refusals["answer"] = "request #4 is decided already"
        start(phone)
        open()
        rule.mainClock.autoAdvance = false
        rule.onNodeWithTag("allow:4").performClick()
        rule.mainClock.advanceTimeBy(300)
        rule.onNodeWithTag("request:4").assertTextContains("request #4 is decided already", substring = true)
        rule.mainClock.advanceTimeBy(6_000)
        rule.onNodeWithTag("request:4").assertDoesNotExist()
    }

    private fun fill(tag: String) = rule.onNodeWithTag(tag).fetchSemanticsNode().config.getOrNull(FillColour)

    @Test
    fun full_is_drawn_in_the_alarm_colour() {
        val phone = phone(page(mode = "auto"))
        start(phone)
        open()

        // Three choices; the chosen one's fill moves, and Full's is the alarm colour.
        rule.onNodeWithTag("mode:auto").assertIsDisplayed()
        rule.onNodeWithTag("mode:ask").assertIsDisplayed()
        rule.onNodeWithTag("mode:full").assertIsDisplayed()
        rule.onNodeWithTag("mode:full").performClick()
        rule.waitForIdle()
        assertEquals(listOf(Triple("server", "a1", "full")), phone.modes)
        update(phone) { it.copy(mode = "full") }
        rule.mainClock.advanceTimeBy(1_000)
        assertEquals(Palette.Dark.fail, fill("mode:full"))
        assertTrue(fill("mode:auto") != Palette.Dark.fail)

        update(phone) { it.copy(mode = "ask") }
        rule.mainClock.advanceTimeBy(1_000)
        assertTrue(fill("mode:full") != Palette.Dark.fail)
    }

    @Test
    fun stop_interrupts_the_turn() {
        val phone = phone(page(working = true))
        start(phone)
        open()
        rule.onNodeWithTag("stop").assertIsDisplayed()
        rule.onNodeWithTag("stop").performClick()
        rule.waitForIdle()
        assertEquals(listOf("server" to "a1"), phone.stops)

        // No turn runs: nothing to stop.
        update(phone) { it.copy(working = false) }
        rule.mainClock.advanceTimeBy(1_000)
        rule.onNodeWithTag("stop").assertDoesNotExist()
    }

    @Test
    fun a_child_s_page_has_no_composer() {
        val child = Conversation(
            "server", "c1", "Check the tests", "ask", true, true, false, false,
            listOf(Entry(1, "agent", "Running the tests")),
            listOf(Child("c1", "Check the tests", "running cargo test", "worker", 1)),
        )
        val phone = phone(child, agent = "c1")
        start(phone)
        open("c1")
        rule.onNodeWithTag("entry:1").assertIsDisplayed()
        // Read only: its conversation and the tree, no composer, no mode, no Stop.
        rule.onNodeWithTag("composer").assertDoesNotExist()
        rule.onNodeWithTag("send").assertDoesNotExist()
        rule.onNodeWithTag("stop").assertDoesNotExist()
        rule.onNodeWithTag("mode:full").assertDoesNotExist()
        rule.onNodeWithTag("child:c1").assertTextContains("running cargo test", substring = true)
    }

    @Test
    fun a_message_to_an_offline_machine_is_refused() {
        val phone = phone(machine = Machine("server", MachineState.Offline, since, "server is offline"))
        // Opened while the machine was on, then it went off.
        start(phone)
        phone.state.value = phone.state.value.copy(machines = listOf(Machine("server", MachineState.Online, since)))
        open()
        rule.onNodeWithTag("composer").performTextInput("are you there?")
        phone.state.value = phone.state.value.copy(machines = listOf(Machine("server", MachineState.Offline, since, "server is offline")))
        rule.waitForIdle()
        rule.onNodeWithTag("send").performClick()
        rule.waitForIdle()

        // Refused, saying since when, and kept in the composer: nothing is queued for later.
        assertTrue(phone.sent.isEmpty())
        rule.onNodeWithText("server is offline since 22:14").assertIsDisplayed()
        rule.onNodeWithTag("composer").assertTextContains("are you there?", substring = true)
    }

    @Test
    fun a_viewer_s_page_reads_only() {
        val phone = phone(
            page(mode = "auto", working = true),
            role = "viewer",
            requests = listOf(request()),
            asks = listOf(ask()),
        )
        start(phone)
        open()
        // It reads: the conversation, the question and the request are there...
        rule.onNodeWithTag("entry:2").assertIsDisplayed()
        rule.onNodeWithTag("request:4").assertTextContains("rm -rf build", substring = true)
        rule.onNodeWithTag("ask:12").assertIsDisplayed()
        // ...and nothing writes: no composer, no answer, no mode, no Stop.
        for (tag in listOf("composer", "send", "stop", "allow:4", "deny:4", "ask-answer", "mode:full", "mode:ask", "option:0:Yes")) {
            rule.onNodeWithTag(tag).assertDoesNotExist()
        }
        rule.onNodeWithText("Gallery").assertDoesNotExist()
        rule.onNodeWithText("Camera").assertDoesNotExist()
        rule.onNodeWithText("View only").assertIsDisplayed()
    }

    @Test
    fun one_entry_point_opens_a_session_by_machine_and_agent() {
        val phone = phone()
        rule.setContent {
            FlottilleApp(phone = phone, deviceName = "pixel", zone = ZoneOffset.UTC, pictures = pictures, open = SessionKey("server", "a1"))
        }
        rule.waitForIdle()
        rule.onNodeWithTag("entry:2").assertIsDisplayed()
        assertEquals(listOf("server" to "a1"), phone.follows)
    }
}
