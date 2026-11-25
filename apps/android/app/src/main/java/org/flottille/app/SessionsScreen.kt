package org.flottille.app

import android.content.Context
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.Crossfade
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.time.ZoneId
import org.flottille.app.ui.Entrance
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Type
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.Member
import org.flottille.core.PhoneView
import org.flottille.core.SessionRow

/** A session's mark: the state is the row's mark, so a row never moves when it changes. */
private enum class Mark(val said: String) { Needs("Needs you"), Running("Running"), Rest("Idle"), Ended("Ended") }

private fun SessionRow.ended() = state == "ended" || state == "failed"

private fun SessionRow.mark(): Mark =
    when {
        ended() -> Mark.Ended
        waiting -> Mark.Needs
        state == "working" || state == "starting" -> Mark.Running
        else -> Mark.Rest
    }

/** The most urgent of [marks]: what a folded pilot or project shows for the rows it hides. */
private fun strongest(marks: List<Mark>): Mark = marks.minByOrNull { it.ordinal } ?: Mark.Rest

/** What the list is made of, in the order it is drawn. */
private sealed interface Line {
    val key: String

    data class MachineHeader(val machine: String, val known: Machine?, val member: Member?, val open: Boolean) : Line {
        override val key get() = "machine:$machine"
    }

    data class ProjectHeader(
        val machine: String,
        val project: String,
        val count: Int,
        val open: Boolean,
        val hidden: Mark,
        val offline: Boolean,
    ) : Line {
        override val key get() = "project:$machine/$project"
    }

    /** A session's row; [leads] the sessions it leads, [led] whether it sits under its pilot. */
    data class Session(
        val session: SessionRow,
        val offline: Boolean,
        val leads: List<SessionRow> = emptyList(),
        val open: Boolean = false,
        val led: Boolean = false,
        val lastLed: Boolean = false,
        val project: String = "",
        val member: Member? = null,
    ) : Line {
        override val key get() = "session:${session.machine}:${session.agent}"
    }

    /** What "Ended" says once of a machine whose ended sessions it cannot list. */
    data class Unlisted(val machine: String, val member: Member?, val said: String) : Line {
        override val key get() = "unlisted:$machine"
    }
}

/**
 * What the tab has folded, kept on the phone as the window keeps its sidebar's: each machine, each
 * project of a machine, each pilot, the "Ended" group. A node that was never folded or unfolded
 * keeps its default: a machine open when it is online, a project open, a pilot and "Ended" folded.
 */
private class Folds(context: Context) {
    private val prefs = context.getSharedPreferences("sessions-folds", Context.MODE_PRIVATE)
    private val open = mutableStateMapOf<String, Boolean>().apply {
        prefs.all.forEach { (key, value) -> if (value is Boolean) put(key, value) }
    }

    fun isOpen(key: String, default: Boolean): Boolean = open[key] ?: default

    fun toggle(key: String, default: Boolean) {
        val now = !isOpen(key, default)
        open[key] = now
        prefs.edit().putBoolean(key, now).apply()
    }

    companion object {
        fun machine(machine: String) = "machine:$machine"

        fun project(machine: String, project: String) = "project:$machine\u001f$project"

        fun lead(session: SessionRow) = "lead:${session.machine}:${session.agent}"

        const val ENDED = "ended"
    }
}

/** The tree of the live sessions, and the rows of "Ended" with what it says of the machines it cannot list. */
private class Tree(val lines: List<Line>, val ended: List<Line>, val endedCount: Int)

