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

/**
 * One entry of a conversation: `user`, `agent`, `thought`, `tool`, `link` or `delivery`. [seq] orders and names
 * it. A `link` is one an agent showed: [text] is its title (or host), [link] its address.
 */
data class Entry(
    val seq: Long,
    val kind: String,
    val text: String,
    val images: List<String> = emptyList(),
    val link: String = "",
    /** What a `delivery` entry is (`woken by`, `report`…), said in one [summary]; [text] is its body. */
    val delivery: String = "",
    val summary: String = "",
)

/** A command of the session that runs and reports an address: [url] opens through a tunnel. */
data class Run(val run: String, val line: String, val url: String)

/** A tunnel the phone holds to the port of a run on a machine; it listens on `127.0.0.1:`[localPort]. */
data class Tunnel(val tunnel: String, val machine: String, val run: String, val port: Int, val localPort: Int)

/** A tunnel that ended, and why in the machine's or the link's words (`run r1 ended`). */
data class TunnelEnd(val tunnel: String, val machine: String, val run: String, val reason: String)

/** A tunnel opened: [url] is the address as it is read on this phone, through it. */
data class OpenedTunnel(val tunnel: Tunnel, val url: String)

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
    /** The commands that run and report an address, oldest first. */
    val runs: List<Run> = emptyList(),
)

/**
 * A request of an agent's that waits for Allow or Deny. What the card says comes from the core as
 * the window says it, never summarised here: [call] the call, [why] every reason and who forwarded
 * it, [always] why a base request always asks, [source] who asked.
 */
data class Request(
    val machine: String,
    val number: Long,
    val agent: String,
    val tool: String,
    /** `call`, or `action`; an `action` of `connections` is never allowed from a card. */
    val kind: String,
    val call: String,
    /** A base request always asks, in every mode. */
    val base: Boolean,
    val judgement: String,
    val why: String,
    val always: String,
    val source: String,
    val says: List<String>,
) {
    /** A proposal to write a connection: what it would write is read on a computer, as in the window. */
    val proposesAConnection: Boolean get() = tool == "connections" && kind == "action"
}

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
    /** The tunnels held to the runs of the machines, oldest first. */
    val tunnels: List<Tunnel> = emptyList(),
    /** The tunnels that ended, the newest last. */
    val tunnelsEnded: List<TunnelEnd> = emptyList(),
) {
    val joined: Boolean get() = role.isNotEmpty()
}

/**
 * Why the core could not do what it was asked: a sentence for the user. [gone] says that what was
 * answered is no longer waiting (decided already, or not on the machine): its card folds.
 */
class PhoneException(val why: String, val gone: Boolean = false) : Exception(why)

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

    /**
     * Opens [url], an address of [machine] (`localhost…`), through a tunnel: [run]'s own address
     * when a run is named, else the run that serves the port the link writes. A tunnel held to the
     * same run and use is the one answered. [browser] says the address goes to the user's browser
     * (its tunnel ends by itself after ten minutes with no connection), not to the app's own web
     * view (whose panel closes its tunnel). The machine decides what it reaches and refuses with a
     * sentence ([PhoneException]).
     */
    suspend fun openAddress(machine: String, run: String?, url: String, browser: Boolean): OpenedTunnel

    /** Closes the tunnel [tunnel]. */
    fun closeTunnel(tunnel: String)

    /** Whether [url] names the machine it is read on: it opens through a tunnel, any other as it is. */
    fun isLocal(url: String): Boolean

    /**
     * Keeps the core connected, whichever screen is on, until the returned handle is closed: the
     * foreground service that keeps a tunnel for the user's browser holds it, since the app is
     * then in the background and its screens no longer collect [views].
     */
    fun keepConnected(): AutoCloseable
}
