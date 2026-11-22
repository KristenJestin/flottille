package org.flottille.core

import android.content.Context
import java.io.File
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import org.flottille.core.ffi.Batch
import org.flottille.core.ffi.Core
import org.flottille.core.ffi.Listener
import org.flottille.core.ffi.Picture
import org.flottille.core.ffi.Reply
import org.flottille.core.ffi.Sealer
import org.flottille.core.ffi.ViewState
import org.flottille.core.ffi.addressIsLocal as addressIsLocalIn
import org.flottille.core.ffi.hubCard as hubCardOf
import org.flottille.core.ffi.keepPanics as keepPanicsIn
import org.flottille.core.ffi.lastPanic as lastPanicIn

/**
 * The mobile core, as the screens use it (`docs/remote.md`, "The Android app"): `connect` is
 * collecting [batches], then [request], [answer], [stop]. Everything that talks to a machine is in
 * the Rust core; this class only carries it.
 */
class FlottilleCore(folder: File, sealer: Sealer = KeystoreSealer()) : AutoCloseable {
    private val core = Core(folder.path, sealer)

    /** The core in the app's own [folder], which no backup copies. */
    constructor(context: Context, sealer: Sealer = KeystoreSealer()) : this(folder(context), sealer)

    /** The device key's fingerprint, once this phone joined its hub. */
    val fingerprint: String? get() = core.fingerprint()

    val joined: Boolean get() = core.joined()

    /** The view state as it is now; each batch carries it too. */
    val view: ViewState get() = core.view()

    /**
     * Pairs with the hub whose pair code the QR holds (`flottille-hub pair`), as `name` with `role`
     * (`controller` or `viewer`), which the app proposes and the user confirms.
     */
    suspend fun join(code: String, name: String, role: String = "controller") = core.join(code, name, role)

    /**
     * Connected while collected: the control session to the hub, a link to each runner, and their
     * events in batches, each with the view state. Collecting ends with the app in the background.
     */
    fun batches(): Flow<Batch> = callbackFlow {
        core.connect(
            object : Listener {
                override fun batch(batch: Batch) {
                    trySend(batch)
                }
            },
        )
        awaitClose { core.disconnect() }
    }.buffer(Channel.UNLIMITED)

    /** Removes this device from the user's list: the hub sends the list back without it. */
    suspend fun leave() = core.leave()

    /** Whether the user is using the app, for the hub (the runners' notifications). */
    suspend fun presence(active: Boolean) = core.presence(active)

    /** `method` with `params` (JSON) on `machine`, signed with the device key: its result. */
    suspend fun request(machine: String, method: String, params: String = "{}"): String =
        core.request(machine, method, params)

    /** Allow or Deny for the request `number` of `machine`. */
    suspend fun answer(machine: String, number: Long, allow: Boolean) =
        core.answer(machine, number.toULong(), allow)

    /** Stops the turn of the session `agent` of `machine`. */
    suspend fun stop(machine: String, agent: String) = core.stop(machine, agent)

    /** The conversation of `agent` of `machine`, in the view state from now on. */
    suspend fun follow(machine: String, agent: String) = core.follow(machine, agent)

    /** The page leaves the screen: the conversation goes from the view state. */
    fun unfollow() = core.unfollow()

    /** The messages before the oldest held. */
    suspend fun older(machine: String, agent: String) = core.older(machine, agent)

    /** A message of the user with its pictures, which the core checks before it sends. */
    suspend fun send(machine: String, agent: String, text: String, pictures: List<Picture>) =
        core.send(machine, agent, text, pictures)

    /** The answers to the ask `asked`, one [Reply] per question. */
    suspend fun answerAsk(machine: String, agent: String, asked: Long, replies: List<Reply>) =
        core.answerAsk(machine, agent, asked.toULong(), replies)

    /** The session's mode: `ask`, `auto` or `full`. */
    suspend fun setMode(machine: String, agent: String, mode: String) = core.setMode(machine, agent, mode)

    /** The bytes of a picture of the conversation. */
    suspend fun image(machine: String, agent: String, name: String): ByteArray = core.image(machine, agent, name)

    /** The tunnel to the run of `machine` that `url` names, and the address read through it. */
    suspend fun openAddress(machine: String, run: String?, url: String) = core.openAddress(machine, run, url)

    /** Closes a tunnel the phone holds. */
    fun closeTunnel(tunnel: String) = core.closeTunnel(tunnel)

    override fun close() = core.close()

    companion object {
        /**
         * The core's folder: in `noBackupFilesDir`, never in a backup. The endpoint's key is there
         * in clear (the app's private storage), and a device key copied to another phone would be
         * one its Keystore cannot open.
         */
        fun folder(context: Context): File = File(context.noBackupFilesDir, "flottille-core")

        /** The hub a pair code names, read with no network: its address and its key's fingerprint. */
        fun card(code: String): org.flottille.core.ffi.HubCard = hubCardOf(code)

        /** Whether an address names the machine it is read on (so that it opens through a tunnel). */
        fun isLocal(url: String): Boolean = addressIsLocalIn(url)

        /** At the app's start: a panic of the core is kept in `folder`. */
        fun keepPanics(folder: File) = keepPanicsIn(folder.path)

        /** The panic kept at the last run, once; to show at this start. */
        fun lastPanic(folder: File): String? = lastPanicIn(folder.path)
    }
}
