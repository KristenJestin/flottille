package org.flottille.core

import kotlinx.coroutines.flow.Flow

/** A machine's state, which the screens say in words: a machine that needs an update is not offline. */
enum class MachineState { Online, Offline, NeedsUpdate }

/** A runner of the user's. [since] is when it came to be in [state] (milliseconds since the epoch). */
data class Machine(val name: String, val state: MachineState, val since: Long, val why: String = "")

/** A session of a machine, tagged with it; [waiting] when it needs the user (a request or a question is open). */
data class SessionRow(
    val machine: String,
    val agent: String,
    val title: String,
    val state: String,
    val last: Long,
    val waiting: Boolean = false,
)

/** One entry of a conversation: `user`, `agent`, `thought` or `tool`. [seq] orders and names it. */
data class Entry(val seq: Long, val kind: String, val text: String, val images: List<String> = emptyList())

/** An agent of the session's tree, read only. */
data class Child(val agent: String, val title: String, val doing: String, val role: String, val depth: Int)

/** The conversation a session page follows, and what its controls depend on. */
data class Conversation(
    val machine: String,
    val agent: String,
    val title: String,
    /** `ask`, `auto` or `full`; empty before the machine said it. */
    val mode: String,
    /** Whether a turn runs: what Stop interrupts. */
    val working: Boolean,
    /** A child is written to by its parent only: its page has no composer. */
    val child: Boolean,
    val ended: Boolean,
    /** Whether older messages remain on the machine. */
    val more: Boolean,
    val entries: List<Entry>,
    val children: List<Child>,
)

/** A request of an agent's that waits for Allow or Deny. [call] is the daemon's own text. */
data class Request(
    val machine: String,
    val number: Long,
    val agent: String,
    val tool: String,
    val call: String,
    /** A base request always asks, in every mode. */
    val base: Boolean,
    val reason: String,
    val reasons: List<String>,
    val says: List<String>,
)

data class AskOption(val label: String, val description: String)

data class AskQuestion(val question: String, val options: List<AskOption>, val multiple: Boolean)

/** A question of an agent's that waits for the user: [asked] is the number `agent.answer` names. */
data class Ask(
    val machine: String,
    val agent: String,
    val asked: Long,
    val question: String,
    val questions: List<AskQuestion>,
)

/** A picture of a message: its type, its bytes and the name shown for it. */
class Picture(val mime: String, val bytes: ByteArray, val name: String = "")

/** What the user answered to one question: the options chosen and the words written. */
data class Reply(val choices: List<String>, val text: String)

/** The sessions of every machine under a project's name: one group, whichever machine runs them. */
data class Project(val name: String, val sessions: List<SessionRow>)

/** A device of the user's list. [you] marks this phone. */
data class Member(
    val name: String,
    val role: String,
    val icon: String,
    val colour: String,
    val you: Boolean,
    val addedBy: String,
)

/** The hub a pair code names: what the phone shows before it sends anything. */
data class HubCard(val url: String, val fingerprint: String)

/** What the screens draw, whole. [role] is empty until the phone is a member of a hub's list. */
data class PhoneView(
    val role: String = "",
    val name: String = "",
    /** Empty while the hub's session is open, else why it is not. */
    val hub: String = "",
    val machines: List<Machine> = emptyList(),
    val projects: List<Project> = emptyList(),
    val members: List<Member> = emptyList(),
    val said: List<String> = emptyList(),
    /** The conversation the page on screen follows, none when no page is. */
    val conversation: Conversation? = null,
    /** What waits for the user's Allow or Deny, and the questions that wait for an answer. */
    val requests: List<Request> = emptyList(),
    val asks: List<Ask> = emptyList(),
) {
    val joined: Boolean get() = role.isNotEmpty()
}

/** Why the core could not do what it was asked: a sentence for the user. */
class PhoneException(val why: String) : Exception(why)

/**
 * The core as the screens see it (`docs/remote.md`, "The Android app"): the module's own types, never
 * the generated bindings. [RealPhone] is the real core behind it; the screens' tests play it.
 */
interface Phone {
    /** The view as it is now. */
    val view: PhoneView

    /** The view as it changes, while collected: connected to the hub, linked to each machine. */
    fun views(): Flow<PhoneView>

    /** The hub a pair code (scanned or pasted) names; refused with a sentence when it is none. */
    fun card(code: String): HubCard

    /** Pairs with the hub as [name] with [role] (`controller` or `viewer`). */
    suspend fun join(code: String, name: String, role: String)

    /** Removes this device from the user's list. */
    suspend fun leave()

    /** Follows the conversation of [agent] of [machine]: in the view while it is on screen. */
    suspend fun follow(machine: String, agent: String)

    /** The page before the oldest messages held. */
    suspend fun older(machine: String, agent: String)

    /** The page is left: nothing of it is kept. */
    fun unfollow()

    /** A message of the user, with pictures, as the window's composer sends it. */
    suspend fun send(machine: String, agent: String, text: String, pictures: List<Picture>)

    /** The answers to the ask [asked], one per question. */
    suspend fun answerAsk(machine: String, agent: String, asked: Long, replies: List<Reply>)

    /** Allow ([allow]) or Deny for the request [number] of [machine]. */
    suspend fun answer(machine: String, number: Long, allow: Boolean)

    /** Puts the session's tree in `ask`, `auto` or `full`. */
    suspend fun setMode(machine: String, agent: String, mode: String)

    /** Stops the turn the session runs; the agent stays. */
    suspend fun stop(machine: String, agent: String)

    /** The bytes of a picture of the conversation, by name. */
    suspend fun picture(machine: String, agent: String, name: String): ByteArray
}
