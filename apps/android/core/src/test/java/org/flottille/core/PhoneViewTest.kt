package org.flottille.core

import org.flottille.core.ffi.AskView
import org.flottille.core.ffi.MachineView
import org.flottille.core.ffi.MemberView
import org.flottille.core.ffi.ProjectView
import org.flottille.core.ffi.RequestView
import org.flottille.core.ffi.SessionView
import org.flottille.core.ffi.ViewState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What the screens read is not the generated bindings' view state but the module's own types: a
 * session that waits for the user (a request or a question of its agent is open) says so, a
 * machine's state is a state and no sentence, and a phone that claims no role has joined no hub.
 */
class PhoneViewTest {
    private fun machine(name: String, state: String) =
        MachineView(
            name = name, role = "runner", online = state == "online", throughHub = false, state = state, since = 5uL, why = "",
        )

    private fun session(machine: String, agent: String) =
        SessionView(machine = machine, agent = agent, title = "t", state = "idle", last = 7uL)

    private fun state(
        machines: List<MachineView> = emptyList(),
        projects: List<ProjectView> = emptyList(),
        requests: List<RequestView> = emptyList(),
        asks: List<AskView> = emptyList(),
        role: String = "controller",
    ) = ViewState(
        role = role,
        name = "pixel",
        hub = "",
        machines = machines,
        members = listOf(
            MemberView(name = "pixel", role = role, icon = "phone", colour = "blue", you = true, addedBy = "the hub"),
        ),
        projects = projects,
        conversation = null,
        requests = requests,
        asks = asks,
        said = emptyList(),
    )

    @Test
    fun a_machine_s_state_is_a_state_and_its_since_a_time() {
        val view = state(
            machines = listOf(machine("a", "online"), machine("b", "offline"), machine("c", "needs an update")),
        ).toPhoneView()
        assertEquals(
            listOf(MachineState.Online, MachineState.Offline, MachineState.NeedsUpdate),
            view.machines.map { it.state },
        )
        assertEquals(5L, view.machines[0].since)
    }

    @Test
    fun a_session_with_a_request_or_a_question_open_waits_for_the_user() {
        val view = state(
            projects = listOf(
                ProjectView("alpha", listOf(session("server", "a1"), session("server", "a2"), session("laptop", "a1"))),
            ),
            requests = listOf(
                RequestView(
                    machine = "server", number = 3uL, agent = "a1", tool = "shell", kind = "call",
                    call = "shell", base = false, says = emptyList(), judgement = "", why = "", always = "", source = "",
                ),
            ),
            asks = listOf(
                AskView(machine = "server", agent = "a2", asked = 9uL, question = "which one?", questions = emptyList()),
            ),
        ).toPhoneView()
        val waiting = view.projects[0].sessions.map { Triple(it.machine, it.agent, it.waiting) }
        // The agent's name alone is not enough: laptop's a1 is another session than server's.
        assertEquals(
            listOf(Triple("server", "a1", true), Triple("server", "a2", true), Triple("laptop", "a1", false)),
            waiting,
        )
    }

    @Test
    fun a_phone_that_claims_no_role_has_joined_no_hub() {
        assertTrue(state().toPhoneView().joined)
        assertFalse(state(role = "").toPhoneView().joined)
        val me = state().toPhoneView().members.single()
        assertEquals(Member("pixel", "controller", "phone", "blue", you = true, addedBy = "the hub"), me)
    }

    @Test
    fun a_conversation_its_requests_and_its_asks_are_the_module_s_own_types() {
        val view = state().copy(
            conversation = org.flottille.core.ffi.ConversationView(
                machine = "server", agent = "a1", title = "Fix the login", mode = "full", state = "working", working = true,
                child = false, ended = false, more = true,
                entries = listOf(
                    org.flottille.core.ffi.Entry(seq = 3uL, kind = "user", text = "look [Image 1: shot.png]", images = listOf("shot.png")),
                ),
                children = listOf(
                    org.flottille.core.ffi.ChildView(agent = "c1", title = "Check", doing = "running", role = "worker", depth = 1u),
                ),
            ),
            requests = listOf(
                RequestView(
                    machine = "server", number = 4uL, agent = "a1", tool = "connections", kind = "action",
                    call = "rm -rf build", base = true, says = listOf("Adds docs."), judgement = "The judge asks you: x",
                    why = "It deletes.", always = "It always asks.", source = "asked by the conversation · #4",
                ),
            ),
            asks = listOf(
                AskView(
                    machine = "server", agent = "a1", asked = 12uL, question = "Push?",
                    questions = listOf(
                        org.flottille.core.ffi.AskQuestion(
                            question = "Push?", options = listOf(org.flottille.core.ffi.AskOption("Yes", "Squash first")), multiple = true,
                        ),
                    ),
                ),
            ),
        ).toPhoneView()

        val page = view.conversation!!
        assertEquals(Conversation("server", "a1", "Fix the login", "full", true, false, false, true, page.entries, page.children), page)
        assertEquals(Entry(3, "user", "look [Image 1: shot.png]", listOf("shot.png")), page.entries.single())
        assertEquals(Child("c1", "Check", "running", "worker", 1), page.children.single())
        assertEquals(
            Request(
                "server", 4, "a1", "connections", "action", "rm -rf build", true, "The judge asks you: x", "It deletes.",
                "It always asks.", "asked by the conversation · #4", listOf("Adds docs."),
            ),
            view.requests.single(),
        )
        val ask = view.asks.single()
        assertEquals(12L, ask.asked)
        assertEquals(AskQuestion("Push?", listOf(AskOption("Yes", "Squash first")), multiple = true), ask.questions.single())
    }
}
