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

    override suspend fun follow(machine: String, agent: String) = guarded { core.follow(machine, agent) }

    override suspend fun older(machine: String, agent: String) = guarded { core.older(machine, agent) }

    override fun unfollow() = core.unfollow()

    override suspend fun send(machine: String, agent: String, text: String, pictures: List<Picture>) =
        guarded { core.send(machine, agent, text, pictures.map { org.flottille.core.ffi.Picture(it.mime, it.bytes) }) }

    override suspend fun answerAsk(machine: String, agent: String, asked: Long, replies: List<Reply>) =
        guarded { core.answerAsk(machine, agent, asked, replies.map { org.flottille.core.ffi.Reply(it.choices, it.text) }) }

    override suspend fun answer(machine: String, number: Long, allow: Boolean) =
        guarded { core.answer(machine, number, allow) }

    override suspend fun setMode(machine: String, agent: String, mode: String) =
        guarded { core.setMode(machine, agent, mode) }

    override suspend fun stop(machine: String, agent: String) = guarded { core.stop(machine, agent) }

    override suspend fun picture(machine: String, agent: String, name: String): ByteArray =
        guarded { core.image(machine, agent, name) }

    /** The core's refusal, as the sentence the screens say. */
    private suspend fun <T> guarded(work: suspend () -> T): T =
        try {
            work()
        } catch (refused: MobileException) {
            throw PhoneException(refused.sentence(), gone = refused is MobileException.Gone)
        }

    private companion object {
        const val FIRST_WAIT = 1_000L
        const val LONGEST_WAIT = 30_000L
    }
}

private fun MobileException.sentence(): String =
    (this as? MobileException.Refused)?.why ?: (this as? MobileException.Gone)?.why ?: message.orEmpty()

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
        conversation = conversation?.let { page ->
            Conversation(
                machine = page.machine,
                agent = page.agent,
                title = page.title,
                mode = page.mode,
                working = page.working,
                child = page.child,
                ended = page.ended,
                more = page.more,
                entries = page.entries.map { Entry(it.seq.toLong(), it.kind, it.text, it.images) },
                children = page.children.map { Child(it.agent, it.title, it.doing, it.role, it.depth.toInt()) },
            )
        },
        requests = requests.map {
            Request(it.machine, it.number.toLong(), it.agent, it.tool, it.kind, it.call, it.base, it.judgement, it.why, it.always, it.source, it.says)
        },
        asks = asks.map { ask ->
            Ask(
                ask.machine,
                ask.agent,
                ask.asked.toLong(),
                ask.question,
                ask.questions.map { q -> AskQuestion(q.question, q.options.map { AskOption(it.label, it.description) }, q.multiple) },
            )
        },
    )
}
