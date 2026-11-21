package org.flottille.app

import android.graphics.BitmapFactory
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.Crossfade
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import java.time.ZoneId
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.FButton
import org.flottille.app.ui.Field
import org.flottille.app.ui.FillColour
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Tag
import org.flottille.app.ui.Type
import org.flottille.app.ui.Unfold
import org.flottille.core.Ask
import org.flottille.core.Child
import org.flottille.core.Conversation
import org.flottille.core.Entry
import org.flottille.core.MachineState
import org.flottille.core.Phone
import org.flottille.core.PhoneException
import org.flottille.core.PhoneView
import org.flottille.core.Picture
import org.flottille.core.Request

/** A session of a machine: the one entry point of its page, for the list and for a notification. */
data class SessionKey(val machine: String, val agent: String) {
    companion object {
        val Saver = androidx.compose.runtime.saveable.listSaver<SessionKey?, String>(
            save = { if (it == null) emptyList() else listOf(it.machine, it.agent) },
            restore = { if (it.size == 2) SessionKey(it[0], it[1]) else null },
        )
    }
}

/** How long a card says why an answer was refused before it folds away. */
private const val REFUSED_MILLIS = 2_500L

/** What the page is made of, in the order it is drawn: a message, or the steps between two. */
private sealed interface Block {
    val key: String

    data class Message(val entry: Entry) : Block {
        override val key get() = "m:${entry.seq}"
    }

    data class Steps(val steps: List<Entry>) : Block {
        override val key get() = "s:${steps.first().seq}"
    }
}

/** The conversation's entries as lines: the thoughts and the tool calls between two messages fold into one. */
private fun toLines(entries: List<Entry>): List<Block> {
    val out = mutableListOf<Block>()
    val steps = mutableListOf<Entry>()
    fun flush() {
        if (steps.isNotEmpty()) out += Block.Steps(steps.toList())
        steps.clear()
    }
    for (entry in entries) {
        if (entry.kind == "thought" || entry.kind == "tool") {
            steps += entry
        } else {
            flush()
            out += Block.Message(entry)
        }
    }
    flush()
    return out
}

/**
 * The session page (`docs/design.md` §10, for a phone): the conversation live, its steps folded,
 * its pictures shown; the composer with pictures from the gallery, the camera or the share sheet;
 * the ask card and the requests, answered in place; the mode, Stop and the tree read only. The
 * page follows its session only while it is on screen. What the device may do is the machine's to
 * say; hiding what a `viewer` cannot do is this page's half.
 */
