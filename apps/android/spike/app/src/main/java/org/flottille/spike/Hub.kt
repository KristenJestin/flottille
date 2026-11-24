package org.flottille.spike

import android.content.Context
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.InetSocketAddress
import java.net.Socket
import java.net.URI
import java.security.MessageDigest
import java.security.SecureRandom
import org.json.JSONObject

/** An invitation's code (`crates/flottille-link/src/invitation.rs`). */
class Invitation(val hubKey: ByteArray, val inviter: ByteArray, val token: ByteArray, val hubUrl: String) {
    companion object {
        private const val ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"

        fun decode(code: String): Invitation {
            val out = java.io.ByteArrayOutputStream()
            var buffer = 0
            var bits = 0
            for (c in code) {
                if (c == '-' || c.isWhitespace()) continue
                val value = when (val upper = c.uppercaseChar()) {
                    'O' -> 0
                    'I', 'L' -> 1
                    else -> ALPHABET.indexOf(upper).also { require(it >= 0) { "this code is damaged" } }
                }
                buffer = (buffer shl 5) or value
                bits += 5
                if (bits >= 8) {
                    bits -= 8
                    out.write((buffer shr bits) and 0xff)
                }
                buffer = buffer and ((1 shl bits) - 1)
            }
            val bytes = out.toByteArray()
            require(bytes.size > 81 + 4) { "this code is damaged" }
            val payload = bytes.copyOfRange(0, bytes.size - 4)
            val sum = MessageDigest.getInstance("SHA-256")
                .digest("flottille-invitation-v1\u0000".toByteArray() + payload).copyOfRange(0, 4)
            require(sum.contentEquals(bytes.copyOfRange(bytes.size - 4, bytes.size))) { "this code is damaged" }
            require(payload[0].toInt() == 1) { "this code is of another version" }
            return Invitation(
                payload.copyOfRange(1, 33),
                payload.copyOfRange(33, 65),
                payload.copyOfRange(65, 81),
                String(payload.copyOfRange(81, payload.size)),
            )
        }
    }

    /** The four words (`crates/flottille-link/src/words.rs`) of this invitation for `key`. */
    fun words(context: Context, key: ByteArray): List<String> {
        val list = context.assets.open("words.txt").bufferedReader().readLines()
        val digest = MessageDigest.getInstance("SHA-256")
            .digest("flottille-pairing-words-v1\u0000".toByteArray() + key + inviter + token)
        var bits = 0L
        for (i in 0 until 6) bits = (bits shl 8) or (digest[i].toLong() and 0xff)
        bits = bits shr 4
        return (0 until 4).map { list[((bits shr (33 - 11 * it)) and 0x7ff).toInt()] }
    }
}

/**
 * The join through the hub (`docs/remote.md`, "Pairing"), the phone's side of the control
 * session of `crates/flottille-link/src/control.rs`: the upgrade, the handshake that proves both
 * keys, and the join request; then the wait until the member list the hub sends holds this key.
 *
 * Throwaway: the list's signatures are not checked back to the root here; the mobile core does
 * it (#824).
 */
object Hub {
    private val SESSION = "flottille-hub-session-v1\u0000".toByteArray()

    private fun proofBytes(side: Int, other: ByteArray, theirs: ByteArray, own: ByteArray) =
        SESSION + byteArrayOf(side.toByte()) + other + theirs + own

    fun join(context: Context, invitation: Invitation, waitMs: Int = 6 * 60 * 1000) {
        val own = Keys.devicePublic(context)
        val url = URI(invitation.hubUrl)
        require(url.scheme == "http") { "the spike reaches a plain hub only (a test machine's)" }
        val port = if (url.port == -1) 80 else url.port
        Socket().use { socket ->
            socket.connect(InetSocketAddress(url.host, port), 10_000)
            socket.soTimeout = 10_000
            val output = socket.getOutputStream()
            val input = BufferedReader(InputStreamReader(socket.getInputStream()))
            val path = url.rawPath.ifEmpty { "/" }
            output.write(
                "GET $path HTTP/1.1\r\nHost: ${url.host}:$port\r\nConnection: Upgrade\r\nUpgrade: flottille-hub\r\n\r\n"
                    .toByteArray(),
            )
            output.flush()
            val status = input.readLine() ?: error("the hub closed the connection")
            require(status.startsWith("HTTP/1.1 101")) { "the hub answered $status" }
            while ((input.readLine() ?: "").isNotEmpty()) Unit
            fun send(value: JSONObject) {
                output.write((value.toString() + "\n").toByteArray())
                output.flush()
            }
            val nonce = ByteArray(32).also { SecureRandom().nextBytes(it) }
            val hello = JSONObject().put("flottille", "0.4.0-spike").put("protocols", org.json.JSONArray().put(1))
            send(JSONObject().put("key", own.hex()).put("nonce", nonce.hex()).put("hello", hello))
            val theirs = JSONObject(input.readLine() ?: error("the hub closed the connection"))
            theirs.optString("refused").takeIf { it.isNotEmpty() }?.let { error("the hub refused: $it") }
            val hubKey = theirs.getString("hub").unhex()
            require(hubKey.contentEquals(invitation.hubKey)) { "the hub has another key than the invitation's" }
            val hubNonce = theirs.getString("nonce").unhex()
            require(Keys.verify(hubKey, proofBytes(1, own, nonce, hubNonce), theirs.getString("proof").unhex())) {
                "the hub did not prove it holds its key"
            }
            val proof = Keys.sign(context, proofBytes(2, hubKey, hubNonce, nonce))
            send(
                JSONObject().put("proof", proof.hex()).put("pinned", JSONObject.NULL)
                    .put("join", JSONObject().put("token", invitation.token.hex()).put("key", own.hex())),
            )
            val welcome = JSONObject(input.readLine() ?: error("the hub closed the connection"))
            welcome.optJSONObject("refused")?.let { error("the hub refused: ${it.optString("reason")}") }
            require(welcome.has("welcome")) { "the hub said $welcome" }
            Store.log(context, "join requested, waiting for the member to accept")
            socket.soTimeout = waitMs
            while (true) {
                val line = input.readLine() ?: error("the hub ended the join")
                val frame = JSONObject(line)
                frame.optJSONObject("refused")?.let { error("the hub refused: ${it.optString("reason")}") }
                // A member's entry holds its key in hexadecimal: the list holding ours is the
                // addition the member signed.
                if (frame.has("list") && line.contains(own.hex())) break
            }
            Store.prefs(context).edit().putString("hub", invitation.hubKey.hex()).commit()
        }
    }
}
