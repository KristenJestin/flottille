package org.flottille.app

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.FButton
import org.flottille.app.ui.Field
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Message
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Tag
import org.flottille.app.ui.Type
import org.flottille.core.Ask
import org.flottille.core.AskQuestion
import org.flottille.core.Reply
import org.flottille.core.Request

/**
 * A request that waits for the user: the call as the daemon gave it (the same words the window
 * shows, never summarised here), the reason, and Allow and Deny, which a viewer does not get. A
 * base request stands apart by its tag and its warning: it asks every time, in every mode.
 * [refusal] is what the machine said when it refused the answer (already answered, gone): the card
 * says it, then folds.
 */
@Composable
fun RequestCard(
    request: Request,
    readOnly: Boolean,
    answering: Boolean,
    refusal: String?,
    onAnswer: (allow: Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    val palette = LocalPalette.current
    val ring by animateColorAsState(if (request.base) palette.wait else palette.line, Motion.spec(Motion.PANEL), label = "request ring")
    Column(
        modifier
            .padding(vertical = Space.Xs)
            .fillMaxWidth()
            .background(palette.card, RoundedCornerShape(12.dp))
            .border(BorderStroke(1.dp, ring), RoundedCornerShape(12.dp))
            .testTag("request:${request.number}")
            .padding(Space.Md),
        verticalArrangement = Arrangement.spacedBy(Space.Sm),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(Space.Sm), verticalAlignment = Alignment.CenterVertically) {
            Label("Request #${request.number}", style = Type.Heading, color = palette.wait, modifier = Modifier.weight(1f))
            if (request.base) Tag("Always asks")
        }
        Label(request.call)
        request.says.forEach { Label(it, style = Type.Label, color = palette.mute) }
        if (request.reason.isNotEmpty()) Label(request.reason, style = Type.Small, color = palette.mute)
        if (request.base) {
            Label(
                "It comes back every time, in every mode. Allow it only if you know what it does.",
                style = Type.Label.copy(fontWeight = FontWeight.SemiBold),
                color = palette.wait,
            )
        }
        Message(refusal, tag = "request-refusal:${request.number}")
        if (!readOnly && refusal == null) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.Sm, Alignment.End)) {
                FButton("Deny", { onAnswer(false) }, kind = ButtonKind.Secondary, enabled = !answering, modifier = Modifier.testTag("deny:${request.number}"))
                FButton("Allow", { onAnswer(true) }, kind = ButtonKind.Primary, enabled = !answering, modifier = Modifier.testTag("allow:${request.number}"))
            }
        }
    }
}

/** What the user picked or wrote for one question. */
private data class Pick(
    val choices: Set<String> = emptySet(),
    val other: Boolean = false,
    val words: String = "",
    val note: String = "",
    val noteOpen: Boolean = false,
)

private fun Pick.answered(question: AskQuestion): Boolean =
    if (question.options.isEmpty() || other) words.isNotBlank() else choices.isNotEmpty()

private fun Pick.reply(question: AskQuestion): Reply =
    Reply(
        choices = question.options.map { it.label }.filter { it in choices },
        text = if (question.options.isEmpty() || other) words.trim() else note.trim(),
    )

/**
 * The ask card (`docs/tools.md`, "Answering an ask"): one question at a time, its options as rows
 * (a round mark for one choice, a square one for several), "Something else…" last, one Answer for
 * the card once every question has a choice or words. A viewer reads it and answers nothing.
 */