@Composable
fun SessionPage(
    phone: Phone,
    view: PhoneView,
    key: SessionKey,
    zone: ZoneId,
    pictures: PictureSource,
    carried: List<Picture>,
    onCarried: () -> Unit,
    onToast: (String) -> Unit,
    onBack: () -> Unit,
) {
    val palette = LocalPalette.current
    val scope = rememberCoroutineScope()
    val machine = view.machines.firstOrNull { it.name == key.machine }
    val online = machine?.state == MachineState.Online
    val page = view.conversation?.takeIf { it.machine == key.machine && it.agent == key.agent }
    val readOnly = view.role == "viewer"

    // Followed while it is on screen, and while the machine is linked; left, nothing is kept.
    LaunchedEffect(key, online) {
        if (online) {
            try {
                phone.follow(key.machine, key.agent)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (refused: PhoneException) {
                onToast(refused.why)
            }
        }
    }
    DisposableEffect(key) { onDispose { phone.unfollow() } }

    val attached = remember(key) { mutableStateListOf<Picture>().apply { addAll(carried) } }
    LaunchedEffect(key) { if (carried.isNotEmpty()) onCarried() }

    // What the machine refused of an answer stays on its card until it folds.
    val refusedRequests = remember(key) { mutableStateMapOf<Long, Pair<Request, String>>() }
    val refusedAsks = remember(key) { mutableStateMapOf<Long, Pair<Ask, String>>() }
    val folded = remember(key) { mutableStateListOf<String>() }
    val answering = remember(key) { mutableStateListOf<String>() }

    val treeIds = page?.children?.map { it.agent }.orEmpty() + key.agent
    val requests = (view.requests.filter { it.machine == key.machine && it.agent in treeIds } +
        refusedRequests.values.map { it.first }.filter { r -> view.requests.none { it.number == r.number && it.machine == r.machine } })
        .filter { "request:${it.number}" !in folded }
        .sortedBy { it.number }
    val asks = (view.asks.filter { it.machine == key.machine && it.agent in treeIds } +
        refusedAsks.values.map { it.first }.filter { a -> view.asks.none { it.asked == a.asked && it.machine == a.machine } })
        .filter { "ask:${it.asked}" !in folded }
        .sortedBy { it.asked }

    Column(Modifier.fillMaxSize().background(palette.bg).testTag("page")) {
        Header(page, key, readOnly, onBack)
        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (page == null) {
                Label(
                    if (online) "Loading the conversation…" else "This machine is not reachable.",
                    color = palette.mute,
                    modifier = Modifier.padding(Space.Lg),
                )
            } else {
                ConversationList(phone, key, page, onOlder = {
                    scope.launch {
                        try {
                            phone.older(key.machine, key.agent)
                        } catch (refused: PhoneException) {
                            onToast(refused.why)
                        }
                    }
                })
            }
        }
        if (page != null && page.children.isNotEmpty()) Tree(page.children)

        // The asks and the requests wait docked above the composer, never scrolled away with the
        // conversation: each unfolds in, and folds away once answered.
        LazyColumn(Modifier.fillMaxWidth().heightIn(max = 360.dp).padding(horizontal = Space.Lg)) {
            items(asks, key = { "ask:${it.asked}" }) { ask ->
                AskCard(
                    ask = ask,
                    readOnly = readOnly,
                    answering = "ask:${ask.asked}" in answering,
                    refusal = refusedAsks[ask.asked]?.second,
                    onAnswer = { replies ->
                        answering += "ask:${ask.asked}"
                        scope.launch {
                            try {
                                phone.answerAsk(ask.machine, ask.agent, ask.asked, replies)
                            } catch (refused: PhoneException) {
                                refusedAsks[ask.asked] = ask to refused.why
                                delay(REFUSED_MILLIS)
                                folded += "ask:${ask.asked}"
                                refusedAsks.remove(ask.asked)
                            } finally {
                                answering -= "ask:${ask.asked}"
                            }
                        }
                    },
                    modifier = Modifier.animateItem(
                        fadeInSpec = Motion.spec(Motion.ENTER),
                        placementSpec = Motion.spec(Motion.PANEL),
                        fadeOutSpec = Motion.spec(Motion.HOVER),
                    ),
                )
            }
            items(requests, key = { "request:${it.number}" }) { request ->
                RequestCard(
                    request = request,
                    readOnly = readOnly,
                    answering = "request:${request.number}" in answering,
                    refusal = refusedRequests[request.number]?.second,
                    onAnswer = { allow ->
                        answering += "request:${request.number}"
                        scope.launch {
                            try {
                                phone.answer(request.machine, request.number, allow)
                            } catch (refused: PhoneException) {
                                refusedRequests[request.number] = request to refused.why
                                delay(REFUSED_MILLIS)
                                folded += "request:${request.number}"
                                refusedRequests.remove(request.number)
                            } finally {
                                answering -= "request:${request.number}"
                            }
                        }
                    },
                    modifier = Modifier.animateItem(
                        fadeInSpec = Motion.spec(Motion.ENTER),
                        placementSpec = Motion.spec(Motion.PANEL),
                        fadeOutSpec = Motion.spec(Motion.HOVER),
                    ),
                )
            }
        }

        when {
            page == null || readOnly || page.child -> Unit
            page.ended -> Label("This session has ended.", color = palette.mute, modifier = Modifier.padding(Space.Lg))
            else -> Composer(
                phone = phone,
                page = page,
                key = key,
                attached = attached,
                pictures = pictures,
                offline = if (online) null else offlineSentence(key.machine, machine, zone),
                onToast = onToast,
            )
        }
    }
}

/** Why nothing can be sent to a machine that is not linked, and since when. */
private fun offlineSentence(name: String, machine: org.flottille.core.Machine?, zone: ZoneId): String =
    when (machine?.state) {
        MachineState.NeedsUpdate -> "$name needs an update"
        MachineState.Offline -> "$name is offline since ${timeOfDay(machine.since, zone)}"
        else -> "$name is not reachable"
    }

