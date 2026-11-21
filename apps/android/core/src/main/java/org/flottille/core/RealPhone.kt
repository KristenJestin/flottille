package org.flottille.core

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import org.flottille.core.ffi.MobileException
import org.flottille.core.ffi.ViewState

/** The real core, as [Phone]: the bindings stay in this module. */
class RealPhone(private val core: FlottilleCore) : Phone {
    override val view: PhoneView get() = core.view.toPhoneView()

    /**
     * Connected while collected. A hub that cannot be reached is said in the view and dialled again
     * after a wait that grows (the connection is the core's; the screens never loop). A phone that
     * joined no hub has nothing to connect to: its view, once.
     */
    override fun views(): Flow<PhoneView> = flow {
        emit(view)
        if (!view.joined) return@flow
        var wait = FIRST_WAIT
        while (true) {
            try {
                core.batches().collect {
                    wait = FIRST_WAIT
                    emit(it.view.toPhoneView())
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (refused: MobileException) {
                emit(view.copy(hub = refused.sentence()))
            }
            delay(wait)
            wait = (wait * 2).coerceAtMost(LONGEST_WAIT)
        }
    }

    override fun card(code: String): HubCard =
        try {
            FlottilleCore.card(code).let { HubCard(it.url, it.fingerprint) }
        } catch (refused: MobileException) {
            throw PhoneException(refused.sentence())
        }

    override suspend fun join(code: String, name: String, role: String) =
        try {
            core.join(code, name, role)
        } catch (refused: MobileException) {
            throw PhoneException(refused.sentence())
        }

    override suspend fun leave() =
        try {
            core.leave()
        } catch (refused: MobileException) {
            throw PhoneException(refused.sentence())
        }

    private companion object {
        const val FIRST_WAIT = 1_000L
        const val LONGEST_WAIT = 30_000L
    }
}

private fun MobileException.sentence(): String = (this as? MobileException.Refused)?.why ?: message.orEmpty()

/** What the core's view state says, in the module's own types. */
internal fun ViewState.toPhoneView(): PhoneView {
    val waiting = requests.map { it.machine to it.agent }.toSet() + asks.map { it.machine to it.agent }.toSet()
    return PhoneView(
        role = role,
        name = name,
        hub = hub,
        machines = machines.map {
            Machine(
                name = it.name,
                state = when (it.state) {
                    "online" -> MachineState.Online
                    "needs an update" -> MachineState.NeedsUpdate
                    else -> MachineState.Offline
                },
                since = it.since.toLong(),
                why = it.why,
            )
        },
        projects = projects.map { project ->
            Project(
                project.name,
                project.sessions.map {
                    SessionRow(it.machine, it.agent, it.title, it.state, it.last.toLong(), (it.machine to it.agent) in waiting)
                },
            )
        },
        members = members.map { Member(it.name, it.role, it.icon, it.colour, it.you, it.addedBy) },
        said = said,
    )
}
