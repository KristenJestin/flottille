package org.flottille.core

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.runBlocking
import org.flottille.core.ffi.MobileException
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith

/** The core on the emulator: its library loaded for the device's ABI, its key sealed by the Keystore. */
@RunWith(AndroidJUnit4::class)
class OnThePhoneTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext

    @Test
    fun the_keystore_seals_and_opens_the_device_key() {
        val sealer = KeystoreSealer("flottille-test")
        val plain = ByteArray(32) { it.toByte() }
        val sealed = sealer.seal(plain)
        assertFalse(sealed.contentEquals(plain))
        assertArrayEquals(plain, sealer.open(sealed))
        sealed[sealed.size - 1] = (sealed.last() + 1).toByte()
        try {
            sealer.open(sealed)
            fail("a sealed key changed is refused")
        } catch (_: MobileException.Refused) {
        }
    }

    @Test
    fun the_core_keeps_its_folder_out_of_backups() {
        val folder = FlottilleCore.folder(context)
        assertTrue(folder.path, folder.canonicalPath.startsWith(context.noBackupFilesDir.canonicalPath))
        FlottilleCore(context, KeystoreSealer("flottille-test")).use { core -> assertFalse(core.joined) }
        assertTrue(File(folder, "endpoint").isDirectory)
    }

    @Test
    fun the_core_loads_on_the_phone_and_says_what_it_refuses() = runBlocking {
        val folder = File(context.filesDir, "core-test").apply { deleteRecursively() }
        FlottilleCore.keepPanics(folder.apply { mkdirs() })
        FlottilleCore(folder, KeystoreSealer("flottille-test")).use { core ->
            assertNull(core.fingerprint)
            try {
                core.join("not a code", "phone")
                fail("a code that is not one is refused")
            } catch (refused: MobileException.Refused) {
                assertTrue(refused.why.isNotEmpty())
            }
        }
        assertNull(FlottilleCore.lastPanic(folder))
    }
}
