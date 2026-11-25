package org.flottille.app.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsPropertyKey
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** A text, in the palette's ink. */
@Composable
fun Label(text: String, modifier: Modifier = Modifier, style: TextStyle = Type.Body, color: Color = LocalPalette.current.text) {
    BasicText(text, modifier, style = style.copy(color = color))
}

/** The four kinds of button (`docs/reference/gpui.md`, "Rules of the window"): one of each, never another. */
enum class ButtonKind { Primary, Secondary, Ghost, Danger }

/**
 * A button: its fill and ink fade over [Motion.HOVER] as it is pressed or disabled, and it sinks
 * to [Motion.PRESSED] over [Motion.PRESS]. Its label is always visible.
 */
@Composable
fun FButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: ButtonKind = ButtonKind.Secondary,
    enabled: Boolean = true,
) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val (fill, pressedFill, ink) = when (kind) {
        ButtonKind.Primary -> Triple(palette.acc, palette.acc.copy(alpha = 0.82f), palette.onAcc)
        ButtonKind.Secondary -> Triple(palette.card2, palette.sel, palette.text)
        ButtonKind.Ghost -> Triple(palette.bg.copy(alpha = 0f), palette.hover, palette.mute)
        ButtonKind.Danger -> Triple(palette.fail, palette.fail.copy(alpha = 0.82f), palette.onFail)
    }
    val background by animateColorAsState(if (pressed) pressedFill else fill, Motion.spec(Motion.HOVER), label = "button fill")
    val enabledAlpha by animateFloatAsState(if (enabled) 1f else 0.45f, Motion.spec(Motion.HOVER), label = "button enabled")
    val size by animateFloatAsState(if (pressed && enabled) Motion.PRESSED else 1f, Motion.spec(Motion.PRESS), label = "button size")
    Box(
        modifier
            .graphicsLayer {
                scaleX = size
                scaleY = size
            }
            .alpha(enabledAlpha)
            .background(background, RoundedCornerShape(10.dp))
            .clickable(interactionSource = source, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = Space.Lg, vertical = Space.Md),
        contentAlignment = Alignment.Center,
    ) {
        Label(text, style = Type.Body.copy(fontWeight = FontWeight.Medium), color = ink)
    }
}

/** A choice among a few (one is chosen): its fill and ring move over [Motion.PANEL] when the choice changes. */
@Composable
fun Choice(text: String, chosen: Boolean, onChoose: () -> Unit, modifier: Modifier = Modifier) {
    val palette = LocalPalette.current
    val fill by animateColorAsState(if (chosen) palette.sel else palette.card, Motion.spec(Motion.PANEL), label = "choice fill")
    val ring by animateColorAsState(if (chosen) palette.acc else palette.line, Motion.spec(Motion.PANEL), label = "choice ring")
    Box(
        modifier
            .background(fill, RoundedCornerShape(10.dp))
            .border(BorderStroke(1.dp, ring), RoundedCornerShape(10.dp))
            .semantics { selected = chosen }
            .clickable(role = Role.RadioButton, indication = null, interactionSource = remember { MutableInteractionSource() }, onClick = onChoose)
            .padding(horizontal = Space.Lg, vertical = Space.Md),
        contentAlignment = Alignment.Center,
    ) {
        Label(text, color = if (chosen) palette.text else palette.mute)
    }
}

/** A field with its label above, always visible. [tag] names it for the tests. */
@Composable
fun Field(
    label: String,
    value: String,
    onChange: (String) -> Unit,
    tag: String,
    modifier: Modifier = Modifier,
    hidden: Boolean = false,
    singleLine: Boolean = true,
    maxLines: Int = if (singleLine) 1 else Int.MAX_VALUE,
) {
    val palette = LocalPalette.current
    var focused by remember { mutableStateOf(false) }
    val ring by animateColorAsState(if (focused) palette.acc else palette.line2, Motion.spec(Motion.HOVER), label = "field ring")
    Column(modifier, verticalArrangement = Arrangement.spacedBy(Space.Xs)) {
        Label(label, style = Type.Label, color = palette.mute)
        BasicTextField(
            value = value,
            onValueChange = onChange,
            singleLine = singleLine,
            maxLines = maxLines,
            textStyle = Type.Body.copy(color = palette.text),
            cursorBrush = SolidColor(palette.acc),
            visualTransformation = if (hidden) PasswordVisualTransformation() else VisualTransformation.None,
            keyboardOptions = KeyboardOptions(keyboardType = if (hidden) KeyboardType.Password else KeyboardType.Text),
            modifier = Modifier
                .testTag(tag)
                .fillMaxWidth()
                .background(palette.field, RoundedCornerShape(10.dp))
                .border(BorderStroke(1.dp, ring), RoundedCornerShape(10.dp))
                .padding(horizontal = Space.Md, vertical = Space.Md)
                .onFocusChanged { focused = it.isFocused },
        )
    }
}