private fun tree(view: PhoneView, folds: Folds, zone: ZoneId): Tree {
    val online = view.machines.filter { it.state == MachineState.Online }.map { it.name }.toSet()
    val members = view.members.associateBy { it.name }
    val all = view.projects.flatMap { project -> project.sessions.map { project.name to it } }
    // Newest first, as the window's sidebar: what a session reports never moves its row.
    val live = all.filterNot { it.second.ended() }.sortedByDescending { it.second.created }
    val ended = all.filter { it.second.ended() }.sortedByDescending { it.second.last }
    val machines = (view.machines.map { it.name } + all.map { it.second.machine }).distinct()
    val out = mutableListOf<Line>()
    for (machine in machines) {
        val known = view.machines.firstOrNull { it.name == machine }
        val mine = live.filter { it.second.machine == machine }
        if (mine.isEmpty()) continue
        val offline = machine !in online
        // An offline machine keeps its projects as last read, folded and greyed.
        val machineOpen = folds.isOpen(Folds.machine(machine), default = !offline)
        out += Line.MachineHeader(machine, known, members[machine], machineOpen)
        if (!machineOpen) continue
        for (project in mine.map { it.first }.distinct().sorted()) {
            val sessions = mine.filter { it.first == project }.map { it.second }
            val ids = sessions.map { it.agent }.toSet()
            // A session sits under its pilot when the pilot is a live row of the same list; else it
            // is a row of its own, as the window shows a session whose pilot works elsewhere.
            fun pilotOf(session: SessionRow) = session.pilot.takeIf { it.isNotEmpty() && it != session.agent && it in ids }
            val open = folds.isOpen(Folds.project(machine, project), default = true)
            out += Line.ProjectHeader(machine, project, sessions.size, open, strongest(sessions.map { it.mark() }), offline)
            if (!open) continue
            for (root in sessions.filter { pilotOf(it) == null }) {
                val leads = sessions.filter { pilotOf(it) == root.agent }
                val leadOpen = leads.isNotEmpty() && folds.isOpen(Folds.lead(root), default = false)
                out += Line.Session(root, offline, leads, leadOpen)
                if (leadOpen) {
                    leads.forEachIndexed { i, led ->
                        out += Line.Session(led, offline, led = true, lastLed = i == leads.lastIndex)
                    }
                }
            }
        }
    }
    // Ended, a row has left its place: it says its project and its machine's icon.
    val endedLines = ended.map { (project, s) -> Line.Session(s, s.machine !in online, project = project, member = members[s.machine]) } +
        view.machines.filter { it.state != MachineState.Online }.map {
            val why = if (it.state == MachineState.Offline) "is ${stateLine(it, zone).lowercase()}" else "needs an update"
            Line.Unlisted(it.name, members[it.name], "${it.name} $why: its ended sessions are not listed")
        }
    return Tree(out, endedLines, ended.size)
}

/**
 * The Sessions tab (`docs/remote.md`, "The Android app"; design B, #1441): the window's sidebar.
 * Each machine, said once with its mark and its state, folded or not; its projects, folded or not;
 * their sessions in compact rows newest first, a session led by another under its pilot (folded by
 * default); the state is the row's mark, so a row never moves when its session reports. The ended
 * sessions leave their places for one folded group pinned at the bottom. A session of a machine
 * that is offline is greyed and is not opened.
 */
@Composable
fun SessionsScreen(
    view: PhoneView,
    zone: ZoneId,
    now: () -> Long,
    onOpen: (machine: String, agent: String) -> Unit,
) {
    val palette = LocalPalette.current
    val context = LocalContext.current
    val folds = remember(context) { Folds(context) }
    val tree = tree(view, folds, zone)
    val at = now()
    Entrance(0, Modifier.fillMaxSize()) {
        if (tree.lines.isEmpty() && tree.endedCount == 0) {
            Column(Modifier.fillMaxSize().padding(Space.Xl), verticalArrangement = Arrangement.spacedBy(Space.Sm)) {
                Label("No sessions yet", style = Type.Heading)
                Label("The sessions of your machines show here as they start.", color = palette.mute)
            }
        } else {
            Column(Modifier.fillMaxSize()) {
                LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(bottom = Space.Md)) {
                    items(tree.lines, key = { it.key }) { line ->
                        // A row inserted, removed or moved unfolds, folds away or slides, never jumps.
                        val move = Modifier.animateItem(
                            fadeInSpec = Motion.spec(Motion.ENTER),
                            placementSpec = Motion.spec(Motion.PANEL),
                            fadeOutSpec = Motion.spec(Motion.HOVER),
                        )
                        when (line) {
                            is Line.MachineHeader -> MachineHeader(line, zone, move) {
                                folds.toggle(Folds.machine(line.machine), default = line.known?.state == MachineState.Online)
                            }
                            is Line.ProjectHeader -> ProjectHeader(line, move) {
                                folds.toggle(Folds.project(line.machine, line.project), default = true)
                            }
                            is Line.Session -> SessionLine(
                                line,
                                view,
                                at,
                                onOpen,
                                onFold = { folds.toggle(Folds.lead(line.session), default = false) },
                                modifier = move,
                            )
                            is Line.Unlisted -> {}
                        }
                    }
                }
                if (tree.endedCount > 0) {
                    EndedGroup(tree, folds.isOpen(Folds.ENDED, default = false), view, at, onOpen) {
                        folds.toggle(Folds.ENDED, default = false)
                    }
                }
            }
        }
    }
}

