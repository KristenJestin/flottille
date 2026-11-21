package org.flottille.app

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import java.util.concurrent.Executors
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.FButton
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Space
import org.flottille.app.ui.Type

/**
 * The QR reader of the hub's pair code: the camera's frames, read by ZXing (no Google service on
 * the phone). Without the camera's permission it asks, and says the code can be pasted instead.
 */
@Composable
fun CameraScanner(onCode: (String) -> Unit) {
    val context = LocalContext.current
    var granted by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    if (!granted) {
        Column(
            Modifier.fillMaxSize().padding(Space.Lg),
            verticalArrangement = Arrangement.spacedBy(Space.Md, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Label("The camera reads the code. You can also paste it below.", style = Type.Label, color = LocalPalette.current.mute)
            FButton("Allow the camera", { ask.launch(Manifest.permission.CAMERA) }, kind = ButtonKind.Secondary)
        }
        return
    }
    val owner = LocalLifecycleOwner.current
    val latest by rememberUpdatedState(onCode)
    val main = remember { ContextCompat.getMainExecutor(context) }
    val worker = remember { Executors.newSingleThreadExecutor() }
    DisposableEffect(Unit) { onDispose { worker.shutdown() } }
    AndroidView(
        modifier = Modifier.fillMaxSize(),
        factory = { ctx ->
            PreviewView(ctx).also { view ->
                val future = ProcessCameraProvider.getInstance(ctx)
                future.addListener(
                    {
                        val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
                        val analysis = ImageAnalysis.Builder()
                            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                            .build()
                            .also { it.setAnalyzer(worker, QrAnalyzer { code -> main.execute { latest(code) } }) }
                        val provider = future.get()
                        provider.unbindAll()
                        provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
                    },
                    main,
                )
            }
        },
    )
}

/** Reads a QR code out of the luminance of a camera frame, whatever its orientation. */
private class QrAnalyzer(private val found: (String) -> Unit) : ImageAnalysis.Analyzer {
    private val reader = MultiFormatReader().apply {
        setHints(
            mapOf(
                DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
                DecodeHintType.TRY_HARDER to true,
            ),
        )
    }

    override fun analyze(image: ImageProxy) {
        try {
            val plane = image.planes[0]
            val bytes = ByteArray(plane.buffer.remaining()).also { plane.buffer.get(it) }
            val source = PlanarYUVLuminanceSource(bytes, plane.rowStride, image.height, 0, 0, image.width, image.height, false)
            found(reader.decodeWithState(BinaryBitmap(HybridBinarizer(source))).text)
        } catch (_: Exception) {
            // No code in this frame: the next one.
        } finally {
            reader.reset()
            image.close()
        }
    }
}
