package org.flottille.app

import android.content.Intent
import android.net.Uri
import java.io.File
import java.io.InputStream
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * What the app reads of pictures it is given by the gallery, the share sheet and the camera
 * (#826): whatever another app hands over, the app is never crashed nor filled, and it never
 * sends a picture it cut short.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], application = android.app.Application::class)
class PicturesTest {
    @get:Rule
    val folder = TemporaryFolder()

    /** A reader that gives [bytes] by uri, counts what is opened and what is read. */
    private class Reader(val files: Map<Uri, () -> InputStream?>, val type: String = "image/gif") : PictureReader {
        val opened = mutableListOf<Uri>()

        override fun type(uri: Uri): String? = type

        override fun open(uri: Uri): InputStream? {
            opened += uri
            return files.getValue(uri)()
        }
    }

    private fun uri(n: Int): Uri = Uri.parse("content://media/external/images/$n")

    private fun of(size: Int): () -> InputStream? = { ByteArray(size) { 7 }.inputStream() }

    /** A stream that gives [size] bytes without holding them, and counts what was read. */
    private class Endless(var size: Long) : InputStream() {
        var read = 0L

        override fun read(): Int = if (read >= size) -1 else { read += 1; 7 }

        override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
            if (read >= size) return -1
            val count = minOf(length.toLong(), size - read).toInt()
            buffer.fill(7, offset, offset + count)
            read += count
            return count
        }
    }

    @Test
    fun a_uri_the_app_has_no_grant_for_is_skipped_and_the_others_are_read() {
        val reader = Reader(
            mapOf(
                uri(1) to { throw SecurityException("no grant") },
                uri(2) to of(1_000),
                uri(3) to { throw IllegalStateException("provider died") },
            ),
        )
        val read = readPictures(listOf(uri(1), uri(2), uri(3)), reader)
        assertEquals(1, read.pictures.size)
        assertEquals(1_000, read.pictures[0].bytes.size)
        // Said once, never with a path or a name of the other app's file.
        assertNotNull(read.refused)
    }

    @Test
    fun a_stream_that_is_not_a_uri_is_ignored_not_thrown() {
        val wrong = Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, "not a uri")
        assertEquals(emptyList<Uri>(), sharedUris(wrong))
        val wrongMany = Intent(Intent.ACTION_SEND_MULTIPLE).putExtra(Intent.EXTRA_STREAM, arrayOf("a", "b"))
        assertEquals(emptyList<Uri>(), sharedUris(wrongMany))
        val right = Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uri(1))
        assertEquals(listOf(uri(1)), sharedUris(right))
    }

    @Test
    fun a_picture_over_the_cap_is_refused_before_it_is_buffered() {
        val big = Endless(200L * 1024 * 1024)
        val reader = Reader(mapOf(uri(1) to { big }))
        val read = readPictures(listOf(uri(1)), reader)
        assertTrue(read.pictures.isEmpty())
        assertNotNull(read.refused)
        // It stopped reading just past the cap: never the whole of it, nor 4 times the cap.
        assertTrue("read ${big.read}", big.read <= PictureLimits.BYTES + 64 * 1024 + 1)
    }

    @Test
    fun a_picture_that_hit_the_cap_is_refused_never_sent_truncated() {
        val reader = Reader(mapOf(uri(1) to of(PictureLimits.BYTES + 1), uri(2) to of(PictureLimits.BYTES)))
        // One byte over: refused, not cut to the cap and sent.
        val over = readPictures(listOf(uri(1)), reader)
        assertTrue(over.pictures.isEmpty())
        assertTrue(over.refused!!.contains("5 MiB"))
        // Exactly the cap is a picture the window takes; the composer judges the frame.
        val exact = readPictures(listOf(uri(2)), reader)
        assertEquals(PictureLimits.BYTES, exact.pictures.single().bytes.size)
    }

    @Test
    fun the_pictures_of_a_share_are_read_until_the_link_frame_is_full() {
        val reader = Reader((1..8).associate { uri(it) to of(300_000) })
        val read = readPictures((1..8).map(::uri), reader)
        // 300 + 300 + 300 KB passes 700 KB at the third: no other is opened to be buffered.
        assertEquals(3, reader.opened.size)
        assertEquals(3, read.pictures.size)
    }

    @Test
    fun a_camera_shot_is_deleted_whatever_comes_of_it() {
        val file = folder.newFile("photo-1.jpg").apply { writeBytes(ByteArray(10) { 1 }) }
        val picture = readShot(file)
        assertEquals(10, picture!!.bytes.size)
        assertFalse("read, then deleted", file.exists())

        // A shot that cannot be read is deleted too, not left in the cache.
        val broken = folder.newFolder("photo-2.jpg")
        assertNull(readShot(broken))
        assertFalse(broken.exists())
    }

    @Test
    fun the_shots_a_dead_process_left_are_cleared_at_the_start() {
        val pictures = folder.newFolder("pictures")
        File(pictures, "photo-1.jpg").writeBytes(byteArrayOf(1))
        File(pictures, "photo-2.jpg").writeBytes(byteArrayOf(2))
        clearShots(pictures)
        assertEquals(0, pictures.listFiles()!!.size)
        // A folder that is not there is no error.
        clearShots(File(folder.root, "none"))
    }
}
