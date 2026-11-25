package org.flottille.app

import android.graphics.BitmapFactory
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.Crossfade
import androidx.compose.animation.animateColorAsState
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
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.time.ZoneId
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.Confirm
import org.flottille.app.ui.Entrance
import org.flottille.app.ui.FButton
import org.flottille.app.ui.FillColour
import org.flottille.app.ui.Glyph
import org.flottille.app.ui.IconButton
import org.flottille.app.ui.Icons
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Sheet
import org.flottille.app.ui.Space
import org.flottille.app.ui.Tag
import org.flottille.app.ui.Type
import org.flottille.app.ui.Unfold
import org.flottille.core.Ask
import org.flottille.core.Child
import org.flottille.core.Conversation
import org.flottille.core.Entry
import org.flottille.core.MachineState
import org.flottille.core.OpenedTunnel
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

/**
 * What the page is made of, in the order it is drawn: a message, a delivery, or the steps between
 * two. [id] names it: its entry's number, and which of the event's entries it is when one prompt
 * held several deliveries.
 */
private sealed interface Block {
    val id: String
    val key: String

    data class Message(val entry: Entry, override val id: String) : Block {
        override val key get() = "m:$id"
    }

    /** Something Flottille sent the agent (a subscription's news, a child's report…): one line, its body folded. */
    data class Delivery(val entry: Entry, override val id: String) : Block {
        override val key get() = "d:$id"
    }

    data class Steps(val steps: List<Entry>) : Block {
        override val id get() = "${steps.first().seq}"
        override val key get() = "s:$id"
    }
}

/** The conversation's entries as lines: the thoughts and the tool calls between two messages fold into one. */
private fun toLines(entries: List<Entry>): List<Block> {
    val out = mutableListOf<Block>()
    val steps = mutableListOf<Entry>()
    val seen = mutableMapOf<Long, Int>()
    fun flush() {
        if (steps.isNotEmpty()) out += Block.Steps(steps.toList())
        steps.clear()
    }
    for (entry in entries) {
        if (entry.kind == "thought" || entry.kind == "tool") {
            steps += entry
            continue
        }
        flush()
        val nth = seen.getOrDefault(entry.seq, 0)
        seen[entry.seq] = nth + 1
        val id = if (nth == 0) "${entry.seq}" else "${entry.seq}.$nth"
        out += if (entry.kind == "delivery") Block.Delivery(entry, id) else Block.Message(entry, id)
    }
    flush()
    return out
}