/** The fill a control draws now, for the tests: the colour in the middle of its move is no pixel to read. */
val FillColour = SemanticsPropertyKey<Color>("FillColour")

/**
 * A block that unfolds and fades in once when [animated] (a message that arrives), and is simply
 * there otherwise (what the page held when it opened).
 */
@Composable
fun Unfold(animated: Boolean, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    if (!animated) {
        Box(modifier) { content() }
        return
    }
    val state = remember { MutableTransitionState(false).apply { targetState = true } }
    AnimatedVisibility(
        visibleState = state,
        modifier = modifier,
        enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.ENTER)),
    ) { content() }
}

/** A small tag, such as the machine a session runs on. */
@Composable
fun Tag(text: String, modifier: Modifier = Modifier, vertical: Dp = 2.dp) {
    val palette = LocalPalette.current
    Box(modifier.background(palette.card2, RoundedCornerShape(6.dp)).padding(horizontal = Space.Sm, vertical = vertical)) {
        Label(text, style = Type.Small, color = palette.mute)
    }
}

/** A message that unfolds and fades in, and folds away and fades out, with the gap around it. */
@Composable
fun Message(text: String?, modifier: Modifier = Modifier, tag: String = "message", failed: Boolean = true) {
    val palette = LocalPalette.current
    AnimatedVisibility(
        visible = text != null,
        modifier = modifier,
        enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.ENTER)),
        exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
    ) {
        // The last text stays while it folds away.
        var shown by remember { mutableStateOf(text.orEmpty()) }
        if (text != null) shown = text
        Box(
            Modifier
                .padding(vertical = Space.Xs)
                .fillMaxWidth()
                .background(if (failed) palette.failBg else palette.card, RoundedCornerShape(10.dp))
                .border(BorderStroke(1.dp, if (failed) palette.failLine else palette.line), RoundedCornerShape(10.dp))
                .padding(Space.Md)
                .testTag(tag),
        ) {
            Label(shown, style = Type.Label, color = if (failed) palette.fail else palette.text)
        }
    }
}

/** A block that comes in once, rising and fading over [Motion.ENTER], after [index] steps of [Motion.STAGGER]. */
@Composable
fun Entrance(index: Int = 0, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val state = remember { MutableTransitionState(false).apply { targetState = true } }
    val delay = minOf(index, Motion.MAX_STAGGER) * Motion.STAGGER
    val rise = with(LocalDensity.current) { Motion.RISE.dp.roundToPx() }
    AnimatedVisibility(
        visibleState = state,
        modifier = modifier,
        enter = fadeIn(Motion.spec(Motion.ENTER, delay)) + slideInVertically(Motion.spec(Motion.ENTER, delay)) { rise },
    ) {
        content()
    }
}

/**
 * The one confirmation dialog: a scrim and a card that fade and scale in over [Motion.MODAL]. It
 * asks once; [onConfirm] is the danger button, [onCancel] the ghost one (and the scrim's tap).
 */
@Composable
fun Confirm(
    visible: Boolean,
    title: String,
    body: String,
    confirm: String,
    onConfirm: () -> Unit,
    onCancel: () -> Unit,
) {
    val palette = LocalPalette.current
    AnimatedVisibility(visible, enter = fadeIn(Motion.spec(Motion.MODAL)), exit = fadeOut(Motion.spec(Motion.MODAL))) {
        Box(
            Modifier
                .fillMaxSize()
                .background(palette.scrim)
                .clickable(indication = null, interactionSource = remember { MutableInteractionSource() }, onClick = onCancel),
            contentAlignment = Alignment.Center,
        ) {
            Column(
                Modifier
                    .animateEnterExit(
                        enter = scaleIn(Motion.spec(Motion.MODAL), initialScale = 0.96f),
                        exit = scaleOut(Motion.spec(Motion.MODAL), targetScale = 0.96f),
                    )
                    .padding(Space.Xl)
                    .background(palette.card, RoundedCornerShape(16.dp))
                    .border(BorderStroke(1.dp, palette.line2), RoundedCornerShape(16.dp))
                    .clickable(indication = null, interactionSource = remember { MutableInteractionSource() }) {}
                    .padding(Space.Xl),
                verticalArrangement = Arrangement.spacedBy(Space.Md),
            ) {
                Label(title, style = Type.Title)
                Label(body, color = palette.mute)
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.Sm, Alignment.End)) {
                    FButton("Cancel", onCancel, kind = ButtonKind.Ghost)
                    FButton(confirm, onConfirm, kind = ButtonKind.Danger)
                }
            }
        }
    }
}


