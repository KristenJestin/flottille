package org.flottille.app

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import java.time.ZoneId
import org.flottille.app.ui.Entrance
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Type
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.PhoneView

/**
 * The hub, reachable or not, and each machine with its state: online, offline since when (and the
 * address that was tried), or needs an update, which is never said offline.
 */
@Composable
fun MachinesScreen(view: PhoneView, zone: ZoneId) {
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = Space.Lg, vertical = Space.Sm),
        verticalArrangement = Arrangement.spacedBy(Space.Sm),
    ) {
        Entrance(0) { HubCard(view.hub) }
        view.machines.forEachIndexed { index, machine ->
            Entrance(index + 1) { MachineCard(machine, zone) }
        }
        if (view.machines.isEmpty()) {
            Entrance(1) {
                Label("No machine yet. A machine of yours that runs agents shows here once it joins your hub.", color = LocalPalette.current.mute)
            }
        }
    }
}

@Composable
private fun Card(tag: String, content: @Composable () -> Unit) {
    val palette = LocalPalette.current
    Column(
        Modifier
            .fillMaxWidth()
            .background(palette.card, RoundedCornerShape(12.dp))
            .border(1.dp, palette.line, RoundedCornerShape(12.dp))
            .padding(Space.Md)
            .semantics(mergeDescendants = true) {}
            .testTag(tag),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) { content() }
}

@Composable
private fun HubCard(hub: String) {
    val palette = LocalPalette.current
    val ink by animateColorAsState(if (hub.isEmpty()) palette.ok else palette.fail, Motion.spec(Motion.PANEL), label = "hub state")
    Card("hub") {
        Label("Your hub", style = Type.Heading, color = palette.mute)
        Label(if (hub.isEmpty()) "Reachable" else hub, color = ink)
    }
}

@Composable
private fun MachineCard(machine: Machine, zone: ZoneId) {
    val palette = LocalPalette.current
    val ink by animateColorAsState(
        when (machine.state) {
            MachineState.Online -> palette.ok
            MachineState.Offline -> palette.fail
            MachineState.NeedsUpdate -> palette.wait
        },
        Motion.spec(Motion.PANEL),
        label = "machine state",
    )
    Card("machine:${machine.name}") {
        Label(machine.name)
        Label(stateLine(machine, zone), style = Type.Label, color = ink)
        if (machine.state != MachineState.Online && machine.why.isNotEmpty()) {
            Label(machine.why, style = Type.Small, color = palette.mute)
        }
    }
}