/**
 * The session page (`docs/design.md` §10, for a phone): the conversation live, its steps folded,
 * its pictures shown, what Flottille delivered folded to one line; it follows its end unless the
 * user scrolled up. The composer with pictures from the gallery, the camera or the share sheet;
 * the ask card and the requests, answered in place; the mode (one control, its choices in a
 * panel), Stop, and the tree read only in a panel of the top bar. The
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
    tunnels: TunnelHost = TunnelHost.None,
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

    // Full lets the agents act without asking: on the phone one tap is not enough to give it, so
    // the one confirmation dialog asks first (Ask and Auto are one tap).
    var askingFull by remember(key) { mutableStateOf(false) }
    var choosingMode by remember(key) { mutableStateOf(false) }
    var treeOpen by remember(key) { mutableStateOf(false) }
    fun setMode(mode: String) {
        scope.launch {
            try {
                phone.setMode(key.machine, key.agent, mode)
            } catch (refused: PhoneException) {
                onToast(refused.why)
            }
        }
    }

    // A run's page in the app, or handed to the user's browser: through a tunnel when the address is
    // the machine's own, as it is when it is a page of the web.
    var browsing by remember(key) { mutableStateOf<OpenedTunnel?>(null) }
    var shownBrowsing by remember { mutableStateOf(browsing) }
    if (browsing != null) shownBrowsing = browsing
    // The in-app browser's tunnel lives with its panel: it closes with it, however the panel goes
    // (Close, Back, or the page itself leaving). Another app of the phone can reach a tunnel's
    // listener, so none stays open for a page nobody looks at.
    DisposableEffect(browsing?.tunnel?.tunnel) {
        val tunnel = browsing?.tunnel?.tunnel
        onDispose {
            if (tunnel != null) {
                try {
                    phone.closeTunnel(tunnel)
                } catch (gone: PhoneException) {
                    // Ended already: the view says why.
                }
            }
        }
    }
    val busy = remember(key) { mutableStateListOf<String>() }
    val opening = Opening(
        readOnly = readOnly,
        busy = busy.toSet(),
        isLocal = phone::isLocal,
        open = { slot, run, url, inBrowser ->
            if (!phone.isLocal(url)) {
                tunnels.browse(url)
            } else {
                busy += slot
                scope.launch {
                    try {
                        val opened = phone.openAddress(key.machine, run, url, inBrowser)
                        if (inBrowser) tunnels.keepForBrowser(opened) else browsing = opened
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (refused: PhoneException) {
                        onToast("Not opened: ${refused.why}")
                    } finally {
                        busy -= slot
                    }
                }
            }
        },
        close = { tunnel ->
            try {
                phone.closeTunnel(tunnel)
            } catch (refused: PhoneException) {
                onToast(refused.why)
            }
        },
    )

    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().background(palette.bg).testTag("page")) {
            val project = view.projects.firstOrNull { p -> p.sessions.any { it.machine == key.machine && it.agent == key.agent } }?.name
            Header(
                page = page,
                key = key,
                project = project,
                readOnly = readOnly,
                onBack = onBack,
                onTree = if (page != null && page.children.isNotEmpty()) ({ treeOpen = true }) else null,
            )
            if (page != null) RunsSection(page.runs, view.tunnels.filter { it.machine == key.machine }, opening)
            Box(Modifier.weight(1f).fillMaxWidth()) {
                if (page == null) {
                    Label(
                        if (online) "Loading the conversation…" else "This machine is not reachable.",
                        color = palette.mute,
                        modifier = Modifier.padding(Space.Lg),
                    )
                } else {
                    ConversationList(phone, key, page, opening, onOlder = {
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
                                    if (refused.gone) {
                                        // Answered already or gone: said on its card, which then folds.
                                        refusedAsks[ask.asked] = ask to refused.why
                                        delay(REFUSED_MILLIS)
                                        folded += "ask:${ask.asked}"
                                        refusedAsks.remove(ask.asked)
                                    } else {
                                        // A link that ended, a time out: the question still waits, so
                                        // does its card, and the user can answer again.
                                        onToast(refused.why)
                                    }
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
                                    if (refused.gone) {
                                        // Decided already or gone: said on its card, which then folds.
                                        refusedRequests[request.number] = request to refused.why
                                        delay(REFUSED_MILLIS)
                                        folded += "request:${request.number}"
                                        refusedRequests.remove(request.number)
                                    } else {
                                        // A link that ended, a time out: the request still waits, so
                                        // does its card, and the user can answer again.
                                        onToast(refused.why)
                                    }
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
                page.ended -> Row(
                    Modifier.fillMaxWidth().background(palette.side).padding(horizontal = Space.Lg, vertical = Space.Md),
                ) {
                    Label("This session has ended.", style = Type.Label, color = palette.mute)
                }
                else -> Composer(
                    phone = phone,
                    page = page,
                    key = key,
                    attached = attached,
                    pictures = pictures,
                    offline = if (online) null else offlineSentence(key.machine, machine, zone),
                    onToast = onToast,
                    onMode = { choosingMode = true },
                )
            }
        }
        Sheet(visible = treeOpen, title = "Agents", tag = "tree", onClose = { treeOpen = false }) {
            TreeRows(page?.children.orEmpty())
        }
        Sheet(visible = choosingMode, title = "Mode", tag = "modes", onClose = { choosingMode = false }) {
            ModeChoices(page?.mode.orEmpty()) { mode ->
                choosingMode = false
                // Full lets the agents act without asking: it is confirmed first, Ask and Auto are one tap.
                when {
                    mode == page?.mode -> Unit
                    mode == "full" -> askingFull = true
                    else -> setMode(mode)
                }
            }
        }
        Confirm(
            visible = askingFull,
            title = "Switch to Full mode?",
            body = "In Full the agents of this session run commands and change files without asking you, until you switch back to Ask or Auto.",
            confirm = "Switch to Full",
            onConfirm = {
                askingFull = false
                setMode("full")
            },
            onCancel = { askingFull = false },
        )
        AnimatedVisibility(
            visible = browsing != null,
            enter = slideInVertically(Motion.spec(Motion.PANEL)) { it / 8 } + fadeIn(Motion.spec(Motion.PANEL)),
            exit = slideOutVertically(Motion.spec(Motion.PANEL)) { it / 8 } + fadeOut(Motion.spec(Motion.PANEL)),
        ) {
            shownBrowsing?.let { opened ->
                TunnelBrowser(
                    opened = opened,
                    ended = view.tunnelsEnded.lastOrNull { it.tunnel == opened.tunnel.tunnel }?.reason,
                    onOutside = tunnels::browse,
                    onClose = { browsing = null },
                )
            }
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

/** The fill of a row or a line that is pressed: it fades over [Motion.HOVER], and back. */
@Composable
private fun pressedFill(source: MutableInteractionSource): androidx.compose.ui.graphics.Color {
    val palette = LocalPalette.current
    val pressed by source.collectIsPressedAsState()
    val fill by animateColorAsState(if (pressed) palette.hover else palette.hover.copy(alpha = 0f), Motion.spec(Motion.HOVER), label = "pressed fill")
    return fill
}

