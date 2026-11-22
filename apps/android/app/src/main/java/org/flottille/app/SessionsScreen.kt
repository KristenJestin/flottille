package org.flottille.app

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import org.flottille.app.ui.Entrance
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Tag
import org.flottille.app.ui.Type
import org.flottille.core.MachineState
import org.flottille.core.PhoneView
import org.flottille.core.SessionRow

/** What the list is made of, in the order it is drawn. */
private sealed interface Line {
    val key: String

    data class Header(val project: String) : Line {
        override val key get() = "project:$project"
    }

    data class Row(val session: SessionRow, val offline: Boolean) : Line {
        override val key get() = "session:${session.machine}:${session.agent}"
    }
}

/**
 * The sessions of every machine in one list (#1150): grouped by project (the core orders the
 * groups by name, so they never move), the machine a tag on each row, in a project the ones that wait
 * for the user first and the ended ones last, then the most recent. A session of a machine that is
 * offline is greyed and is not opened.
 */
@Composable
fun SessionsScreen(view: PhoneView, onOpen: (machine: String, agent: String) -> Unit) {
    val online = view.machines.filter { it.state == MachineState.Online }.map { it.name }.toSet()
    val lines = view.projects.flatMap { project ->
        val ordered = project.sessions.sortedWith(
            compareBy<SessionRow> { it.state == "ended" }.thenBy { !it.waiting }.thenByDescending { it.last },
        )
        listOf<Line>(Line.Header(project.name)) + ordered.map { Line.Row(it, offline = it.machine !in online) }
    }
    Entrance(0, Modifier.fillMaxSize()) {
        if (lines.isEmpty()) {
            Column(Modifier.fillMaxSize().padding(Space.Xl), verticalArrangement = Arrangement.spacedBy(Space.Sm)) {
                Label("No sessions yet", style = Type.Heading)
                Label("The sessions of your machines show here as they start.", color = LocalPalette.current.mute)
            }
        } else {
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(horizontal = Space.Lg, vertical = Space.Sm)) {
                items(lines, key = { it.key }) { line ->
                    // A row inserted, removed or moved unfolds, folds away or slides, never jumps.
                    val move = Modifier.animateItem(
                        fadeInSpec = Motion.spec(Motion.ENTER),
                        placementSpec = Motion.spec(Motion.PANEL),
                        fadeOutSpec = Motion.spec(Motion.HOVER),
                    )
                    when (line) {
                        is Line.Header -> Header(line.project, move)
                        is Line.Row -> SessionRowView(line.session, line.offline, onOpen, move)
                    }
                }
            }
        }
    }
}

@Composable
private fun Header(project: String, modifier: Modifier) {
    Label(
        project.uppercase(),
        modifier = modifier.testTag("project:$project").padding(top = Space.Lg, bottom = Space.Xs, start = Space.Xs),
        style = Type.Heading,
        color = LocalPalette.current.faint,
    )
}

@Composable
private fun SessionRowView(session: SessionRow, offline: Boolean, onOpen: (String, String) -> Unit, modifier: Modifier) {
    val palette = LocalPalette.current
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val fill by animateColorAsState(if (pressed && !offline) palette.hover else palette.card, Motion.spec(Motion.HOVER), label = "row fill")
    val ring by animateColorAsState(if (session.waiting) palette.wait else palette.line, Motion.spec(Motion.PANEL), label = "row ring")
    val ink by animateFloatAsState(if (offline) 0.45f else 1f, Motion.spec(Motion.PANEL), label = "row greyed")
    Row(
        modifier
            .padding(vertical = Space.Xs)
            .fillMaxWidth()
            .alpha(ink)
            .background(fill, RoundedCornerShape(12.dp))
            .border(1.dp, ring, RoundedCornerShape(12.dp))
            .testTag("session:${session.machine}:${session.agent}")
            .semantics { if (offline) stateDescription = "Machine offline" }
            .clickable(
                interactionSource = source,
                indication = null,
                enabled = !offline,
                role = Role.Button,
                onClick = { onOpen(session.machine, session.agent) },
            )
            .padding(Space.Md),
        horizontalArrangement = Arrangement.spacedBy(Space.Md),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Label(session.title)
            Label(
                sessionState(session.state, session.waiting),
                style = Type.Small,
                color = if (session.waiting) palette.wait else palette.mute,
            )
        }
        Tag(session.machine)
    }
}