@Composable
private fun MachineHeader(line: Line.MachineHeader, zone: ZoneId, modifier: Modifier, onToggle: () -> Unit) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    val offline = line.known?.state != MachineState.Online
    val name by animateColorAsState(if (offline) palette.mute else palette.text, Motion.spec(Motion.PANEL), label = "machine name")
    val state = line.known?.let { stateLine(it, zone).lowercase() } ?: "not known"
    Row(
        modifier
            .fillMaxWidth()
            .background(pressFill(source))
            .testTag(line.key)
            .semantics { stateDescription = if (line.open) "Unfolded" else "Folded" }
            .clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onToggle)
            .padding(start = Space.Lg, end = Space.Lg, top = Space.Md, bottom = Space.Xs),
        horizontalArrangement = Arrangement.spacedBy(Space.Sm),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        MachineMark(line.member, badgeOf(line.known?.state), said = "${line.machine}, $state")
        Label(line.machine, style = Type.Body.copy(fontWeight = FontWeight.SemiBold), color = name)
        Crossfade(state, Modifier.weight(1f), animationSpec = Motion.spec(Motion.PANEL), label = "machine state") {
            Label(it, style = Type.Label, color = palette.faint)
        }
        Chevron(line.open)
    }
}

/** A press fades the fill in over [Motion.HOVER], as every interactive element of the window. */
@Composable
private fun pressFill(source: MutableInteractionSource, enabled: Boolean = true): androidx.compose.ui.graphics.Color {
    val palette = LocalPalette.current
    val pressed by source.collectIsPressedAsState()
    val fill by animateColorAsState(
        if (pressed && enabled) palette.sel else palette.bg.copy(alpha = 0f),
        Motion.spec(Motion.HOVER),
        label = "row fill",
    )
    return fill
}

@Composable
private fun ProjectHeader(line: Line.ProjectHeader, modifier: Modifier, onToggle: () -> Unit) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    Row(
        modifier
            .fillMaxWidth()
            .background(pressFill(source))
            .testTag(line.key)
            .semantics { stateDescription = if (line.open) "Unfolded" else "Folded" }
            .clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onToggle)
            .padding(start = 20.dp, end = Space.Lg, top = 7.dp, bottom = 7.dp),
        horizontalArrangement = Arrangement.spacedBy(Space.Sm),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Chevron(line.open)
        Folder()
        Label(line.project, style = Type.Label.copy(fontWeight = FontWeight.Medium), color = palette.mute)
        Label("${line.count}", modifier = Modifier.weight(1f), style = Type.Small, color = palette.faint)
        // Folded, the project shows the most urgent of what it hides; open, its rows say it.
        AnimatedVisibility(
            !line.open && line.hidden != Mark.Rest,
            enter = fadeIn(Motion.spec(Motion.HOVER)),
            exit = fadeOut(Motion.spec(Motion.HOVER)),
        ) {
            MarkView(line.hidden, said = line.hidden.said, tag = "${line.key}:mark", ago = "")
        }
    }
}

/**
 * "Ended", pinned at the bottom of the tab: folded, it says how many; unfolded, its rows rise above
 * it, each saying its project and its machine, and it says once of each machine that is not online
 * that its ended sessions are not listed.
 */
@Composable
private fun EndedGroup(tree: Tree, open: Boolean, view: PhoneView, now: Long, onOpen: (String, String) -> Unit, onToggle: () -> Unit) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    val turn by animateFloatAsState(if (open) 90f else -90f, Motion.spec(Motion.PANEL), label = "ended chevron")
    Column(
        Modifier
            .fillMaxWidth()
            .drawBehind { drawLine(palette.line, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx()) }
            .background(palette.side),
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(pressFill(source))
                .testTag("ended")
                .semantics { stateDescription = if (open) "Unfolded" else "Folded" }
                .clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onToggle)
                .padding(horizontal = Space.Lg, vertical = Space.Md),
            horizontalArrangement = Arrangement.spacedBy(Space.Sm),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Label("Ended", style = Type.Label.copy(fontWeight = FontWeight.SemiBold), color = palette.mute)
            Label("${tree.endedCount}", modifier = Modifier.weight(1f), style = Type.Label, color = palette.faint)
            Box(Modifier.rotate(turn - 90f)) { Chevron(true) }
        }
        AnimatedVisibility(
            open,
            enter = fadeIn(Motion.spec(Motion.PANEL)) + expandVertically(Motion.spec(Motion.PANEL)),
            exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
        ) {
            LazyColumn(Modifier.fillMaxWidth().heightIn(max = 320.dp)) {
                items(tree.ended, key = { it.key }) { line ->
                    val move = Modifier.animateItem(
                        fadeInSpec = Motion.spec(Motion.ENTER),
                        placementSpec = Motion.spec(Motion.PANEL),
                        fadeOutSpec = Motion.spec(Motion.HOVER),
                    )
                    when (line) {
                        is Line.Session -> SessionLine(line, view, now, onOpen, onFold = {}, modifier = move)
                        is Line.Unlisted -> Row(
                            move.fillMaxWidth().testTag(line.key).padding(horizontal = Space.Lg, vertical = Space.Sm),
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            MachineIcon(line.member, offline = true)
                            Label(line.said, style = Type.Small, color = palette.faint)
                        }
                        else -> {}
                    }
                }
            }
        }
    }
}

