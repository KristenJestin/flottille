package org.flottille.spike

import android.content.Context
import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.nio.ByteBuffer
import java.security.SecureRandom
import org.json.JSONObject

/**
 * The address the phone knows, where it answers: the gate of the spike's server, which stands for
 * the link the mobile core opens (#824). Every request is signed with the device key in the very
 * encoding a link carries (`crates/flottille-link/src/request.rs`), and checked there as a daemon
 * checks one.
 */
object Gate {
    /** The machine the requests are for: the daemon's name in the member list. */
    const val MACHINE = "pc"

    fun address(context: Context): String = Store.prefs(context).getString("gate", "")!!

    fun signed(context: Context, method: String, params: JSONObject): String {
        val id = "phone-" + System.nanoTime()
        val device = Keys.devicePublic(context)
        val nonce = ByteArray(16).also { SecureRandom().nextBytes(it) }
        val at = System.currentTimeMillis()
        val paramsText = params.toString()
        val bytes = ByteArrayOutputStream()
        fun text(value: String) {
            val utf8 = value.toByteArray()
            bytes.write(ByteBuffer.allocate(4).putInt(utf8.size).array())
            bytes.write(utf8)
        }
        bytes.write("flottille-request-v1\u0000".toByteArray())
        text(id)
        text(MACHINE)
        bytes.write(device)
        bytes.write(nonce)
        bytes.write(ByteBuffer.allocate(8).putLong(at).array())
        text(method)
        text(paramsText)
        val sig = Keys.sign(context, bytes.toByteArray())
        return JSONObject()
            .put("id", id)
            .put("machine", MACHINE)
            .put(
                "signed",
                JSONObject().put("device", device.hex()).put("nonce", nonce.hex()).put("at", at)
                    .put("method", method).put("params", paramsText),
            )
            .put("sig", sig.hex())
            .toString()
    }

    /** POSTs `body` to the gate's `path`: its answer, or an error with the gate's refusal. */
    fun post(context: Context, path: String, body: String, timeoutMs: Int = 4_000): JSONObject {
        val connection = URL(address(context) + path).openConnection() as HttpURLConnection
        connection.connectTimeout = timeoutMs
        connection.readTimeout = timeoutMs
        connection.requestMethod = "POST"
        connection.doOutput = true
        connection.setRequestProperty("Content-Type", "application/json")
        try {
            connection.outputStream.use { it.write(body.toByteArray()) }
            val code = connection.responseCode
            val stream = if (code < 400) connection.inputStream else connection.errorStream
            val text = stream.bufferedReader().readText()
            if (code >= 400) error("the gate said $code: $text")
            return JSONObject(text)
        } finally {
            connection.disconnect()
        }
    }

    fun call(context: Context, method: String, params: JSONObject, timeoutMs: Int = 4_000): JSONObject =
        post(context, "/call", signed(context, method, params), timeoutMs)
}
