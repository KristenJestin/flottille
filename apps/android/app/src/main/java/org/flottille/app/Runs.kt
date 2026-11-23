package org.flottille.app

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.FButton
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Type
import org.flottille.core.Run
import org.flottille.core.Tunnel

/**
 * What a page can do with an address of a machine (`docs/remote.md`, "Tunnels", "On the phone"):
 * [open] reads it in the app or hands it to the user's browser, through a tunnel when it is the
 * machine's own; [close] ends a tunnel the phone holds. [busy] lists what is being opened, so that
 * a second tap waits; a `viewer` has no tunnel to open (the machine refuses it), so its page
 * draws none.
 */
class Opening(
    val readOnly: Boolean,
    val busy: Set<String>,
    val isLocal: (String) -> Boolean,
    val open: (slot: String, run: String?, url: String, inBrowser: Boolean) -> Unit,
    val close: (tunnel: String) -> Unit,
)

/**
 * The commands of the session that run and report an address, each with **Open** (in the app) and
 * **Open in browser**, and the tunnel the phone holds to it, with Close. A row unfolds and fades in
 * when the run starts and folds away when it ends; the tunnel's line unfolds when it opens.
 */
@Composable
fun RunsSection(runs: List<Run>, tunnels: List<Tunnel>, opening: Opening) {
    val palette = LocalPalette.current
    if (runs.isEmpty() || opening.readOnly) return
    Column(Modifier.fillMaxWidth().padding(horizontal = Space.Lg, vertical = Space.Xs)) {
        Label("RUNS", style = Type.Heading, color = palette.faint)
        LazyColumn(Modifier.heightIn(max = 220.dp).testTag("runs"), verticalArrangement = Arrangement.spacedBy(Space.Xs)) {
            items(runs, key = { "run:${it.run}" }) { run ->
                RunRow(
                    run = run,
                    tunnel = tunnels.firstOrNull { it.run == run.run },
                    opening = opening,
                    modifier = Modifier.animateItem(
                        fadeInSpec = Motion.spec(Motion.ENTER),
                        placementSpec = Motion.spec(Motion.PANEL),
                        fadeOutSpec = Motion.spec(Motion.HOVER),
                    ),
                )
            }
        }
    }
}

@Composable
private fun RunRow(run: Run, tunnel: Tunnel?, opening: Opening, modifier: Modifier) {
    val palette = LocalPalette.current
    val slot = "run:${run.run}"
    Column(
        modifier
            .fillMaxWidth()
            .background(palette.card, RoundedCornerShape(12.dp))
            .border(BorderStroke(1.dp, palette.line), RoundedCornerShape(12.dp))
            .testTag("run:${run.run}")
            .padding(Space.Md),
        verticalArrangement = Arrangement.spacedBy(Space.Xs),
    ) {
        Label(run.line.ifEmpty { run.run }, style = Type.Label)
        Label(run.url, style = Type.Small, color = palette.mute)
        TunnelLine(tunnel, opening, Modifier.testTag("tunnel:${run.run}"))
        OpenButtons(slot, run.run, run.url, opening, "run-open:${run.run}", "run-browser:${run.run}")
    }
}

/** The tunnel held to a run, `localhost:5173 → server` with Close: it unfolds in and folds away. */
@Composable
private fun TunnelLine(tunnel: Tunnel?, opening: Opening, modifier: Modifier = Modifier) {
    val palette = LocalPalette.current
    AnimatedVisibility(
        visible = tunnel != null,
        modifier = modifier,
        enter = fadeIn(Motion.spec(Motion.ENTER)) + expandVertically(Motion.spec(Motion.ENTER)),
        exit = fadeOut(Motion.spec(Motion.HOVER)) + shrinkVertically(Motion.spec(Motion.PANEL)),
    ) {
        // The last tunnel stays while the line folds away.
        var shown by remember { mutableStateOf(tunnel) }
        if (tunnel != null) shown = tunnel
        shown?.let { held ->
            Row(horizontalArrangement = Arrangement.spacedBy(Space.Sm), verticalAlignment = Alignment.CenterVertically) {
                Label(held.said(), style = Type.Small, color = palette.run, modifier = Modifier.weight(1f))
                FButton("Close", { opening.close(held.tunnel) }, kind = ButtonKind.Ghost, modifier = Modifier.testTag("tunnel-close:${held.run}"))
            }
        }
    }
}

/** **Open** and **Open in browser**; a page of the web has **Open** only, and a viewer's has neither tunnel. */
@Composable
private fun OpenButtons(slot: String, run: String?, url: String, opening: Opening, openTag: String, browserTag: String) {
    val local = opening.isLocal(url)
    if (opening.readOnly && local) return
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.Sm, Alignment.End)) {
        if (local) {
            FButton("Open in browser", { opening.open(slot, run, url, true) }, kind = ButtonKind.Ghost, enabled = slot !in opening.busy, modifier = Modifier.testTag(browserTag))
        }
        FButton("Open", { opening.open(slot, run, url, false) }, kind = ButtonKind.Secondary, enabled = slot !in opening.busy, modifier = Modifier.testTag(openTag))
    }
}

/** A link an agent showed: its title and address, with the same two buttons as a run's. */
@Composable
fun LinkCard(seq: Long, title: String, url: String, opening: Opening) {
    val palette = LocalPalette.current
    Column(
        Modifier
            .fillMaxWidth()
            .background(palette.card, RoundedCornerShape(12.dp))
            .border(BorderStroke(1.dp, palette.line), RoundedCornerShape(12.dp))
            .testTag("link:$seq")
            .padding(Space.Md),
        verticalArrangement = Arrangement.spacedBy(Space.Xs),
    ) {
        Label(title.ifEmpty { url }, style = Type.Label)
        Label(url, style = Type.Small, color = palette.mute)
        OpenButtons("link:$seq", null, url, opening, "link-open:$seq", "link-browser:$seq")
    }
}
