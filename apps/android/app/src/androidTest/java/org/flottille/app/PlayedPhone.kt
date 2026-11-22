package org.flottille.app

import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import kotlin.concurrent.thread
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import org.flottille.core.HubCard
import org.flottille.core.OpenedTunnel
import org.flottille.core.Phone
import org.flottille.core.PhoneException
import org.flottille.core.PhoneView
import org.flottille.core.Picture
import org.flottille.core.Reply
import org.flottille.core.Tunnel
import org.flottille.core.TunnelEnd

/** What the real core listens on: `127.0.0.1`, never another address. */
val IPV4_LOOPBACK: InetAddress = InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1))

/**
 * A fake server standing for a run's port on a machine: it answers each request with a page and
 * keeps what it was asked, and by whom (the web view's `User-Agent` says it was the app's).
 */
class FakeRunServer : AutoCloseable {
    private val socket = ServerSocket(0, 50, IPV4_LOOPBACK)
    val port: Int get() = socket.localPort
    val requests: MutableList<String> = java.util.Collections.synchronizedList(mutableListOf())

    init {
        thread(isDaemon = true) {
            while (!socket.isClosed) {
                val client = try {
                    socket.accept()
                } catch (closed: java.io.IOException) {
                    return@thread
                }
                thread(isDaemon = true) {
                    client.use {
                        val head = it.getInputStream().bufferedReader().let { reader ->
                            generateSequence { reader.readLine() }.takeWhile { line -> line.isNotEmpty() }.toList()
                        }
                        requests += head.joinToString("\n")
                        val body = "<html><head><title>hello from the run</title></head><body>hello from the run</body></html>"
                        it.getOutputStream().write(
                            "HTTP/1.0 200 OK\r\nContent-Type: text/html\r\nContent-Length: ${body.length}\r\n\r\n$body".toByteArray(),
                        )
                    }
                }
            }
        }
    }

    override fun close() = socket.close()
}

/**
 * The core, played for the emulator (the issue's "fake core and fake server"): a real listener on
 * the phone's loopback whose connections go to [server], as a tunnel's do, and the view the screens
 * draw. The machine's side (the run ends, the link drops) is [endTunnel].
 */
class PlayedPhone(private val server: FakeRunServer, initial: PhoneView) : Phone {
    val state = MutableStateFlow(initial)
    val opened = java.util.Collections.synchronizedList(mutableListOf<Triple<String, String?, String>>())
    val closed = java.util.Collections.synchronizedList(mutableListOf<String>())

    @Volatile
    var holds = 0
    private val listeners = mutableMapOf<String, ServerSocket>()
    private var count = 0

    override val view: PhoneView get() = state.value

    override fun views(): Flow<PhoneView> = state

    override fun card(code: String): HubCard = throw PhoneException("not played")

    override suspend fun join(code: String, name: String, role: String) = throw PhoneException("not played")

    override suspend fun leave() = throw PhoneException("not played")

    override suspend fun follow(machine: String, agent: String) = Unit

    override suspend fun older(machine: String, agent: String) = Unit

    override fun unfollow() = Unit

    override suspend fun send(machine: String, agent: String, text: String, pictures: List<Picture>) = Unit

    override suspend fun answerAsk(machine: String, agent: String, asked: Long, replies: List<Reply>) = Unit

    override suspend fun answer(machine: String, number: Long, allow: Boolean) = Unit

    override suspend fun setMode(machine: String, agent: String, mode: String) = Unit

    override suspend fun stop(machine: String, agent: String) = Unit

    override suspend fun picture(machine: String, agent: String, name: String): ByteArray = ByteArray(0)

    override suspend fun openAddress(machine: String, run: String?, url: String, browser: Boolean): OpenedTunnel {
        opened += Triple(machine, run, url)
        val listener = ServerSocket(0, 50, IPV4_LOOPBACK)
        val tunnel = Tunnel("t${++count}", machine, run ?: "r1", server.port, listener.localPort)
        listeners[tunnel.tunnel] = listener
        thread(isDaemon = true) {
            while (!listener.isClosed) {
                val local = try {
                    listener.accept()
                } catch (closed: java.io.IOException) {
                    return@thread
                }
                pipe(local)
            }
        }
        state.value = state.value.copy(tunnels = state.value.tunnels + tunnel)
        val local = url.replace(Regex("^(\\w+://)[^/?#]+"), "$1localhost:${tunnel.localPort}")
        return OpenedTunnel(tunnel, local)
    }

    /** What a tunnel does with a connection: the bytes go to the run's port, and come back. */
    private fun pipe(local: Socket) {
        // A probe of the listener, or a test that ended with the server: the connection just ends.
        val remote = try {
            Socket(IPV4_LOOPBACK, server.port)
        } catch (refused: java.io.IOException) {
            local.close()
            return
        }
        fun copy(from: Socket, to: Socket) = thread(isDaemon = true) {
            try {
                from.getInputStream().copyTo(to.getOutputStream())
                to.shutdownOutput()
            } catch (ended: java.io.IOException) {
                // The other side ended: so does this one.
            }
        }
        copy(local, remote)
        copy(remote, local)
    }

    override fun closeTunnel(tunnel: String) {
        closed += tunnel
        end(tunnel, "closed by the user")
    }

    /** The machine ends the tunnel: its run ended, or the link dropped. */
    fun endTunnel(tunnel: String, reason: String) = end(tunnel, reason)

    @Synchronized
    private fun end(tunnel: String, reason: String) {
        listeners.remove(tunnel)?.close()
        val held = state.value.tunnels.firstOrNull { it.tunnel == tunnel } ?: throw PhoneException("no tunnel $tunnel is held by this phone")
        state.value = state.value.copy(
            tunnels = state.value.tunnels - held,
            tunnelsEnded = state.value.tunnelsEnded + TunnelEnd(held.tunnel, held.machine, held.run, reason),
        )
    }

    /** Whether something still listens on the tunnel's local port. */
    fun listening(tunnel: Tunnel): Boolean =
        try {
            Socket(IPV4_LOOPBACK, tunnel.localPort).use { true }
        } catch (refused: java.io.IOException) {
            false
        }

    override fun isLocal(url: String): Boolean =
        Regex("^\\w+://(localhost|127\\.0\\.0\\.1|\\[::1]|0\\.0\\.0\\.0)(:\\d+)?([/?#]|$)").containsMatchIn(url)

    override fun keepConnected(): AutoCloseable {
        holds += 1
        return AutoCloseable { holds -= 1 }
    }
}
