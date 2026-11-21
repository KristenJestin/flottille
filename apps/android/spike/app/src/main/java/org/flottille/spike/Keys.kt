package org.flottille.spike

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.SecureRandom
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.PKCS8EncodedKeySpec
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.params.Ed25519PublicKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer

/**
 * The phone's keys (`docs/remote.md`, "Identity and keys"): the device key, Ed25519, made by the
 * app and kept encrypted under an AES key of the Android Keystore; and the push subscription's
 * P-256 key and auth secret (RFC 8291), kept the same way.
 */
object Keys {
    private const val SEAL = "flottille-spike-seal"

    private fun seal(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(SEAL, null) as SecretKey?)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(SEAL, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return generator.generateKey()
    }

    private fun sealed(context: Context, name: String, make: () -> ByteArray): ByteArray {
        val prefs = Store.prefs(context)
        prefs.getString(name, null)?.let { text ->
            val bytes = Base64.decode(text, Base64.NO_WRAP)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, seal(), GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
            return cipher.doFinal(bytes, 12, bytes.size - 12)
        }
        val secret = make()
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, seal())
        val text = Base64.encodeToString(cipher.iv + cipher.doFinal(secret), Base64.NO_WRAP)
        prefs.edit().putString(name, text).commit()
        return secret
    }

    private fun random(size: Int) = ByteArray(size).also { SecureRandom().nextBytes(it) }

    fun device(context: Context): Ed25519PrivateKeyParameters =
        Ed25519PrivateKeyParameters(sealed(context, "device", { random(32) }), 0)

    fun devicePublic(context: Context): ByteArray = device(context).generatePublicKey().encoded

    fun sign(context: Context, bytes: ByteArray): ByteArray {
        val signer = Ed25519Signer()
        signer.init(true, device(context))
        signer.update(bytes, 0, bytes.size)
        return signer.generateSignature()
    }

    fun verify(key: ByteArray, bytes: ByteArray, signature: ByteArray): Boolean {
        val verifier = Ed25519Signer()
        verifier.init(false, Ed25519PublicKeyParameters(key, 0))
        verifier.update(bytes, 0, bytes.size)
        return verifier.verifySignature(signature)
    }

    /** The push subscription's private key, PKCS#8. */
    fun pushPrivate(context: Context): java.security.PrivateKey {
        val encoded = sealed(context, "push-key") {
            val generator = KeyPairGenerator.getInstance("EC")
            generator.initialize(ECGenParameterSpec("secp256r1"))
            val pair = generator.generateKeyPair()
            Store.prefs(context).edit()
                .putString("push-public", Base64.encodeToString(uncompressed(pair.public as ECPublicKey), Base64.NO_WRAP))
                .commit()
            pair.private.encoded
        }
        return KeyFactory.getInstance("EC").generatePrivate(PKCS8EncodedKeySpec(encoded))
    }

    /** The subscription's public key, uncompressed (65 bytes), as `p256dh` says it. */
    fun pushPublic(context: Context): ByteArray {
        pushPrivate(context)
        return Base64.decode(Store.prefs(context).getString("push-public", "")!!, Base64.NO_WRAP)
    }

    fun pushAuth(context: Context): ByteArray = sealed(context, "push-auth") { random(16) }

    fun uncompressed(key: ECPublicKey): ByteArray {
        fun fixed(value: java.math.BigInteger): ByteArray {
            val bytes = value.toByteArray()
            return when {
                bytes.size == 32 -> bytes
                bytes.size > 32 -> bytes.copyOfRange(bytes.size - 32, bytes.size)
                else -> ByteArray(32 - bytes.size) + bytes
            }
        }
        return byteArrayOf(4) + fixed(key.w.affineX) + fixed(key.w.affineY)
    }
}

fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

fun String.unhex(): ByteArray = chunked(2).map { it.toInt(16).toByte() }.toByteArray()