/**
 * A button that is an icon, round: [label] is what a screen reader says of it. Its fill fades over
 * [Motion.HOVER] as it is pressed or disabled, and it sinks to [Motion.PRESSED] over [Motion.PRESS].
 * A [ButtonKind.Primary] one is filled (Send), a [ButtonKind.Secondary] one sits on a card (Stop).
 */
@Composable
fun IconButton(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: ButtonKind = ButtonKind.Ghost,
    enabled: Boolean = true,
    size: Dp = 44.dp,
) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val (fill, pressedFill, ink) = when (kind) {
        ButtonKind.Primary -> Triple(palette.acc, palette.acc.copy(alpha = 0.82f), palette.onAcc)
        ButtonKind.Secondary -> Triple(palette.card2, palette.sel, palette.text)
        ButtonKind.Ghost -> Triple(palette.bg.copy(alpha = 0f), palette.sel, palette.text)
        ButtonKind.Danger -> Triple(palette.fail, palette.fail.copy(alpha = 0.82f), palette.onFail)
    }
    val background by animateColorAsState(if (pressed) pressedFill else fill, Motion.spec(Motion.HOVER), label = "icon button fill")
    val enabledAlpha by animateFloatAsState(if (enabled) 1f else 0.4f, Motion.spec(Motion.HOVER), label = "icon button enabled")
    val scale by animateFloatAsState(if (pressed && enabled) Motion.PRESSED else 1f, Motion.spec(Motion.PRESS), label = "icon button size")
    Box(
        modifier
            .size(size)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .alpha(enabledAlpha)
            .background(background, CircleShape)
            .semantics { contentDescription = label }
            .clickable(interactionSource = source, indication = null, enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Glyph(icon, ink, size = 20.dp)
    }
}

/**
 * A panel that rises from the bottom over a scrim, over [Motion.PANEL], and goes down again: the
 * tree of a session, the choice of its mode. [onClose] is its Close button and the scrim's tap;
 * [aside] a word said beside the title (`Read only`).
 * [tag] names the panel for the tests.
 */
@Composable
fun Sheet(
    visible: Boolean,
    title: String,
    tag: String,
    onClose: () -> Unit,
    aside: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val palette = LocalPalette.current
    AnimatedVisibility(visible, enter = fadeIn(Motion.spec(Motion.PANEL)), exit = fadeOut(Motion.spec(Motion.PANEL))) {
        Box(
            Modifier
                .fillMaxSize()
                .background(palette.scrim)
                // A tap with no semantics: a clickable scrim would merge the panel into itself, and
                // Close is the panel's way out for a screen reader.
                .pointerInput(onClose) { detectTapGestures { onClose() } },
            contentAlignment = Alignment.BottomCenter,
        ) {
            Column(
                Modifier
                    .animateEnterExit(
                        enter = slideInVertically(Motion.spec(Motion.PANEL)) { it },
                        exit = slideOutVertically(Motion.spec(Motion.PANEL)) { it },
                    )
                    .fillMaxWidth()
                    .heightIn(max = 600.dp)
                    .background(palette.bg, RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp))
                    // A tap on the panel is not one on the scrim; with no semantics, its rows stay their own.
                    .pointerInput(Unit) { detectTapGestures { } }
                    .testTag(tag)
                    .navigationBarsPadding()
                    .padding(bottom = Space.Lg),
            ) {
                Box(Modifier.fillMaxWidth().padding(top = Space.Sm), contentAlignment = Alignment.Center) {
                    Box(Modifier.size(width = 32.dp, height = 4.dp).background(palette.line2, RoundedCornerShape(2.dp)))
                }
                Row(
                    Modifier.fillMaxWidth().padding(start = Space.Xl - Space.Xs, end = Space.Sm),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Label(title, style = Type.Body.copy(fontWeight = FontWeight.SemiBold), modifier = Modifier.weight(1f))
                    if (aside != null) Label(aside, style = Type.Small, color = palette.faint)
                    IconButton(Icons.Close, "Close", onClose)
                }
                content()
            }
        }
    }
}