/** The top bar: Back, the session and where it runs, and the tree's panel when it has one. */
@Composable
private fun Header(page: Conversation?, key: SessionKey, project: String?, readOnly: Boolean, onBack: () -> Unit, onTree: (() -> Unit)?) {
    val palette = LocalPalette.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(start = Space.Xs, end = Space.Sm),
        horizontalArrangement = Arrangement.spacedBy(Space.Xs),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(Icons.Back, "Back", onBack, modifier = Modifier.testTag("page-back"))
        Column(Modifier.weight(1f).padding(start = Space.Xs)) {
            BasicText(
                page?.title.orEmpty().ifEmpty { "Session" },
                style = Type.Body.copy(fontSize = 16.5.sp, fontWeight = FontWeight.SemiBold, color = palette.text),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            BasicText(
                listOfNotNull(key.machine, project).joinToString(" · "),
                style = Type.Small.copy(color = palette.faint),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (readOnly) Tag("View only")
        AnimatedVisibility(
            visible = onTree != null,
            enter = fadeIn(Motion.spec(Motion.ENTER)) + scaleIn(Motion.spec(Motion.ENTER), initialScale = 0.8f),
            exit = fadeOut(Motion.spec(Motion.HOVER)) + scaleOut(Motion.spec(Motion.HOVER), targetScale = 0.8f),
        ) {
            IconButton(Icons.Tree, "Agents", { onTree?.invoke() }, modifier = Modifier.testTag("open-tree"))
        }
    }
}

@Composable
private fun ConversationList(phone: Phone, key: SessionKey, page: Conversation, opening: Opening, onOlder: () -> Unit) {
    val palette = LocalPalette.current
    val scope = rememberCoroutineScope()
    val lines = toLines(page.entries)
    // It opens on its end: the line before "Earlier messages" is the last one.
    val state = rememberLazyListState(initialFirstVisibleItemIndex = lines.size)
    // What was there when the page opened is simply there; what arrives after unfolds and fades in.
    val initial = remember(key) { lines.map { it.key }.toSet() }
    val appeared = remember(key) { mutableStateListOf<String>() }
    val unfolded = remember(key) { mutableStateListOf<String>() }

    // It follows its end until the user drags it away from it, and again once they are back at it.
    var following by remember(key) { mutableStateOf(true) }
    val dragged by state.interactionSource.collectIsDraggedAsState()
    LaunchedEffect(state) {
        snapshotFlow { state.canScrollForward to dragged }.collect { (more, drag) ->
            if (!more) following = true else if (drag) following = false
        }
    }
    // Followed, each change of the end keeps it in sight: a line that arrives, one that unfolds.
    LaunchedEffect(state) {
        snapshotFlow {
            val info = state.layoutInfo
            Triple(info.totalItemsCount, state.canScrollForward, info.visibleItemsInfo.lastOrNull()?.let { it.index to it.size })
        }.collect { (count, more, last) ->
            if (following && more && !dragged && count > 0) state.scrollToItem(count - 1, last?.second ?: 0)
        }
    }
    // What arrived while the user read above: said by an affordance that brings them down.
    val last = lines.lastOrNull()?.key
    var seen by remember(key) { mutableStateOf(last) }
    LaunchedEffect(following, last) { if (following) seen = last }
    val unseen = !following && last != seen

    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            state = state,
            modifier = Modifier.fillMaxSize().testTag("conversation"),
            contentPadding = PaddingValues(start = Space.Lg, end = Space.Lg, top = Space.Sm, bottom = Space.Lg),
            verticalArrangement = Arrangement.spacedBy(Space.Sm),
        ) {
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
                val open = line.key in unfolded
                val toggle: () -> Unit = { if (open) unfolded -= line.key else unfolded += line.key }
                // The tag is on what unfolds: the line's own size is what grows.
                Unfold(animated, move.testTag("line:${line.key}")) {
                    when (line) {
                        is Block.Message -> when (line.entry.kind) {
                            "link" -> LinkCard(line.entry.seq, line.entry.text, line.entry.link, opening)
                            else -> MessageView(phone, key, line.entry, line.id)
                        }
                        is Block.Delivery -> DeliveryView(line.entry, line.id, open, toggle)
                        is Block.Steps -> StepsView(line.steps, open, toggle)
                    }
                }
            }
        }
        AnimatedVisibility(
            visible = unseen,
            modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = Space.Md),
            enter = fadeIn(Motion.spec(Motion.ENTER)) + slideInVertically(Motion.spec(Motion.ENTER)) { it / 2 },
            exit = fadeOut(Motion.spec(Motion.HOVER)) + slideOutVertically(Motion.spec(Motion.PANEL)) { it / 2 },
        ) {
            val source = remember { MutableInteractionSource() }
            val pressed by source.collectIsPressedAsState()
            val fill by animateColorAsState(if (pressed) palette.mute else palette.text, Motion.spec(Motion.HOVER), label = "new messages fill")
            Row(
                Modifier
                    .background(fill, RoundedCornerShape(18.dp))
                    .testTag("new-messages")
                    .clickable(interactionSource = source, indication = null, role = Role.Button) {
                        seen = last
                        scope.launch { state.animateScrollToItem((state.layoutInfo.totalItemsCount - 1).coerceAtLeast(0)) }
                    }
                    .padding(horizontal = Space.Md, vertical = Space.Sm),
                horizontalArrangement = Arrangement.spacedBy(Space.Xs),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Glyph(Icons.Down, palette.bg, size = 14.dp)
                Label("New messages", style = Type.Label.copy(fontWeight = FontWeight.Medium), color = palette.bg)
            }
        }
    }
}

