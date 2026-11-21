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
}
