package org.flottille.app

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import org.flottille.core.Conversation
import org.flottille.core.HubCard
import org.flottille.core.Phone
import org.flottille.core.PhoneException
import org.flottille.core.Picture
import org.flottille.core.PhoneView
import org.flottille.core.Reply

/**
 * The core the screens see, played: what the tests give it is what the screens draw, and what the
 * screens ask of it is kept. The real core stands behind the same interface in the app.
 */
class FakePhone(initial: PhoneView = PhoneView()) : Phone {
    val state = MutableStateFlow(initial)

    /** The card a code gives; a code that is not here is refused as the core refuses it. */
    val cards = mutableMapOf("CODE-1" to HubCard("https://hub.example.net", "SHA256:q1w2e3r4"))

    /** What the view is once the phone joined. */
    var afterJoin = PhoneView(role = "controller", name = "pixel")

    /** What the view is once the phone left. */
    var afterLeave = PhoneView(said = listOf("this phone was removed from the user's hub: pair it again"))

    data class Joined(val code: String, val name: String, val role: String)

    val joins = mutableListOf<Joined>()
    var leaves = 0

    /** What the session page asked of the core, in order. */
    data class Sent(val machine: String, val agent: String, val text: String, val pictures: List<Picture>)

    data class AskAnswered(val machine: String, val agent: String, val asked: Long, val replies: List<Reply>)

    val sent = mutableListOf<Sent>()
    val askAnswers = mutableListOf<AskAnswered>()
    val allowed = mutableListOf<Triple<String, Long, Boolean>>()
    val modes = mutableListOf<Triple<String, String, String>>()
    val stops = mutableListOf<Pair<String, String>>()
    val follows = mutableListOf<Pair<String, String>>()
    val olders = mutableListOf<Pair<String, String>>()
    var unfollows = 0

    /** The conversation a session gives once it is followed. */
    val pages = mutableMapOf<String, Conversation>()

    /** What older messages `older` brings: it replaces the page. */
    val earlier = mutableMapOf<String, Conversation>()

    /** The sentence the core refuses a method with (`send`, `answer`, `answerAsk`, `setMode`, `stop`). */
    val refusals = mutableMapOf<String, String>()

    /** The bytes of the pictures of a conversation, by name. */
    val pictureBytes = mutableMapOf<String, ByteArray>()

    private fun refuse(method: String) {
        refusals[method]?.let { throw PhoneException(it) }
    }

    override val view: PhoneView get() = state.value

    override fun views(): Flow<PhoneView> = state

    override fun card(code: String): HubCard =
        cards[code.trim()] ?: throw PhoneException("this is not a Flottille pair code")

    override suspend fun join(code: String, name: String, role: String) {
        joins += Joined(code, name, role)
        state.value = afterJoin.copy(role = role.ifEmpty { afterJoin.role })
    }

    override suspend fun leave() {
        leaves += 1
        state.value = afterLeave
    }

    override suspend fun follow(machine: String, agent: String) {
        follows += machine to agent
        state.value = state.value.copy(conversation = pages[agent])
    }

    override suspend fun older(machine: String, agent: String) {
        olders += machine to agent
        earlier[agent]?.let { state.value = state.value.copy(conversation = it) }
    }

    override fun unfollow() {
        unfollows += 1
        state.value = state.value.copy(conversation = null)
    }

    override suspend fun send(machine: String, agent: String, text: String, pictures: List<Picture>) {
        refuse("send")
        sent += Sent(machine, agent, text, pictures)
    }

    override suspend fun answerAsk(machine: String, agent: String, asked: Long, replies: List<Reply>) {
        refuse("answerAsk")
        askAnswers += AskAnswered(machine, agent, asked, replies)
    }

    override suspend fun answer(machine: String, number: Long, allow: Boolean) {
        refuse("answer")
        allowed += Triple(machine, number, allow)
    }

    override suspend fun setMode(machine: String, agent: String, mode: String) {
        refuse("setMode")
        modes += Triple(machine, agent, mode)
    }

    override suspend fun stop(machine: String, agent: String) {
        refuse("stop")
        stops += machine to agent
    }

    override suspend fun picture(machine: String, agent: String, name: String): ByteArray =
        pictureBytes[name] ?: throw PhoneException("no picture named $name")
}
