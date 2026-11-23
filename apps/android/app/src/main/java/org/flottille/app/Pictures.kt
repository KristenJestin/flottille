package org.flottille.app

import android.Manifest
import android.content.ContentResolver
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.activity.ComponentActivity
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.FileProvider
import androidx.lifecycle.lifecycleScope
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.InputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.flottille.core.Picture

/** The window's limits on the pictures of a message (`docs/protocol.md`, `agent.send`). */
object PictureLimits {
    const val MOST = 20
    const val BYTES = 5 * 1024 * 1024
    val TYPES = listOf("image/png", "image/jpeg", "image/gif", "image/webp")

    /** Why [add] cannot join the [held] pictures of a message, or null when it can. */
    fun refusal(held: List<Picture>, add: Picture): String? =
        when {
            add.mime !in TYPES -> "${add.name.ifEmpty { "this file" }} is not a picture Flottille takes (png, jpeg, gif or webp)"
            add.bytes.isEmpty() -> "${add.name.ifEmpty { "this picture" }} is empty"
            add.bytes.size > BYTES -> "${add.name.ifEmpty { "this picture" }} is over 5 MiB"
            held.size >= MOST -> "A message carries $MOST pictures at most"
            else -> null
        }
}

/** What a read of pictures gave: the pictures, and one sentence of why others were left out. */
class Pictured(val pictures: List<Picture>, val refused: String? = null)

/**
 * Where the pictures of a message come from: the gallery and the camera. The system's
 * ([SystemPictures]) never needs a storage permission; the tests play another. A picture's bytes
 * are in memory only for the time of the message: nothing of one is logged or kept.
 */
interface PictureSource {
    /** The pictures the user picks from the gallery; none when they pick none. */
    fun gallery(done: (Pictured) -> Unit)

    /** A picture the camera takes; none when the user leaves it. */
    fun camera(done: (Picture?) -> Unit)

    /** A phone with no way to pick: the composer offers none. */
    object None : PictureSource {
        override fun gallery(done: (Pictured) -> Unit) = done(Pictured(emptyList()))

        override fun camera(done: (Picture?) -> Unit) = done(null)
    }
}

/**
 * The system's pictures, registered by the activity before it starts: the photo picker (no
 * permission), and the camera writing into a file of the app's cache that only a FileProvider
 * gives out, for the time of the shot. Read, then deleted.
 */
class SystemPictures(private val activity: ComponentActivity) : PictureSource {
    private var galleryDone: ((Pictured) -> Unit)? = null
    private var cameraDone: ((Picture?) -> Unit)? = null
    private var shot: File? = null

    // What is read stays off the main thread: a picture can be megabytes.
    private val pick = activity.registerForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(PictureLimits.MOST)) { uris ->
        val done = galleryDone
        galleryDone = null
        activity.lifecycleScope.launch {
            val read = withContext(Dispatchers.IO) { readPictures(uris, ResolverReader(activity.contentResolver)) }
            done?.invoke(read)
        }
    }

    private val shoot = activity.registerForActivityResult(ActivityResultContracts.TakePicture()) { taken ->
        val file = shot
        shot = null
        val done = cameraDone
        cameraDone = null
        activity.lifecycleScope.launch {
            // Read, then deleted, whatever came of it: a shot is never left in the cache.
            val picture = withContext(Dispatchers.IO) {
                if (file == null) null else if (taken) readShot(file) else file.delete().let { null }
            }
            done?.invoke(picture)
        }
    }

    private val permission = activity.registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) launchCamera() else finish(null)
    }

    override fun gallery(done: (Pictured) -> Unit) {
        galleryDone = done
        pick.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
    }

    override fun camera(done: (Picture?) -> Unit) {
        cameraDone = done
        // The app declares the camera permission (the QR reader), so the camera app is refused
        // for it unless it is granted.
        if (activity.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            launchCamera()
        } else {
            permission.launch(Manifest.permission.CAMERA)
        }
    }

    /**
     * What a process that died between the shot and its read left behind. Called once the activity
     * has its context (onCreate): the system builds the activity first and attaches it after, so
     * its cache folder does not exist yet when this class is built.
     */
    fun clearLeftovers() {
        clearShots(File(activity.cacheDir, "pictures"))
    }

    private fun launchCamera() {
        val folder = File(activity.cacheDir, "pictures").apply { mkdirs() }
        val file = File.createTempFile("photo-", ".jpg", folder)
        shot = file
        shoot.launch(FileProvider.getUriForFile(activity, "${activity.packageName}.pictures", file))
    }

    private fun finish(picture: Picture?) {
        cameraDone?.invoke(picture)
        cameraDone = null
    }
}

/** What the system's content resolver says of a picture it gives out by a uri. */
interface PictureReader {
    fun type(uri: Uri): String?

    fun open(uri: Uri): InputStream?
}

/** The reader of the app: the content resolver. */
class ResolverReader(private val resolver: ContentResolver) : PictureReader {
    override fun type(uri: Uri): String? = resolver.getType(uri)

    override fun open(uri: Uri): InputStream? = resolver.openInputStream(uri)
}