@Composable
private fun SessionLine(
    line: Line.Session,
    view: PhoneView,
    now: Long,
    onOpen: (String, String) -> Unit,
    onFold: () -> Unit,
    modifier: Modifier,
) {
    val palette = LocalPalette.current
    val session = line.session
    val source = remember { MutableInteractionSource() }
    val ink by animateFloatAsState(if (line.offline) 0.45f else 1f, Motion.spec(Motion.PANEL), label = "row greyed")
    val pilot = line.leads.isNotEmpty()
    // A folded pilot shows the most urgent of its own state and its led sessions'.
    val mark = if (pilot && !line.open) strongest(listOf(session.mark()) + line.leads.map { it.mark() }) else session.mark()
    val connector = palette.line2
    val indent: Dp = if (line.led) 40.dp else 0.dp
    Box(
        modifier
            .fillMaxWidth()
            .drawBehind {
                if (!line.led) return@drawBehind
                // The line from the pilot down to each row it leads, as the window draws it.
                val x = 24.dp.toPx()
                val mid = size.height / 2
                val width = 1.5.dp.toPx()
                drawLine(connector, Offset(x, 0f), Offset(x, if (line.lastLed) mid else size.height), width)
                drawLine(connector, Offset(x, mid), Offset(x + 10.dp.toPx(), mid), width)
            },
    ) {
        Column(
            Modifier
                .padding(start = indent)
                .fillMaxWidth()
                .alpha(ink)
                .background(pressFill(source, enabled = !line.offline))
                .testTag(line.key)
                .semantics { if (line.offline) stateDescription = "Machine offline" }
                .clickable(
                    interactionSource = source,
                    indication = null,
                    enabled = !line.offline,
                    role = Role.Button,
                    onClick = { onOpen(session.machine, session.agent) },
                )
                .padding(start = if (line.led) 0.dp else Space.Lg, end = Space.Lg, top = 7.dp, bottom = 7.dp),
            verticalArrangement = Arrangement.spacedBy(1.dp),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(Space.Sm), verticalAlignment = Alignment.CenterVertically) {
                BasicText(
                    session.title.ifEmpty { "Untitled session" },
                    Modifier.weight(1f),
                    style = Type.Body.copy(color = palette.text, fontWeight = FontWeight.Medium, fontSize = 14.sp),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Crossfade(mark, animationSpec = Motion.spec(Motion.HOVER), label = "row mark") { shown ->
                    MarkView(shown, said = said(shown, view, session), tag = "mark:${session.machine}:${session.agent}", ago = ago(session.last, now))
                }
                if (pilot) FoldButton(session, line.open, onFold)
            }
            when {
                pilot -> BasicText(leadsLine(line.leads, palette.faint, palette.wait), style = Type.Small, maxLines = 1, overflow = TextOverflow.Ellipsis)
                line.project.isNotEmpty() -> Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
                    MachineIcon(line.member, line.offline)
                    Label(line.project, style = Type.Small, color = palette.faint)
                }
            }
        }
    }
}

/** What a pilot's row says of the sessions it leads: `leads 3 · 1 waiting`, the waiting ones in the colour of what needs the user. */
private fun leadsLine(leads: List<SessionRow>, faint: androidx.compose.ui.graphics.Color, wait: androidx.compose.ui.graphics.Color) =
    buildAnnotatedString {
        withStyle(SpanStyle(color = faint)) { append("leads ${leads.size}") }
        val waiting = leads.count { it.mark() == Mark.Needs }
        if (waiting > 0) {
            withStyle(SpanStyle(color = faint)) { append(" · ") }
            withStyle(SpanStyle(color = wait, fontWeight = FontWeight.Medium)) { append("$waiting waiting") }
        }
    }

