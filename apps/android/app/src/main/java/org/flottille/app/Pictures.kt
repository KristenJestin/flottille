package org.flottille.app

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.activity.ComponentActivity
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.FileProvider
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.InputStream
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

/**
 * Where the pictures of a message come from: the gallery and the camera. The system's
 * ([SystemPictures]) never needs a storage permission; the tests play another. A picture's bytes
 * are in memory only for the time of the message: nothing of one is logged or kept.
 */
interface PictureSource {
    /** The pictures the user picks from the gallery; none when they pick none. */
    fun gallery(done: (List<Picture>) -> Unit)

    /** A picture the camera takes; none when the user leaves it. */
    fun camera(done: (Picture?) -> Unit)

    /** A phone with no way to pick: the composer offers none. */
    object None : PictureSource {
        override fun gallery(done: (List<Picture>) -> Unit) = done(emptyList())

        override fun camera(done: (Picture?) -> Unit) = done(null)
    }
}

/**
 * The system's pictures, registered by the activity before it starts: the photo picker (no
 * permission), and the camera writing into a file of the app's cache that only a FileProvider
 * gives out, for the time of the shot. Read, then deleted.
 */
class SystemPictures(private val activity: ComponentActivity) : PictureSource {
    private var galleryDone: ((List<Picture>) -> Unit)? = null
    private var cameraDone: ((Picture?) -> Unit)? = null
    private var shot: File? = null

    private val gallery = activity.registerForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(PictureLimits.MOST)) { uris ->
        galleryDone?.invoke(uris.mapNotNull { read(activity, it) })
        galleryDone = null
    }

    private val camera = activity.registerForActivityResult(ActivityResultContracts.TakePicture()) { taken ->
        val file = shot
        shot = null
        val picture = if (taken && file != null) {
            file.inputStream().use { fitted(it, "image/jpeg", file.name) }
        } else {
            null
        }
        file?.delete()
        cameraDone?.invoke(picture)
        cameraDone = null
    }

    private val permission = activity.registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) launchCamera() else finish(null)
    }

    override fun gallery(done: (List<Picture>) -> Unit) {
        galleryDone = done
        gallery.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
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

    private fun launchCamera() {
        val folder = File(activity.cacheDir, "pictures").apply { mkdirs() }
        val file = File.createTempFile("photo-", ".jpg", folder)
        shot = file
        camera.launch(FileProvider.getUriForFile(activity, "${activity.packageName}.pictures", file))
    }

    private fun finish(picture: Picture?) {
        cameraDone?.invoke(picture)
        cameraDone = null
    }
}

/** A picture the system gave by [uri] (a gallery pick, a share), or null when it cannot be read. */
fun read(context: Context, uri: Uri): Picture? {
    val mime = context.contentResolver.getType(uri) ?: return null
    val name = uri.lastPathSegment?.substringAfterLast('/').orEmpty().ifEmpty { "picture" }
    return context.contentResolver.openInputStream(uri)?.use { fitted(it, mime, name) }
}

/** The pictures an intent shares to the app (an intent filter for images only), read. */
fun shared(context: Context, intent: Intent): List<Picture> {
    // The typed getters need API 33; the app's minimum is 31.
    @Suppress("DEPRECATION")
    val uris = when (intent.action) {
        Intent.ACTION_SEND -> listOfNotNull(intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))
        Intent.ACTION_SEND_MULTIPLE -> intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
        else -> emptyList()
    }
    return uris.take(PictureLimits.MOST).mapNotNull { read(context, it) }
}

/** At most [limit] bytes of [stream]: a file of any size never fills the memory. */
private fun readAtMost(stream: InputStream, limit: Int): ByteArray {
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(64 * 1024)
    var total = 0
    while (total < limit) {
        val read = stream.read(buffer, 0, minOf(buffer.size, limit - total))
        if (read < 0) break
        out.write(buffer, 0, read)
        total += read
    }
    return out.toByteArray()
}

/**
 * The picture [stream] holds, scaled down to 2000 px on its long side when it is over the limit
 * (a phone's camera makes more than 5 MiB), as the daemon scales what it is given. A gif or a
 * webp that is too big is left as it is: the composer refuses it.
 */
private fun fitted(stream: InputStream, mime: String, name: String): Picture {
    val bytes = readAtMost(stream, PictureLimits.BYTES * 4 + 1)
    if (bytes.size <= PictureLimits.BYTES || mime !in listOf("image/jpeg", "image/png", "image/webp")) {
        return Picture(mime, bytes, name)
    }
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / sample > 2000) sample *= 2
    val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
        ?: return Picture(mime, bytes, name)
    var quality = 90
    while (true) {
        val out = ByteArrayOutputStream()
        bitmap.compress(Bitmap.CompressFormat.JPEG, quality, out)
        if (out.size() <= PictureLimits.BYTES || quality <= 40) {
            bitmap.recycle()
            return Picture("image/jpeg", out.toByteArray(), name.substringBeforeLast('.') + ".jpg")
        }
        quality -= 15
    }
}