/** A message: the user's in a bubble on the right, the agent's as plain text. */
@Composable
private fun MessageView(phone: Phone, key: SessionKey, entry: Entry, id: String) {
    val palette = LocalPalette.current
    if (entry.kind != "user") {
        Label(entry.text, style = Type.Body.copy(lineHeight = 22.sp), modifier = Modifier.fillMaxWidth().testTag("entry:$id"))
        return
    }
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        Box(Modifier.fillMaxWidth(0.85f), contentAlignment = Alignment.CenterEnd) {
            Column(
                Modifier
                    .background(palette.card2, RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp, bottomEnd = 4.dp, bottomStart = 16.dp))
                    .testTag("entry:$id")
                    .padding(horizontal = Space.Md, vertical = 9.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                // Each picture sent sits inside the bubble of its message, above its words.
                entry.images.forEach { name -> PictureView(phone, key, name) }
                if (entry.text.isNotEmpty()) Label(entry.text)
            }
        }
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

/** A chevron that turns a quarter as what it opens unfolds, over [Motion.PANEL]. */
@Composable
private fun Chevron(open: Boolean, down: Boolean = false) {
    val palette = LocalPalette.current
    val turn by animateFloatAsState((if (open) 90f else 0f) + (if (down) 90f else 0f), Motion.spec(Motion.PANEL), label = "chevron")
    Glyph(Icons.Chevron, palette.faint, Modifier.rotate(turn), size = 13.dp)
}

/** The thoughts and tool calls between two messages: one line, until a tap unfolds them. */
@Composable
private fun StepsView(steps: List<Entry>, open: Boolean, onToggle: () -> Unit) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    val ink by animateColorAsState(if (source.collectIsPressedAsState().value) palette.text else palette.mute, Motion.spec(Motion.HOVER), label = "steps ink")
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier
                .fillMaxWidth()
                .clickable(role = Role.Button, indication = null, interactionSource = source, onClick = onToggle)
                .padding(vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(Space.Sm),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Glyph(if (steps.all { it.kind == "thought" }) Icons.Spark else Icons.File, ink, size = 13.dp)
            Label(if (steps.size == 1) "1 step" else "${steps.size} steps", style = Type.Label, color = ink, modifier = Modifier.weight(1f))
            Chevron(open)
        }
        AnimatedVisibility(
            visible = open,
            enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.PANEL)),
            exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
        ) {
            Column(Modifier.padding(start = Space.Xl, top = 2.dp, bottom = 6.dp), verticalArrangement = Arrangement.spacedBy(Space.Xs)) {
                steps.forEach { step ->
                    Label(step.text, style = Type.Small, color = palette.faint, modifier = Modifier.testTag("entry:${step.seq}"))
                }
            }
        }
    }
}