@Composable
fun AskCard(
    ask: Ask,
    readOnly: Boolean,
    answering: Boolean,
    refusal: String?,
    onAnswer: (List<Reply>) -> Unit,
    modifier: Modifier = Modifier,
) {
    val palette = LocalPalette.current
    val picks = remember(ask.asked) { mutableStateListOf<Pick>().apply { repeat(ask.questions.size) { add(Pick()) } } }
    var index by rememberSaveable(ask.asked) { mutableIntStateOf(0) }
    val answered = ask.questions.indices.count { picks.getOrNull(it)?.answered(ask.questions[it]) == true }
    val complete = answered == ask.questions.size
    // The next question still unanswered after [from], else the one shown stays.
    fun next(from: Int): Int =
        (from + 1 until ask.questions.size).firstOrNull { !picks[it].answered(ask.questions[it]) } ?: from

    Column(
        modifier
            .padding(vertical = Space.Xs)
            .fillMaxWidth()
            .background(palette.card, RoundedCornerShape(12.dp))
            .border(BorderStroke(1.dp, palette.wait), RoundedCornerShape(12.dp))
            .testTag("ask:${ask.asked}")
            .padding(Space.Md),
        verticalArrangement = Arrangement.spacedBy(Space.Sm),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(Space.Sm), verticalAlignment = Alignment.CenterVertically) {
            Label("Asks you", style = Type.Heading, color = palette.wait, modifier = Modifier.weight(1f))
            Tag("$answered of ${ask.questions.size} answered")
        }
        if (ask.questions.size > 1) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                FButton("Previous", { index -= 1 }, kind = ButtonKind.Ghost, enabled = index > 0, modifier = Modifier.testTag("ask-previous"))
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Space.Xs)) {
                    Label("${index + 1} of ${ask.questions.size}", style = Type.Label, color = palette.mute)
                    Row(horizontalArrangement = Arrangement.spacedBy(Space.Xs)) {
                        ask.questions.indices.forEach { Dot(filled = picks[it].answered(ask.questions[it]), shown = it == index) }
                    }
                }
                FButton("Next", { index += 1 }, kind = ButtonKind.Ghost, enabled = index < ask.questions.size - 1, modifier = Modifier.testTag("ask-next"))
            }
        }
        // The question slides in from the side of the move.
        AnimatedContent(
            targetState = index,
            transitionSpec = {
                val forward = targetState > initialState
                (fadeIn(Motion.spec(Motion.PANEL)) + slideInHorizontally(Motion.spec(Motion.PANEL)) { if (forward) it / 4 else -it / 4 }) togetherWith
                    (fadeOut(Motion.spec(Motion.HOVER)) + slideOutHorizontally(Motion.spec(Motion.PANEL)) { if (forward) -it / 4 else it / 4 })
            },
            label = "ask question",
        ) { shown ->
            val question = ask.questions[shown]
            val pick = picks[shown]
            Column(verticalArrangement = Arrangement.spacedBy(Space.Sm)) {
                Label(question.question)
                if (question.multiple) Label("several", style = Type.Small, color = palette.mute)
                question.options.forEach { option ->
                    OptionRow(
                        label = option.label,
                        description = option.description,
                        chosen = option.label in pick.choices,
                        several = question.multiple,
                        tag = if (readOnly) null else "option:$shown:${option.label}",
                        onClick = {
                            val chosen = if (question.multiple) {
                                if (option.label in pick.choices) pick.choices - option.label else pick.choices + option.label
                            } else {
                                setOf(option.label)
                            }
                            picks[shown] = pick.copy(choices = chosen, other = pick.other && question.multiple)
                            if (!question.multiple) index = next(shown)
                        },
                    )
                }
                if (question.options.isNotEmpty() && !readOnly) {
                    OptionRow(
                        label = "Something else…",
                        description = "",
                        chosen = pick.other,
                        several = question.multiple,
                        tag = "other:$shown",
                        onClick = {
                            picks[shown] = pick.copy(other = !pick.other, choices = if (question.multiple) pick.choices else emptySet())
                        },
                    )
                }
                // The field unfolds in the row when "Something else…" is picked, and folds away
                // when it is let go: the words are kept.
                AnimatedVisibility(
                    visible = !readOnly && (question.options.isEmpty() || pick.other),
                    enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.ENTER)),
                    exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
                ) {
                    Field("Your answer", pick.words, { picks[shown] = pick.copy(words = it) }, tag = "ask-words", singleLine = false, maxLines = 8)
                }
                AnimatedVisibility(
                    visible = !readOnly && question.options.isNotEmpty() && !pick.other && pick.choices.isNotEmpty(),
                    enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.ENTER)),
                    exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
                ) {
                    if (pick.noteOpen) {
                        Field("Note with your choice", pick.note, { picks[shown] = pick.copy(note = it) }, tag = "ask-note", singleLine = false, maxLines = 8)
                    } else {
                        FButton("+ Add a note", { picks[shown] = pick.copy(noteOpen = true) }, kind = ButtonKind.Ghost, modifier = Modifier.testTag("ask-note-add"))
                    }
                }
            }
        }
        Message(refusal, tag = "ask-refusal:${ask.asked}")
        if (!readOnly && refusal == null) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                FButton(
                    "Answer",
                    { onAnswer(ask.questions.indices.map { picks[it].reply(ask.questions[it]) }) },
                    kind = ButtonKind.Primary,
                    enabled = complete && !answering,
                    modifier = Modifier.testTag("ask-answer"),
                )
            }
        }
    }
}

/** A dot per question: filled once it has a choice or words, the one shown longer. */
@Composable
private fun Dot(filled: Boolean, shown: Boolean) {
    val palette = LocalPalette.current
    val fill by animateColorAsState(if (filled) palette.acc else palette.line2, Motion.spec(Motion.PANEL), label = "dot fill")
    val width by animateFloatAsState(if (shown) 14f else 6f, Motion.spec(Motion.PANEL), label = "dot width")
    Box(Modifier.size(width.dp, 6.dp).background(fill, CircleShape))
}

/** An option: its fill and ring move over [Motion.PANEL] as it is chosen; a mark round or square. */
@Composable
private fun OptionRow(label: String, description: String, chosen: Boolean, several: Boolean, tag: String?, onClick: () -> Unit) {
    val palette = LocalPalette.current
    val fill by animateColorAsState(if (chosen) palette.sel else palette.card, Motion.spec(Motion.PANEL), label = "option fill")
    val ring by animateColorAsState(if (chosen) palette.acc else palette.line, Motion.spec(Motion.PANEL), label = "option ring")
    val mark by animateColorAsState(if (chosen) palette.acc else palette.bg.copy(alpha = 0f), Motion.spec(Motion.PANEL), label = "option mark")
    val shape = if (several) RoundedCornerShape(3.dp) else CircleShape
    val row = Modifier
        .fillMaxWidth()
        .background(fill, RoundedCornerShape(10.dp))
        .border(BorderStroke(1.dp, ring), RoundedCornerShape(10.dp))
        .then(if (tag != null) Modifier.testTag(tag) else Modifier)
        .semantics { selected = chosen }
        .then(
            if (tag != null) {
                Modifier.clickable(role = Role.RadioButton, indication = null, interactionSource = remember { MutableInteractionSource() }, onClick = onClick)
            } else {
                Modifier
            },
        )
        .padding(Space.Md)
    Row(row, horizontalArrangement = Arrangement.spacedBy(Space.Md), verticalAlignment = Alignment.Top) {
        Box(Modifier.padding(top = 3.dp).size(14.dp).border(BorderStroke(1.5.dp, ring), shape).padding(3.dp).background(mark, shape))
        Column(Modifier.weight(1f).alpha(if (tag == null) 0.8f else 1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Label(label, color = if (chosen) palette.text else palette.mute)
            if (description.isNotEmpty()) Label(description, style = Type.Small, color = palette.faint)
        }
    }
}
