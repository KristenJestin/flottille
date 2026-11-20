package org.flottille.spike

import android.app.BroadcastOptions
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Base64
import java.nio.ByteBuffer
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import java.security.KeyFactory
import java.security.spec.ECPoint
import java.security.spec.ECPublicKeySpec
import java.security.interfaces.ECPublicKey
import org.json.JSONObject

/**
 * UnifiedPush, the connector's side (`unifiedpush.org/developers/spec/android`), written here
 * rather than taken from the connector library so that the spike sees every intent; and the Web
 * Push decryption (RFC 8291, aes128gcm).
 */
object Push {
    const val REGISTER = "org.unifiedpush.android.distributor.REGISTER"
    const val MESSAGE_ACK = "org.unifiedpush.android.distributor.MESSAGE_ACK"

    /** The distributors installed: the packages that take a registration. */
    fun distributors(context: Context): List<String> =
        context.packageManager.queryBroadcastReceivers(Intent(REGISTER), 0)
            .map { it.activityInfo.packageName }
            .filter { it != context.packageName }
            .distinct()

    /** Registers with `distributor`: its endpoint comes back as NEW_ENDPOINT (PushReceiver). */
    fun register(context: Context, distributor: String) {
        val token = UUID.randomUUID().toString()
        Store.prefs(context).edit().putString("token", token).putString("distributor", distributor).commit()
        val intent = Intent(REGISTER).setPackage(distributor)
            .putExtra("token", token)
            .putExtra("application", context.packageName)
            .putExtra("features", arrayOf("org.unifiedpush.android.distributor.feature.BYTES_MESSAGE"))
        // Who registers, for a distributor of the spec's third version: a PendingIntent below
        // Android 14, the identity shared from it on.
        val identity = PendingIntent.getBroadcast(
            context, 0, Intent("org.flottille.spike.IDENTITY").setPackage(context.packageName),
            PendingIntent.FLAG_IMMUTABLE,
        )
        intent.putExtra("pi", identity)
        if (Build.VERSION.SDK_INT >= 34) {
            val options = BroadcastOptions.makeBasic().setShareIdentityEnabled(true)
            context.sendBroadcast(intent, null, options.toBundle())
        } else {
            context.sendBroadcast(intent)
        }
        Store.log(context, "registering with $distributor")
    }

    fun ack(context: Context, id: String?) {
        val distributor = Store.prefs(context).getString("distributor", null) ?: return
        val token = Store.prefs(context).getString("token", null) ?: return
        context.sendBroadcast(Intent(MESSAGE_ACK).setPackage(distributor).putExtra("token", token).putExtra("id", id))
    }

    fun b64(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)

    /** The subscription the runner encrypts for: what `push.register` gives it. */
    fun subscription(context: Context, endpoint: String): JSONObject =
        JSONObject().put("endpoint", endpoint)
            .put("p256dh", b64(Keys.pushPublic(context)))
            .put("auth", b64(Keys.pushAuth(context)))

    private fun hmac(key: ByteArray, vararg parts: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        parts.forEach { mac.update(it) }
        return mac.doFinal()
    }

    private fun expand(prk: ByteArray, info: ByteArray, length: Int) =
        hmac(prk, info, byteArrayOf(1)).copyOfRange(0, length)

    /** The plaintext of an aes128gcm Web Push body, one record. */
    fun decrypt(context: Context, body: ByteArray): ByteArray {
        val buffer = ByteBuffer.wrap(body)
        val salt = ByteArray(16).also { buffer.get(it) }
        buffer.int // the record size
        val idLength = buffer.get().toInt() and 0xff
        val senderPublic = ByteArray(idLength).also { buffer.get(it) }
        val cipherText = ByteArray(buffer.remaining()).also { buffer.get(it) }

        val ownPublic = Keys.pushPublic(context)
        val agreement = KeyAgreement.getInstance("ECDH")
        agreement.init(Keys.pushPrivate(context))
        agreement.doPhase(publicKey(senderPublic, Keys.pushPrivate(context)), true)
        val shared = agreement.generateSecret()

        val prkKey = hmac(Keys.pushAuth(context), shared)
        val ikm = expand(prkKey, "WebPush: info\u0000".toByteArray() + ownPublic + senderPublic, 32)
        val prk = hmac(salt, ikm)
        val cek = expand(prk, "Content-Encoding: aes128gcm\u0000".toByteArray(), 16)
        val nonce = expand(prk, "Content-Encoding: nonce\u0000".toByteArray(), 12)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, SecretKeySpec(cek, "AES"), GCMParameterSpec(128, nonce))
        val record = cipher.doFinal(cipherText)
        // The padding delimiter: the last record's is 2, after the content.
        val end = record.indexOfLast { it.toInt() == 2 }
        require(end >= 0) { "a push record without its delimiter" }
        return record.copyOfRange(0, end)
    }

    private fun publicKey(uncompressed: ByteArray, like: java.security.PrivateKey): ECPublicKey {
        val params = (like as java.security.interfaces.ECPrivateKey).params
        val point = ECPoint(
            java.math.BigInteger(1, uncompressed.copyOfRange(1, 33)),
            java.math.BigInteger(1, uncompressed.copyOfRange(33, 65)),
        )
        return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(point, params)) as ECPublicKey
    }
}