/**
 * Something Flottille sent the agent, drawn as the window draws it: what it is and its one line,
 * its body folded under it until a tap opens it. Never the user's bubble, never its markers.
 */
@Composable
private fun DeliveryView(entry: Entry, id: String, open: Boolean, onToggle: () -> Unit) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    val press = pressedFill(source)
    val foldable = entry.text.isNotBlank()
    val icon = when (entry.delivery) {
        "report" -> Icons.Report
        "woken by" -> Icons.Bell
        "message", "started by", "brief" -> Icons.Tree
        else -> Icons.Spark
    }
    Column(
        Modifier
            .fillMaxWidth()
            .background(palette.card, RoundedCornerShape(12.dp))
            .border(BorderStroke(1.dp, palette.line), RoundedCornerShape(12.dp)),
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(press, RoundedCornerShape(12.dp))
                .testTag("delivery:$id")
                .clickable(enabled = foldable, role = Role.Button, indication = null, interactionSource = source, onClick = onToggle)
                .padding(horizontal = Space.Md, vertical = 9.dp),
            horizontalArrangement = Arrangement.spacedBy(Space.Sm),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Glyph(icon, palette.faint, size = 13.dp)
            Label(entry.delivery, style = Type.Label.copy(fontWeight = FontWeight.SemiBold), color = palette.mute)
            BasicText(
                entry.summary.ifEmpty { entry.text.lineSequence().firstOrNull { it.isNotBlank() }.orEmpty() },
                style = Type.Label.copy(color = palette.mute),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            if (foldable) Chevron(open)
        }
        AnimatedVisibility(
            visible = open,
            enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.PANEL)),
            exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
        ) {
            Label(
                entry.text,
                style = Type.Label.copy(lineHeight = 19.sp),
                color = palette.mute,
                modifier = Modifier.padding(start = 36.dp, end = Space.Md, bottom = 10.dp).testTag("delivery-body:$id"),
            )
        }
    }
}