/**
 * The pictures at [uris] (a gallery pick, a share), read for a message. A uri comes from another
 * app, so it is never trusted: one the app has no grant for, one that fails, one that is no picture
 * or is over [PictureLimits.BYTES] (refused before it is held, and never cut short) is left out and
 * said once. A picture is held as it is, never shrunk: the core sends it in chunks (#1187).
 */
fun readPictures(uris: List<Uri>, reader: PictureReader): Pictured {
    val pictures = mutableListOf<Picture>()
    var unread = 0
    var oversize: String? = null
    for (uri in uris.take(PictureLimits.MOST)) {
        try {
            val mime = reader.type(uri)
            val name = uri.lastPathSegment?.substringAfterLast('/').orEmpty().ifEmpty { "picture" }
            val picture = if (mime == null) null else reader.open(uri)?.use { fitted(it, mime, name, PictureLimits.BYTES) }
            if (picture == null) {
                unread += 1
            } else {
                pictures += picture
            }
        } catch (big: TooBig) {
            oversize = "${big.name.ifEmpty { "a picture" }} is over 5 MiB"
        } catch (_: Exception) {
            // No grant (SecurityException), a provider that died, a file that is gone: one picture
            // less, never the end of the app.
            unread += 1
        }
    }
    val said = listOfNotNull(
        oversize,
        if (unread > 0) "$unread picture${if (unread > 1) "s" else ""} could not be read" else null,
    ).joinToString(". ").ifEmpty { null }
    return Pictured(pictures, said)
}

/** The pictures an intent shares to the app (an intent filter for images only): their uris. */
@Suppress("DEPRECATION") // The typed getters need API 33; the app's minimum is 31.
fun sharedUris(intent: Intent): List<Uri> {
    // Another app wrote the extra: a value of another type, or one that cannot be read, is no
    // picture, never an exception.
    val extra = runCatching { intent.extras?.get(Intent.EXTRA_STREAM) }.getOrNull()
    return when (intent.action) {
        Intent.ACTION_SEND -> listOfNotNull(extra as? Uri)
        Intent.ACTION_SEND_MULTIPLE -> (extra as? List<*>).orEmpty().filterIsInstance<Uri>()
        else -> emptyList()
    }
}

/**
 * The shot the camera wrote into [file]: read, and deleted whatever comes of it. A photo of a phone
 * may be more than a message's picture takes, so it is read up to [CAMERA_BYTES] and scaled down
 * only when it is over [PictureLimits.BYTES]: it could not be sent otherwise.
 */
fun readShot(file: File): Picture? =
    try {
        file.inputStream().use { fitted(it, "image/jpeg", file.name, CAMERA_BYTES) }
    } catch (_: Exception) {
        null
    } finally {
        file.deleteRecursively()
    }

/** A picture longer than what is read of it: refused, never cut short. */
private class TooBig(val name: String) : Exception()

/** Removes what is left in the folder of the camera's shots (a process that died before its read). */
fun clearShots(folder: File) {
    folder.listFiles()?.forEach { it.deleteRecursively() }
}

/** What a shot of the camera is read up to, before it is scaled down. */
private const val CAMERA_BYTES = 20 * 1024 * 1024

/**
 * At most [limit] bytes of [stream], read plus one so that a stream longer than [limit] is seen:
 * a file of any size never fills the memory. The size is [limit] + 1 when it hit the cap.
 */
private fun readAtMost(stream: InputStream, limit: Int): ByteArray {
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(64 * 1024)
    var total = 0
    while (total <= limit) {
        val read = stream.read(buffer, 0, minOf(buffer.size, limit + 1 - total))
        if (read < 0) break
        out.write(buffer, 0, read)
        total += read
    }
    return out.toByteArray()
}

/**
 * The picture [stream] holds, as it is when it is [PictureLimits.BYTES] at most; over it (a camera's
 * shot alone may be), scaled down to 2000 px on its long side and re-encoded, as the daemon scales
 * what it is given. A gif or a webp is left as it is: the composer refuses one that does not fit.
 */
private fun fitted(stream: InputStream, mime: String, name: String, limit: Int): Picture {
    val bytes = readAtMost(stream, limit)
    // Past the limit it was cut short: never sent as a picture it is not.
    if (bytes.size > limit) throw TooBig(name)
    if (bytes.size <= PictureLimits.BYTES || mime !in listOf("image/jpeg", "image/png", "image/webp")) {
        return Picture(mime, bytes, name)
    }
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / sample > 2000) sample *= 2
    var bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
        ?: return Picture(mime, bytes, name)
    val jpeg = name.substringBeforeLast('.') + ".jpg"
    while (true) {
        for (quality in listOf(85, 70, 55)) {
            val out = ByteArrayOutputStream()
            bitmap.compress(Bitmap.CompressFormat.JPEG, quality, out)
            if (out.size() <= PictureLimits.BYTES) {
                bitmap.recycle()
                return Picture("image/jpeg", out.toByteArray(), jpeg)
            }
        }
        // Still too big: half the size, until it fits or there is nothing left to give.
        if (bitmap.width < 200 || bitmap.height < 200) {
            val out = ByteArrayOutputStream()
            bitmap.compress(Bitmap.CompressFormat.JPEG, 55, out)
            return Picture("image/jpeg", out.toByteArray(), jpeg)
        }
        bitmap = Bitmap.createScaledBitmap(bitmap, bitmap.width / 2, bitmap.height / 2, true)
    }
}
