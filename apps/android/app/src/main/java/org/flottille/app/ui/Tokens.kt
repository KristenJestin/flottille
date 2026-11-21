package org.flottille.app.ui

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The kit's motion (`gpui-design-system`, `src/motion.rs`), copied: the same durations, in
 * milliseconds, and the same ease-out cubic. Everything that moves in the app uses these, never a
 * number of its own.
 */
object Motion {
    /** A block, row, card or item appears. */
    const val ENTER = 240

    /** A level folds, a panel slides, a state swaps, a highlight moves. */
    const val PANEL = 280

    /** A whole surface changes. */
    const val COVER = 420

    /** A hover or a resting colour changes. */
    const val HOVER = 140

    /** A button pressed. */
    const val PRESS = 90

    /** A modal and its scrim come or go. */
    const val MODAL = 180

    /** The gap between siblings that arrive together, and the most that wait for it. */
    const val STAGGER = 70
    const val MAX_STAGGER = 8

    /** How far an entrance rises while it fades in, in dp. */
    const val RISE = 6

    /** The size of a button while it is pressed. */
    const val PRESSED = 0.96f

    /** `ease_out_cubic` of the kit: 1 - (1 - t)^3. */
    val EaseOut = CubicBezierEasing(0.33f, 1f, 0.68f, 1f)

    fun <T> spec(duration: Int, delay: Int = 0) = tween<T>(durationMillis = duration, delayMillis = delay, easing = EaseOut)
}

/** The kit's own palette, light and dark (`src/theme.rs`): the roles the app draws with. */
data class Palette(
    val bg: Color,
    val side: Color,
    val card: Color,
    val card2: Color,
    val line: Color,
    val line2: Color,
    val hover: Color,
    val sel: Color,
    val text: Color,
    val mute: Color,
    val faint: Color,
    val acc: Color,
    val onAcc: Color,
    val field: Color,
    val run: Color,
    val runBg: Color,
    /** Waits for the user, and nothing else. */
    val wait: Color,
    val waitBg: Color,
    val ok: Color,
    val okBg: Color,
    val fail: Color,
    val failBg: Color,
    val onFail: Color,
    val failLine: Color,
    val scrim: Color,
) {
    companion object {
        val Dark = Palette(
            bg = Color(0xFF141414), side = Color(0xFF121212), card = Color(0xFF1B1B1B), card2 = Color(0xFF222222),
            line = Color(0xFF262626), line2 = Color(0xFF303030), hover = Color(0xFF212121), sel = Color(0xFF262626),
            text = Color(0xFFECECEC), mute = Color(0xFFA3A3A3), faint = Color(0xFF8A8A8A),
            acc = Color(0xFFECECEC), onAcc = Color(0xFF111111), field = Color(0xFF121212),
            run = Color(0xFF7AA7FF), runBg = Color(0xFF18233A), wait = Color(0xFFF5A524), waitBg = Color(0xFF2E2210),
            ok = Color(0xFF4ADE80), okBg = Color(0xFF15291C), fail = Color(0xFFF87171), failBg = Color(0xFF3A1C1C),
            onFail = Color(0xFF1A0606), failLine = Color(0xFF5A2A2A), scrim = Color(0x80000000),
        )
        val Light = Palette(
            bg = Color(0xFFFBFBFA), side = Color(0xFFF5F5F4), card = Color(0xFFF4F4F3), card2 = Color(0xFFEBEBEA),
            line = Color(0xFFE2E2E0), line2 = Color(0xFFD4D4D2), hover = Color(0xFFEDEDEC), sel = Color(0xFFE5E5E3),
            text = Color(0xFF171717), mute = Color(0xFF4F4F4E), faint = Color(0xFF636362),
            acc = Color(0xFF1C1C1C), onAcc = Color(0xFFFAFAFA), field = Color(0xFFFDFDFD),
            run = Color(0xFF2251C4), runBg = Color(0xFFE7EDFB), wait = Color(0xFF8F5200), waitBg = Color(0xFFFAEFDC),
            ok = Color(0xFF17703A), okBg = Color(0xFFE2F2E7), fail = Color(0xFFB3261E), failBg = Color(0xFFFBE7E5),
            onFail = Color(0xFFFFFFFF), failLine = Color(0xFFEAB8B3), scrim = Color(0x4D000000),
        )
    }
}

val LocalPalette = staticCompositionLocalOf { Palette.Dark }

/** Shapes and spacing: the kit's scale. */
object Space {
    val Xs = 4.dp
    val Sm = 8.dp
    val Md = 12.dp
    val Lg = 16.dp
    val Xl = 24.dp
}

object Type {
    val Title = TextStyle(fontSize = 22.sp, fontWeight = FontWeight.SemiBold)
    val Heading = TextStyle(fontSize = 13.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.4.sp)
    val Body = TextStyle(fontSize = 15.sp)
    val Label = TextStyle(fontSize = 13.sp)
    val Small = TextStyle(fontSize = 12.sp)
}

/** The palette that follows the system's light or dark. */
@Composable
fun FlottilleTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalPalette provides if (dark) Palette.Dark else Palette.Light, content = content)
}
