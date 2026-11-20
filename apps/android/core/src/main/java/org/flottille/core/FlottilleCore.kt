package org.flottille.core

import java.io.File
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import org.flottille.core.ffi.Batch
import org.flottille.core.ffi.Core
import org.flottille.core.ffi.Listener
import org.flottille.core.ffi.Pairing
import org.flottille.core.ffi.Sealer
import org.flottille.core.ffi.ViewState
import org.flottille.core.ffi.keepPanics as keepPanicsIn
import org.flottille.core.ffi.lastPanic as lastPanicIn

/**
 * The mobile core, as the screens use it (`docs/remote.md`, "The Android app"): `connect` is
 * collecting [batches], then [request], [answer], [stop]. Everything that talks to a machine is in
 * the Rust core; this class only carries it.
 */
class FlottilleCore(folder: File, sealer: Sealer = KeystoreSealer()) : AutoCloseable {
    private val core = Core(folder.path, sealer)

    /** The device key's fingerprint, once this phone joined its hub. */
    val fingerprint: String? get() = core.fingerprint()

    val joined: Boolean get() = core.joined()

    /** The view state as it is now; each batch carries it too. */
    val view: ViewState get() = core.view()

    /** Starts joining from the QR's code: the pairing's words, then its `accepted()`. */
    suspend fun join(code: String): Pairing = core.join(code)

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

    override fun close() = core.close()

    companion object {
        /** At the app's start: a panic of the core is kept in `folder`. */
        fun keepPanics(folder: File) = keepPanicsIn(folder.path)

        /** The panic kept at the last run, once; to show at this start. */
        fun lastPanic(folder: File): String? = lastPanicIn(folder.path)
    }
}
