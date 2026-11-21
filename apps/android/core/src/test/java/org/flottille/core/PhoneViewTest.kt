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
        MachineView(name = name, role = "runner", online = state == "online", state = state, since = 5uL, why = "")

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
            requests = listOf(RequestView(machine = "server", number = 3uL, agent = "a1", tool = "shell", reasons = emptyList())),
            asks = listOf(AskView(machine = "server", agent = "a2", asked = 9uL, question = "which one?")),
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
}
