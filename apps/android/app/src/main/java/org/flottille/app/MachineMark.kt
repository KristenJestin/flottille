package org.flottille.app

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Motion
import org.flottille.core.MachineState
import org.flottille.core.Member

/**
 * The colours a device is drawn in, as the window's (`flottille-window`, `machines.rs`, `COLOURS`):
 * the word the member list signs and its shade; grey for a word it does not know.
 */
private val shades = mapOf(
    "grey" to Color(0xFF8B8F98),
    "red" to Color(0xFFE5534B),
    "orange" to Color(0xFFF0883E),
    "yellow" to Color(0xFFE3B341),
    "green" to Color(0xFF57AB5A),
    "teal" to Color(0xFF39C5BB),
    "blue" to Color(0xFF4F8CF7),
    "purple" to Color(0xFFA371F7),
)

fun shadeOf(colour: String): Color = shades[colour] ?: shades.getValue("grey")

/** A machine's badge: one shape per state (#1249). [Unknown] is a machine the list does not say. */
enum class Badge { Online, Offline, Update, Unknown }

fun badgeOf(state: MachineState?): Badge =
    when (state) {
        MachineState.Online -> Badge.Online
        MachineState.Offline -> Badge.Offline
        MachineState.NeedsUpdate -> Badge.Update
        null -> Badge.Unknown
    }

/**
 * A machine's mark (#1249): its icon in its colour (the member list's), and a badge of one shape
 * per state: a filled dot online, a hollow ring offline (the icon greyed), an amber dot with an
 * arrow when it needs an update, a dashed ring when its state is not known. The colours move over
 * [Motion.PANEL].
 */
@Composable
fun MachineMark(member: Member?, badge: Badge, modifier: Modifier = Modifier, size: Dp = 26.dp, said: String = "") {
    val palette = LocalPalette.current
    val ink by animateColorAsState(
        if (badge == Badge.Offline || badge == Badge.Unknown) palette.faint else shadeOf(member?.colour.orEmpty()),
        Motion.spec(Motion.PANEL),
        label = "machine ink",
    )
    val fill by animateColorAsState(
        when (badge) {
            Badge.Online -> palette.ok
            Badge.Update -> palette.wait
            Badge.Offline, Badge.Unknown -> palette.bg
        },
        Motion.spec(Motion.PANEL),
        label = "badge fill",
    )
    val icon = member?.icon.orEmpty()
    Box(modifier.size(size).semantics { if (said.isNotEmpty()) contentDescription = said }, contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(size * 0.7f)) { drawIcon(icon, ink) }
        Canvas(Modifier.size(size)) {
            val r = this.size.width * 0.2f
            val c = Offset(this.size.width - r, this.size.height - r)
            drawCircle(palette.bg, r + 2.dp.toPx(), c)
            drawCircle(fill, r, c)
            when (badge) {
                Badge.Offline -> drawCircle(palette.faint, r - 0.75.dp.toPx(), c, style = Stroke(1.5.dp.toPx()))
                Badge.Unknown -> drawCircle(
                    palette.faint,
                    r - 0.75.dp.toPx(),
                    c,
                    style = Stroke(1.5.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(2.dp.toPx(), 1.5.dp.toPx()))),
                )
                Badge.Update -> {
                    val w = 1.6.dp.toPx()
                    val a = r * 0.55f
                    drawLine(palette.bg, Offset(c.x, c.y + a), Offset(c.x, c.y - a), w, StrokeCap.Round)
                    drawLine(palette.bg, Offset(c.x - a * 0.8f, c.y - a * 0.2f), Offset(c.x, c.y - a), w, StrokeCap.Round)
                    drawLine(palette.bg, Offset(c.x + a * 0.8f, c.y - a * 0.2f), Offset(c.x, c.y - a), w, StrokeCap.Round)
                }
                Badge.Online -> {}
            }
        }
    }
}

/** A machine's icon alone, in its colour: what an ended row says of its machine. */
@Composable
fun MachineIcon(member: Member?, offline: Boolean, modifier: Modifier = Modifier) {
    val palette = LocalPalette.current
    val ink = if (offline) palette.faint else shadeOf(member?.colour.orEmpty())
    Canvas(modifier.size(13.dp)) { drawIcon(member?.icon.orEmpty(), ink) }
}

/** The icons of the member list (`desktop`, `laptop`, `server`, `phone`, `tablet`), on a 24-unit grid. */
private fun DrawScope.drawIcon(icon: String, ink: Color) {
    val s = size.width / 24f
    val line = Stroke(1.8f * s, cap = StrokeCap.Round)
    fun rect(x: Float, y: Float, w: Float, h: Float, r: Float) =
        drawRoundRect(ink, Offset(x * s, y * s), Size(w * s, h * s), CornerRadius(r * s), style = line)
    fun seg(x1: Float, y1: Float, x2: Float, y2: Float) =
        drawLine(ink, Offset(x1 * s, y1 * s), Offset(x2 * s, y2 * s), 1.8f * s, StrokeCap.Round)
    when (icon) {
        "laptop" -> {
            rect(5f, 5f, 14f, 10f, 1.5f)
            seg(2f, 19f, 22f, 19f)
        }
        "server" -> {
            rect(4f, 4f, 16f, 7f, 2f)
            rect(4f, 13f, 16f, 7f, 2f)
            seg(8f, 7.5f, 8.01f, 7.5f)
            seg(8f, 16.5f, 8.01f, 16.5f)
        }
        "phone" -> {
            rect(7f, 2.5f, 10f, 19f, 2f)
            seg(11f, 18.5f, 13f, 18.5f)
        }
        "tablet" -> {
            rect(4f, 3f, 16f, 18f, 2f)
            seg(11f, 18f, 13f, 18f)
        }
        // A desktop, and a member added before icons.
        else -> {
            rect(3f, 4f, 18f, 12f, 2f)
            seg(8f, 20f, 16f, 20f)
            seg(12f, 16f, 12f, 20f)
        }
    }
}
