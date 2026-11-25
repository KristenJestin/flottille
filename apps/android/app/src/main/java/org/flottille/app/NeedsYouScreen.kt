package org.flottille.app

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.flottille.app.ui.Entrance
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Tag
import org.flottille.app.ui.Type
import org.flottille.core.Ask
import org.flottille.core.Phone
import org.flottille.core.PhoneException
import org.flottille.core.PhoneView
import org.flottille.core.Request

/** What waits for the user: a request or a question, of any machine. */
private sealed interface Waiting {
    val key: String
    val machine: String
    val agent: String
    val at: Long
    val number: Long

    data class OfRequest(val request: Request) : Waiting {
        override val key get() = "request:${request.machine}:${request.number}"
        override val machine get() = request.machine
        override val agent get() = request.agent
        override val at get() = request.at
        override val number get() = request.number
    }

    data class OfAsk(val ask: Ask) : Waiting {
        override val key get() = "ask:${ask.machine}:${ask.asked}"
        override val machine get() = ask.machine
        override val agent get() = ask.agent
        override val at get() = ask.at
        override val number get() = ask.asked
    }
}

/** How long a card says why an answer was refused before it folds away, as on the session page. */
private const val REFUSED_MILLIS = 2_500L

/**
 * Needs you (`docs/remote.md`, "The Android app"), where the app opens: each request and question
 * that waits for the user, from every machine, newest first, on the cards of the session page,
 * answered in place. A card leaves when the machine says it is answered, here or elsewhere; a tap
 * on it outside its buttons opens its session.
 */
@Composable
fun NeedsYouScreen(
    phone: Phone,
    view: PhoneView,
    onOpen: (machine: String, agent: String) -> Unit,
    onToast: (String) -> Unit,
) {
    val readOnly = view.role == "viewer"
    val scope = rememberCoroutineScope()
    // What the machine refused as gone stays on its card until it folds.
    val refused = remember { mutableStateMapOf<String, Pair<Waiting, String>>() }
    val folded = remember { mutableStateListOf<String>() }
    val answering = remember { mutableStateListOf<String>() }
    val titles = view.projects.flatMap { it.sessions }.associate { (it.machine to it.agent) to it.title }

    val live = view.requests.map { Waiting.OfRequest(it) } + view.asks.map { Waiting.OfAsk(it) }
    val waiting = (live + refused.values.map { it.first }.filter { r -> live.none { it.key == r.key } })
        .filter { it.key !in folded }
    // A card that no longer waits (answered here or elsewhere) stays while it folds away, then goes.
    val last = remember { mutableMapOf<String, Waiting>() }
    val leaving = remember { mutableStateMapOf<String, Waiting>() }
    val waitingKeys = waiting.map { it.key }.toSet()
    last.forEach { (key, item) -> if (key !in waitingKeys && key !in leaving) leaving[key] = item }
    waitingKeys.forEach { if (it in leaving) leaving.remove(it) }
    last.clear()
    waiting.forEach { last[it.key] = it }
    val shown = (waiting + leaving.values)
        .sortedWith(compareByDescending<Waiting> { it.at }.thenBy { it.machine }.thenByDescending { it.number })

    fun answer(item: Waiting, call: suspend () -> Unit) {
        answering += item.key
        scope.launch {
            try {
                call()
            } catch (refusal: PhoneException) {
                if (refusal.gone) {
                    // Decided already or gone: said on its card, which then folds.
                    refused[item.key] = item to refusal.why
                    delay(REFUSED_MILLIS)
                    folded += item.key
                    refused.remove(item.key)
                } else {
                    // A link that ended, a time out: it still waits, so does its card.
                    onToast(refusal.why)
                }
            } finally {
                answering -= item.key
            }
        }
    }

    Entrance(0, Modifier.fillMaxSize().testTag("needs-you")) {
        if (shown.isEmpty()) {
            Label("Nothing waits for you.", color = LocalPalette.current.mute, modifier = Modifier.padding(Space.Xl).testTag("needs-you-empty"))
        } else {
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(horizontal = Space.Lg, vertical = Space.Sm)) {
                items(shown, key = { it.key }) { item ->
                    // A card that comes unfolds and fades in, one that leaves fades and folds away,
                    // the others slide.
                    val presence = remember { MutableTransitionState(false) }
                    presence.targetState = item.key !in leaving
                    if (presence.isIdle && !presence.currentState && !presence.targetState) {
                        LaunchedEffect(item.key) { leaving.remove(item.key) }
                    }
                    AnimatedVisibility(
                        visibleState = presence,
                        modifier = Modifier.animateItem(fadeInSpec = null, placementSpec = Motion.spec(Motion.PANEL), fadeOutSpec = null),
                        enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.ENTER)),
                        exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
                    ) {
                        WaitingCard(
                            item = item,
                            title = titles[item.machine to item.agent] ?: item.agent,
                            onOpen = { onOpen(item.machine, item.agent) },
                        ) {
                            when (item) {
                                is Waiting.OfRequest -> RequestCard(
                                    request = item.request,
                                    readOnly = readOnly,
                                    answering = item.key in answering,
                                    refusal = refused[item.key]?.second,
                                    onAnswer = { allow -> answer(item) { phone.answer(item.machine, item.request.number, allow) } },
                                )
                                is Waiting.OfAsk -> AskCard(
                                    ask = item.ask,
                                    readOnly = readOnly,
                                    answering = item.key in answering,
                                    refusal = refused[item.key]?.second,
                                    onAnswer = { replies -> answer(item) { phone.answerAsk(item.machine, item.agent, item.ask.asked, replies) } },
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

/**
 * A card of Needs you: the session and the machine it waits on, over the session page's own card.
 * A tap outside its buttons opens the session; pressed, it sinks as a button does.
 */
@Composable
private fun WaitingCard(item: Waiting, title: String, onOpen: () -> Unit, card: @Composable () -> Unit) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val size by animateFloatAsState(if (pressed) PRESSED_CARD else 1f, Motion.spec(Motion.PRESS), label = "card size")
    Column(
        Modifier
            .padding(vertical = Space.Xs)
            .fillMaxWidth()
            .graphicsLayer {
                scaleX = size
                scaleY = size
            }
            .testTag("needs:${item.key}")
            .clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onOpen),
    ) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = Space.Xs),
            horizontalArrangement = Arrangement.spacedBy(Space.Sm),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Label(title, style = Type.Label, color = palette.mute, modifier = Modifier.weight(1f))
            Tag(item.machine)
        }
        card()
    }
}

/** A card is larger than a button: it sinks less, so that its text does not swim. */
private const val PRESSED_CARD = 0.98f
