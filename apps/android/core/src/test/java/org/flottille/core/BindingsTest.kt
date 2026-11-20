package org.flottille.core

import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.flottille.core.ffi.MobileException
import org.flottille.core.ffi.Sealer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * The generated bindings, on this machine's JVM, over the core built for it: what crosses the
 * boundary (a foreign sealer, an error and its sentence, a Flow of batches, the panic kept).
 * Joining and linking are the Rust tests' (`crates/flottille-mobile/tests`).
 */
class BindingsTest {
    /** The Keystore, played. */
    private object Plain : Sealer {
        override fun seal(plain: ByteArray) = plain.map { (it.toInt() xor 0x5a).toByte() }.toByteArray()
        override fun open(sealed: ByteArray) = seal(sealed)
    }

    private fun folder(): File = Files.createTempDirectory("flottille-core").toFile()

    @Test
    fun a_core_opens_in_its_folder_and_is_no_member_before_it_joins() {
        FlottilleCore(folder(), Plain).use { core ->
            assertNull(core.fingerprint)
            assertFalse(core.joined)
            assertEquals("", core.view.role)
            assertTrue(core.view.machines.isEmpty())
        }
    }

    @Test
    fun a_code_that_is_not_a_pair_code_is_refused_with_its_sentence() = runBlocking {
        FlottilleCore(folder(), Plain).use { core ->
            try {
                core.join("not a code", "phone")
                fail("a code that is not one is refused")
            } catch (refused: MobileException.Refused) {
                assertTrue(refused.why, refused.why.isNotEmpty())
            }
        }
    }

    @Test
    fun the_batches_of_a_phone_that_joined_no_hub_say_so() = runBlocking {
        FlottilleCore(folder(), Plain).use { core ->
            try {
                core.batches().first()
                fail("no hub, no batch")
            } catch (refused: MobileException.Refused) {
                assertTrue(refused.why, refused.why.contains("joined no hub"))
            }
        }
    }

    @Test
    fun a_request_without_a_connection_is_refused() = runBlocking {
        FlottilleCore(folder(), Plain).use { core ->
            try {
                core.request("server", "project.list")
                fail("no connection, no request")
            } catch (refused: MobileException.Refused) {
                assertTrue(refused.why, refused.why.contains("not connected"))
            }
        }
    }

    @Test
    fun a_panic_kept_is_read_once_at_the_next_start() {
        val folder = folder()
        assertNull(FlottilleCore.lastPanic(folder))
        File(folder, "panic.txt").writeText("panicked at src/mobile.rs:1:1:\nthe core broke")
        assertEquals("panicked at src/mobile.rs:1:1:\nthe core broke", FlottilleCore.lastPanic(folder))
        assertNull(FlottilleCore.lastPanic(folder))
    }
}