@Composable
private fun Header(page: Conversation?, key: SessionKey, readOnly: Boolean, onBack: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = Space.Lg, vertical = Space.Sm),
        horizontalArrangement = Arrangement.spacedBy(Space.Sm),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FButton("Back", onBack, kind = ButtonKind.Ghost, modifier = Modifier.testTag("page-back"))
        Label(page?.title.orEmpty().ifEmpty { "Session" }, style = Type.Heading, modifier = Modifier.weight(1f))
        Tag(key.machine)
        if (readOnly) Tag("View only")
    }
}

@Composable
private fun ConversationList(phone: Phone, key: SessionKey, page: Conversation, onOlder: () -> Unit) {
    val state = rememberLazyListState()
    val lines = toLines(page.entries)
    // What was there when the page opened is simply there; what arrives after unfolds and fades in.
    val initial = remember(key) { lines.map { it.key }.toSet() }
    val appeared = remember(key) { mutableStateListOf<String>() }
    val unfolded = remember(key) { mutableStateListOf<String>() }
    // Stays at the end while the conversation grows, unless the user scrolled up.
    val last = lines.lastOrNull()?.key
    LaunchedEffect(last) {
        val atEnd = state.layoutInfo.visibleItemsInfo.lastOrNull()?.index?.let { it >= state.layoutInfo.totalItemsCount - 2 } ?: true
        if (last != null && atEnd) state.animateScrollToItem((state.layoutInfo.totalItemsCount - 1).coerceAtLeast(0))
    }
    LazyColumn(state = state, modifier = Modifier.fillMaxSize().padding(horizontal = Space.Lg), verticalArrangement = Arrangement.spacedBy(Space.Sm)) {
        item(key = "earlier") {
            AnimatedVisibility(
                visible = page.more,
                enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.ENTER)),
                exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
            ) {
                FButton("Earlier messages", onOlder, kind = ButtonKind.Secondary, modifier = Modifier.fillMaxWidth().testTag("earlier"))
            }
        }
        items(lines, key = { it.key }) { line ->
            // Decided once for the line: it never changes shape while it unfolds.
            val animated = remember { line.key !in initial && line.key !in appeared }
            LaunchedEffect(line.key) { if (animated) appeared += line.key }
            val move = Modifier.animateItem(placementSpec = Motion.spec(Motion.PANEL))
            Unfold(animated, move) {
                when (line) {
                    is Block.Message -> MessageView(phone, key, line.entry)
                    is Block.Steps -> StepsView(line.steps, open = line.key in unfolded) {
                        if (line.key in unfolded) unfolded -= line.key else unfolded += line.key
                    }
                }
            }
        }
    }
}

@Composable
private fun MessageView(phone: Phone, key: SessionKey, entry: Entry) {
    val palette = LocalPalette.current
    val mine = entry.kind == "user"
    Column(
        Modifier
            .fillMaxWidth()
            .background(if (mine) palette.card2 else palette.bg, RoundedCornerShape(12.dp))
            .testTag("entry:${entry.seq}")
            .padding(if (mine) Space.Md else Space.Xs),
        verticalArrangement = Arrangement.spacedBy(Space.Sm),
    ) {
        // Each picture sent sits inside the bubble of its message, above its words.
        entry.images.forEach { name -> PictureView(phone, key, name) }
        Label(entry.text, color = if (mine) palette.text else palette.text)
    }
}

/** A picture of the conversation: a placeholder that fades into the picture once it is in. */
@Composable
private fun PictureView(phone: Phone, key: SessionKey, name: String) {
    val palette = LocalPalette.current
    val picture by produceState<androidx.compose.ui.graphics.ImageBitmap?>(null, key, name) {
        value = try {
            val bytes = phone.picture(key.machine, key.agent, name)
            decode(bytes, 900)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (refused: PhoneException) {
            null
        }
    }
    val alpha by animateFloatAsState(if (picture != null) 1f else 0f, Motion.spec(Motion.ENTER), label = "picture")
    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp, max = 320.dp)
            .background(palette.field, RoundedCornerShape(8.dp))
            .testTag("picture:$name"),
        contentAlignment = Alignment.Center,
    ) {
        val shown = picture
        if (shown != null) {
            Image(shown, name, Modifier.fillMaxWidth().alpha(alpha), contentScale = ContentScale.Fit)
        } else {
            Label(name, style = Type.Small, color = palette.faint)
        }
    }
}

