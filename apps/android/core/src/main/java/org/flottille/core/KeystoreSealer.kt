package org.flottille.core

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.GeneralSecurityException
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.flottille.core.ffi.MobileException
import org.flottille.core.ffi.Sealer

/**
 * Seals the device key for the core under an AES key of the Android Keystore, which never leaves
 * it (`docs/remote.md`, "Identity and keys"): what is sealed is the IV, then AES-GCM's output.
 */
class KeystoreSealer(private val alias: String = "flottille-device") : Sealer {
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as SecretKey?)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    // The Keystore throws `ProviderException` (a `RuntimeException`) as often as a
    // `GeneralSecurityException`: anything thrown is a refusal said, never a panic of the core.
    override fun seal(plain: ByteArray): ByteArray = try {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.iv + cipher.doFinal(plain)
    } catch (error: GeneralSecurityException) {
        throw MobileException.Refused("the device key cannot be sealed: ${error.message}")
    } catch (error: RuntimeException) {
        throw MobileException.Refused("the device key cannot be sealed: ${error.message}")
    }

    override fun open(sealed: ByteArray): ByteArray = try {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, sealed, 0, IV))
        cipher.doFinal(sealed, IV, sealed.size - IV)
    } catch (error: GeneralSecurityException) {
        throw MobileException.Refused("the device key cannot be opened: ${error.message}")
    } catch (error: RuntimeException) {
        throw MobileException.Refused("the device key cannot be opened: ${error.message}")
    }

    private companion object {
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV = 12
    }
}
