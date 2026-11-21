package org.flottille.core

import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The libraries the app ships, read as the phone's loader reads them: every LOAD segment of their
 * ELF program headers aligned on 16 KB, as Android 15's pages ask.
 */
class AlignmentTest {
    @Test
    fun the_libraries_are_aligned_on_sixteen_kilobytes() {
        val jniLibs = File(System.getProperty("flottille.jniLibs"))
        val abis = jniLibs.listFiles()!!.filter { it.isDirectory }.map { it.name }.sorted()
        assertEquals(listOf("arm64-v8a", "x86_64"), abis)
        for (abi in abis) {
            val library = File(jniLibs, "$abi/libflottille_mobile.so")
            val aligns = loadAlignments(library.readBytes())
            assertTrue("$abi has no LOAD segment", aligns.isNotEmpty())
            for (align in aligns) {
                assertTrue("$abi: a LOAD segment aligned on $align bytes", align >= 16384 && align % 16384 == 0L)
            }
        }
    }

    /** The `p_align` of each `PT_LOAD` header of a 64-bit little-endian ELF file. */
    private fun loadAlignments(bytes: ByteArray): List<Long> {
        val elf = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals(0x7f, bytes[0].toInt())
        assertEquals("ELF", String(bytes, 1, 3))
        assertEquals("a 64-bit library", 2, bytes[4].toInt())
        val headers = elf.getLong(0x20)
        val size = elf.getShort(0x36).toInt()
        val count = elf.getShort(0x38).toInt()
        return (0 until count).mapNotNull { n ->
            val at = (headers + n * size).toInt()
            if (elf.getInt(at) == 1) elf.getLong(at + 0x30) else null
        }
    }
}