/** A picture's bytes as an image no wider than [width] px, or none when they are no picture. */
private fun decode(bytes: ByteArray, width: Int): androidx.compose.ui.graphics.ImageBitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (bounds.outWidth / (sample * 2) >= width) sample *= 2
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
}

/** The thoughts and tool calls between two messages: one line, until a click unfolds them. */
@Composable
private fun StepsView(steps: List<Entry>, open: Boolean, onToggle: () -> Unit) {
    val palette = LocalPalette.current
    val turn by animateFloatAsState(if (open) 90f else 0f, Motion.spec(Motion.PANEL), label = "steps chevron")
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier
                .clickable(role = Role.Button, indication = null, interactionSource = remember { MutableInteractionSource() }, onClick = onToggle)
                .padding(vertical = Space.Xs),
            horizontalArrangement = Arrangement.spacedBy(Space.Sm),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Label("›", color = palette.faint, modifier = Modifier.rotate(turn))
            Label(if (steps.size == 1) "1 step" else "${steps.size} steps", style = Type.Label, color = palette.mute)
        }
        AnimatedVisibility(
            visible = open,
            enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.PANEL)),
            exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
        ) {
            Column(Modifier.padding(start = Space.Lg), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                steps.forEach { step ->
                    Label(step.text, style = Type.Small, color = palette.faint, modifier = Modifier.testTag("entry:${step.seq}"))
                }
            }
        }
    }
}

/** The agents of the session's tree, read only: what each does. */
@Composable
private fun Tree(children: List<Child>) {
    val palette = LocalPalette.current
    Column(
        Modifier.fillMaxWidth().padding(horizontal = Space.Lg, vertical = Space.Xs),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Label("AGENTS", style = Type.Heading, color = palette.faint)
        children.forEach { child ->
            Unfold(animated = true, Modifier.testTag("child:${child.agent}")) {
                Row(Modifier.padding(start = (child.depth.coerceAtLeast(1) * 8).dp), horizontalArrangement = Arrangement.spacedBy(Space.Sm)) {
                    Label(child.title.ifEmpty { child.agent }, style = Type.Label)
                    Label(child.doing, style = Type.Small, color = palette.mute)
                }
            }
        }
    }
}