/** The agents of the session's tree, read only, in their panel: each with what it does. */
@Composable
private fun TreeRows(children: List<Child>) {
    val palette = LocalPalette.current
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState())) {
        children.forEachIndexed { index, child ->
            Entrance(index) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .testTag("child:${child.agent}")
                        .padding(start = Space.Xl - Space.Xs + ((child.depth.coerceAtLeast(1) - 1) * 20).dp, end = Space.Xl - Space.Xs, top = 9.dp, bottom = 9.dp),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    val ended = child.doing.startsWith("ended") || child.doing.startsWith("failed")
                    Box(Modifier.size(18.dp), contentAlignment = Alignment.Center) {
                        Box(
                            Modifier
                                .size(9.dp)
                                .border(BorderStroke(1.5.dp, if (ended) palette.faint else palette.run), CircleShape)
                                .background(if (ended) palette.bg.copy(alpha = 0f) else palette.run, CircleShape),
                        )
                    }
                    Column(Modifier.weight(1f)) {
                        BasicText(child.title.ifEmpty { child.agent }, style = Type.Body.copy(fontWeight = FontWeight.Medium, color = palette.text), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        BasicText(child.doing, style = Type.Small.copy(color = palette.faint), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        }
        Label(
            "Read only: a sub-agent belongs to its parent.",
            style = Type.Small,
            color = palette.faint,
            modifier = Modifier.padding(horizontal = Space.Xl - Space.Xs, vertical = Space.Sm),
        )
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
    onMode: () -> Unit,
) {
    val palette = LocalPalette.current
    val scope = rememberCoroutineScope()
    var text by remember(key) { mutableStateOf("") }
    var sending by remember(key) { mutableStateOf(false) }
    val canSend = (text.isNotBlank() || attached.isNotEmpty()) && !sending

    fun attach(picked: Pictured) {
        picked.refused?.let(onToast)
        for (picture in picked.pictures) {
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
        Modifier
            .fillMaxWidth()
            .background(palette.side)
            .padding(start = 10.dp, end = 10.dp, top = Space.Sm, bottom = Space.Sm),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
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
                            .background(palette.card2, RoundedCornerShape(10.dp))
                            .testTag("attached:$index")
                            .padding(start = Space.Sm, top = Space.Xs, bottom = Space.Xs),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(Space.Sm),
                    ) {
                        Thumbnail(picture)
                        Label("${picture.name.ifEmpty { "picture" }} · ${size(picture.bytes.size)}", style = Type.Label, modifier = Modifier.weight(1f))
                        IconButton(Icons.Close, "Remove", { attached.removeAt(index) }, modifier = Modifier.testTag("remove:$index"), size = 36.dp)
                    }
                }
            }
        }
        MessageField(text, { text = it })
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(Icons.Image, "Gallery", { pictures.gallery(::attach) }, size = 40.dp)
            IconButton(Icons.Camera, "Camera", { pictures.camera { it?.let { picture -> attach(Pictured(listOf(picture))) } } }, size = 40.dp)
            ModeChip(page.mode, onMode, Modifier.padding(start = Space.Xs))
            Box(Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) {
                // Stop takes Send's place while a turn runs and Send has nothing to send.
                Crossfade(targetState = page.working && !canSend, animationSpec = Motion.spec(Motion.PANEL), label = "send or stop") { stop ->
                    if (stop) {
                        IconButton(
                            Icons.Stop,
                            "Stop",
                            { scope.launch { try { phone.stop(key.machine, key.agent) } catch (refused: PhoneException) { onToast(refused.why) } } },
                            kind = ButtonKind.Secondary,
                            modifier = Modifier.testTag("stop"),
                        )
                    } else {
                        IconButton(Icons.Send, "Send", ::send, kind = ButtonKind.Primary, enabled = canSend, modifier = Modifier.testTag("send"))
                    }
                }
            }
        }
    }
}

