package org.flottille.app.ui

import androidx.compose.foundation.Image
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.foundation.layout.size
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * The app's icons: the strokes of the prototype the maintainer validated (24 units, a 1.8 line,
 * round ends), drawn in the ink they are given. No icon library: these few are the app's own.
 */
object Icons {
    private fun circle(cx: Float, cy: Float, r: Float) = "M${cx - r} ${cy}a$r $r 0 1 0 ${2 * r} 0a$r $r 0 1 0 ${-2 * r} 0"

    private fun icon(name: String, vararg paths: String): ImageVector =
        ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f).apply {
            for (d in paths) {
                addPath(
                    pathData = addPathNodes(d),
                    stroke = SolidColor(Color.Black),
                    strokeLineWidth = 1.8f,
                    strokeLineCap = StrokeCap.Round,
                    strokeLineJoin = StrokeJoin.Round,
                )
            }
        }.build()

    val Back = icon("back", "M15 5l-7 7 7 7")
    val Chevron = icon("chevron", "M9 6l6 6-6 6")
    val Close = icon("close", "M6 6l12 12M18 6L6 18")
    val Check = icon("check", "M5 12l4 4 10-10")
    val Send = icon("send", "M5 12h13M13 6l6 6-6 6")
    val Stop = icon("stop", "M8.5 7h7a1.5 1.5 0 0 1 1.5 1.5v7a1.5 1.5 0 0 1-1.5 1.5h-7a1.5 1.5 0 0 1-1.5-1.5v-7a1.5 1.5 0 0 1 1.5-1.5z")
    val Image = icon("image", "M5 5h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z", circle(9f, 10f, 1.8f), "M21 16l-5-5-9 8")
    val Camera = icon("camera", "M4 8h3l2-3h6l2 3h3v11H4z", circle(12f, 13f, 3.5f))
    val Tree = icon("tree", circle(6f, 5f, 2f), circle(18f, 12f, 2f), circle(18f, 19f, 2f), "M6 7v10a2 2 0 0 0 2 2h8M6 10a2 2 0 0 0 2 2h8")
    val Report = icon("report", "M5 4h14v16H5z", "M9 9h6M9 13h6M9 17h3")
    val Bell = icon("bell", "M6 16V11a6 6 0 0 1 12 0v5l2 2H4z", "M10 20a2 2 0 0 0 4 0")
    val Spark = icon("spark", "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z")
    val File = icon("file", "M6 3h8l4 4v14H6z", "M14 3v4h4")
    val Down = icon("down", "M12 5v14M6 13l6 6 6-6")
}

/** An icon in [tint], [size] wide; it says nothing to a screen reader: its control does. */
@Composable
fun Glyph(icon: ImageVector, tint: Color, modifier: Modifier = Modifier, size: Dp = 20.dp) {
    Image(rememberVectorPainter(icon), null, modifier.size(size), colorFilter = ColorFilter.tint(tint))
}