@Composable
private fun Composer(
    phone: Phone,
    page: Conversation,
    key: SessionKey,
    attached: MutableList<Picture>,
    pictures: PictureSource,
    offline: String?,
    onToast: (String) -> Unit,
) {
    val palette = LocalPalette.current
    val scope = rememberCoroutineScope()
    var text by remember(key) { mutableStateOf("") }
    var sending by remember(key) { mutableStateOf(false) }
    val canSend = (text.isNotBlank() || attached.isNotEmpty()) && !sending

    fun attach(picked: List<Picture>) {
        for (picture in picked) {
            val why = PictureLimits.refusal(attached, picture)
            if (why != null) onToast(why) else attached += picture
        }
    }

    fun send() {
        // A message to a machine that is off is refused, never kept: it would arrive hours later.
        if (offline != null) {
            onToast(offline)
            return
        }
        sending = true
        scope.launch {
            try {
                phone.send(key.machine, key.agent, text.trim(), attached.toList())
                text = ""
                attached.clear()
            } catch (refused: PhoneException) {
                onToast(refused.why)
            } finally {
                sending = false
            }
        }
    }

    Column(
        Modifier.fillMaxWidth().background(palette.side).padding(Space.Md),
        verticalArrangement = Arrangement.spacedBy(Space.Sm),
    ) {
        ModeControl(page.mode) { mode ->
            scope.launch {
                try {
                    phone.setMode(key.machine, key.agent, mode)
                } catch (refused: PhoneException) {
                    onToast(refused.why)
                }
            }
        }
        // What is attached unfolds in the composer and folds away when it leaves.
        AnimatedVisibility(
            visible = attached.isNotEmpty(),
            enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.ENTER)),
            exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(Space.Xs)) {
                attached.toList().forEachIndexed { index, picture ->
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .background(palette.card2, RoundedCornerShape(8.dp))
                            .testTag("attached:$index")
                            .padding(horizontal = Space.Md, vertical = Space.Xs),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(Space.Sm),
                    ) {
                        Thumbnail(picture)
                        Label("${picture.name.ifEmpty { "picture" }} · ${size(picture.bytes.size)}", style = Type.Label, modifier = Modifier.weight(1f))
                        FButton("Remove", { attached.removeAt(index) }, kind = ButtonKind.Ghost, modifier = Modifier.testTag("remove:$index"))
                    }
                }
            }
        }
        Field("Message", text, { text = it }, tag = "composer", singleLine = false, maxLines = 8)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.Sm), verticalAlignment = Alignment.CenterVertically) {
            FButton("Gallery", { pictures.gallery(::attach) }, kind = ButtonKind.Ghost)
            FButton("Camera", { pictures.camera { it?.let { picture -> attach(listOf(picture)) } } }, kind = ButtonKind.Ghost)
            Box(Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) {
                // Stop takes Send's place while a turn runs and Send has nothing to send.
                Crossfade(targetState = page.working && !canSend, animationSpec = Motion.spec(Motion.PANEL), label = "send or stop") { stop ->
                    if (stop) {
                        FButton(
                            "Stop",
                            { scope.launch { try { phone.stop(key.machine, key.agent) } catch (refused: PhoneException) { onToast(refused.why) } } },
                            kind = ButtonKind.Danger,
                            modifier = Modifier.testTag("stop"),
                        )
                    } else {
                        FButton("Send", ::send, kind = ButtonKind.Primary, enabled = canSend, modifier = Modifier.testTag("send"))
                    }
                }
            }
        }
    }
}

private fun size(bytes: Int): String =
    when {
        bytes < 1024 -> "$bytes B"
        bytes < 1024 * 1024 -> "${bytes / 1024} KiB"
        else -> "%.1f MiB".format(bytes / 1024.0 / 1024.0)
    }

/** A small look at an attached picture; a name alone when the bytes are no picture. */
@Composable
private fun Thumbnail(picture: Picture) {
    val bitmap = remember(picture) { runCatching { decode(picture.bytes, 96) }.getOrNull() }
    if (bitmap != null) Image(bitmap, null, Modifier.size(36.dp), contentScale = ContentScale.Crop)
}

/** Ask, Auto or Full: the chosen one's fill moves, and Full's is the alarm colour. */
@Composable
private fun ModeControl(mode: String, onChoose: (String) -> Unit) {
    val palette = LocalPalette.current
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.Xs)) {
        listOf("ask" to "Ask", "auto" to "Auto", "full" to "Full").forEach { (value, label) ->
            val chosen = mode == value
            val alarm = value == "full"
            val fill by animateColorAsState(
                when {
                    !chosen -> palette.card
                    alarm -> palette.fail
                    else -> palette.sel
                },
                Motion.spec(Motion.PANEL),
                label = "mode fill",
            )
            val ink by animateColorAsState(
                when {
                    !chosen -> palette.mute
                    alarm -> palette.onFail
                    else -> palette.text
                },
                Motion.spec(Motion.PANEL),
                label = "mode ink",
            )
            val ring by animateColorAsState(
                if (chosen && !alarm) palette.acc else if (alarm) palette.fail else palette.line,
                Motion.spec(Motion.PANEL),
                label = "mode ring",
            )
            Box(
                Modifier
                    .weight(1f)
                    .background(fill, RoundedCornerShape(10.dp))
                    .border(BorderStroke(1.dp, ring), RoundedCornerShape(10.dp))
                    .testTag("mode:$value")
                    .semantics { set(FillColour, fill) }
                    .clickable(
                        role = Role.RadioButton,
                        indication = null,
                        interactionSource = remember { MutableInteractionSource() },
                        onClick = { if (!chosen) onChoose(value) },
                    )
                    .padding(vertical = Space.Md),
                contentAlignment = Alignment.Center,
            ) {
                Label(label, color = ink)
            }
        }
    }
}