/** The composer's field: round, its placeholder said until something is written; its ring fades in with the focus. */
@Composable
private fun MessageField(text: String, onChange: (String) -> Unit) {
    val palette = LocalPalette.current
    var focused by remember { mutableStateOf(false) }
    val ring by animateColorAsState(if (focused) palette.faint else palette.line2, Motion.spec(Motion.HOVER), label = "composer ring")
    BasicTextField(
        value = text,
        onValueChange = onChange,
        maxLines = 6,
        textStyle = Type.Body.copy(color = palette.text),
        cursorBrush = SolidColor(palette.acc),
        modifier = Modifier
            .fillMaxWidth()
            .testTag("composer")
            .semantics { contentDescription = "Message" }
            .background(palette.field, RoundedCornerShape(20.dp))
            .border(BorderStroke(1.dp, ring), RoundedCornerShape(20.dp))
            .onFocusChanged { focused = it.isFocused }
            .padding(horizontal = 14.dp, vertical = 10.dp),
        decorationBox = { field ->
            Box {
                if (text.isEmpty()) Label("Write to the agent", color = palette.faint)
                field()
            }
        },
    )
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

/** The names of the modes, and what each lets the agents do. */
private val Modes = listOf(
    Triple("ask", "Ask", "Every call that is not harmless asks you."),
    Triple("auto", "Auto", "The judge allows what is harmless and asks you the rest."),
    Triple("full", "Full", "The agents run commands and change files without asking you."),
)

/** The mode, one control: its name, in the alarm colour in Full; a tap opens the choices. */
@Composable
private fun ModeChip(mode: String, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    val palette = LocalPalette.current
    val alarm = mode == "full"
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val fill by animateColorAsState(
        when {
            alarm -> palette.failBg
            pressed -> palette.sel
            else -> palette.bg.copy(alpha = 0f)
        },
        Motion.spec(Motion.PANEL),
        label = "mode fill",
    )
    val ink by animateColorAsState(if (alarm) palette.fail else palette.text, Motion.spec(Motion.PANEL), label = "mode ink")
    val ring by animateColorAsState(if (alarm) palette.failLine else palette.line2, Motion.spec(Motion.PANEL), label = "mode ring")
    val name = Modes.firstOrNull { it.first == mode }?.second ?: "Mode"
    Row(
        modifier
            .background(fill, RoundedCornerShape(16.dp))
            .border(BorderStroke(1.dp, ring), RoundedCornerShape(16.dp))
            .testTag("mode")
            .semantics { set(FillColour, fill) }
            .clickable(role = Role.Button, indication = null, interactionSource = source, onClick = onOpen)
            .padding(start = Space.Md, end = Space.Sm, top = 6.dp, bottom = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Label("Mode", style = Type.Small, color = palette.faint)
        // The name of the mode cross-fades when it changes.
        Crossfade(targetState = name, animationSpec = Motion.spec(Motion.HOVER), label = "mode name") { shown ->
            Label(shown, style = Type.Label.copy(fontWeight = FontWeight.Medium), color = ink)
        }
        Chevron(open = false, down = true)
    }
}

/** The three modes in their panel, the chosen one marked: its fill and its check move over [Motion.PANEL]. */
@Composable
private fun ModeChoices(mode: String, onChoose: (String) -> Unit) {
    val palette = LocalPalette.current
    Column(Modifier.fillMaxWidth().padding(horizontal = Space.Md)) {
        Modes.forEachIndexed { index, (value, name, says) ->
            val chosen = mode == value
            val alarm = value == "full"
            val source = remember { MutableInteractionSource() }
            val pressed by source.collectIsPressedAsState()
            val fill by animateColorAsState(
                when {
                    chosen && alarm -> palette.failBg
                    chosen -> palette.sel
                    pressed -> palette.hover
                    else -> palette.bg.copy(alpha = 0f)
                },
                Motion.spec(Motion.PANEL),
                label = "mode choice fill",
            )
            val mark by animateFloatAsState(if (chosen) 1f else 0f, Motion.spec(Motion.PANEL), label = "mode choice mark")
            Entrance(index) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .background(fill, RoundedCornerShape(12.dp))
                        .testTag("mode:$value")
                        .semantics { selected = chosen }
                        .clickable(role = Role.RadioButton, indication = null, interactionSource = source) { onChoose(value) }
                        .padding(horizontal = Space.Md, vertical = Space.Md),
                    horizontalArrangement = Arrangement.spacedBy(Space.Md),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Label(name, style = Type.Body.copy(fontWeight = FontWeight.Medium), color = if (alarm) palette.fail else palette.text)
                        Label(says, style = Type.Small, color = palette.faint)
                    }
                    Glyph(Icons.Check, palette.text, Modifier.alpha(mark), size = 18.dp)
                }
            }
        }
        Label(
            "Full asks you to confirm it first.",
            style = Type.Small,
            color = palette.faint,
            modifier = Modifier.padding(horizontal = Space.Md, vertical = Space.Sm),
        )
    }
}