private fun said(mark: Mark, view: PhoneView, session: SessionRow): String =
    when (mark) {
        Mark.Needs -> when {
            view.requests.any { it.machine == session.machine && it.agent == session.agent } -> "Needs you: a permission"
            view.asks.any { it.machine == session.machine && it.agent == session.agent } -> "Needs you: a question"
            else -> "Needs you"
        }
        else -> mark.said
    }

/** The mark of a state: a spinner for a session that works, a pulsing dot for one that needs the user, the time of the last exchange at rest. */
@Composable
private fun MarkView(mark: Mark, said: String, tag: String, ago: String) {
    val palette = LocalPalette.current
    val described = Modifier.testTag(tag).semantics { contentDescription = said }
    when (mark) {
        Mark.Running -> {
            val turn by rememberInfiniteTransition(label = "spinner").animateFloat(
                0f,
                360f,
                infiniteRepeatable(tween(900, easing = LinearEasing)),
                label = "spinner turn",
            )
            Box(described.size(18.dp), contentAlignment = Alignment.Center) {
                Canvas(Modifier.size(12.dp).rotate(turn)) {
                    drawArc(palette.run, 0f, 270f, false, style = Stroke(2.dp.toPx(), cap = StrokeCap.Round))
                }
            }
        }
        Mark.Needs -> {
            val pulse by rememberInfiniteTransition(label = "pulse").animateFloat(
                1f,
                0.7f,
                infiniteRepeatable(tween(800, easing = Motion.EaseOut), RepeatMode.Reverse),
                label = "pulse size",
            )
            Box(described.size(18.dp), contentAlignment = Alignment.Center) {
                Box(Modifier.size(9.dp).scale(pulse).alpha(0.55f + 0.45f * (pulse - 0.7f) / 0.3f).background(palette.wait, CircleShape))
            }
        }
        Mark.Ended -> Box(described.size(18.dp), contentAlignment = Alignment.Center) {
            Canvas(Modifier.size(9.dp)) { drawCircle(palette.faint, style = Stroke(1.5.dp.toPx())) }
        }
        Mark.Rest -> Box(described, contentAlignment = Alignment.Center) {
            Label(ago, style = Type.Small.copy(fontFamily = FontFamily.Default), color = palette.faint)
        }
    }
}

/** The button that folds or unfolds the sessions a pilot leads: its chevron turns over [Motion.PANEL]. */
@Composable
private fun FoldButton(session: SessionRow, open: Boolean, onFold: () -> Unit) {
    val source = remember { MutableInteractionSource() }
    Box(
        Modifier
            .size(28.dp)
            .background(pressFill(source), CircleShape)
            .testTag("fold:${session.machine}:${session.agent}")
            .semantics {
                contentDescription = if (open) "Hide the sessions it leads" else "Show the sessions it leads"
            }
            .clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onFold),
        contentAlignment = Alignment.Center,
    ) {
        Chevron(open)
    }
}

@Composable
private fun Chevron(open: Boolean) {
    val palette = LocalPalette.current
    val turn by animateFloatAsState(if (open) 90f else 0f, Motion.spec(Motion.PANEL), label = "chevron")
    Canvas(Modifier.size(13.dp).rotate(turn)) {
        val s = size.width / 24f
        val path = Path().apply {
            moveTo(9 * s, 6 * s)
            lineTo(15 * s, 12 * s)
            lineTo(9 * s, 18 * s)
        }
        drawPath(path, palette.faint, style = Stroke(1.8.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
    }
}

@Composable
private fun Folder() {
    val palette = LocalPalette.current
    Canvas(Modifier.size(13.dp)) {
        val s = size.width / 24f
        val path = Path().apply {
            moveTo(3 * s, 7 * s)
            quadraticTo(3 * s, 5 * s, 5 * s, 5 * s)
            lineTo(9 * s, 5 * s)
            lineTo(11 * s, 7 * s)
            lineTo(19 * s, 7 * s)
            quadraticTo(21 * s, 7 * s, 21 * s, 9 * s)
            lineTo(21 * s, 17 * s)
            quadraticTo(21 * s, 19 * s, 19 * s, 19 * s)
            lineTo(5 * s, 19 * s)
            quadraticTo(3 * s, 19 * s, 3 * s, 17 * s)
            close()
        }
        drawPath(path, palette.mute, style = Stroke(1.8.dp.toPx(), join = StrokeJoin.Round))
    }
}
