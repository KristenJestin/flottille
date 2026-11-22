package org.flottille.app

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.Confirm
import org.flottille.app.ui.Entrance
import org.flottille.app.ui.FButton
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Space
import org.flottille.app.ui.Tag
import org.flottille.app.ui.Type
import org.flottille.core.PhoneView

/**
 * This device's name and role, the members, and Remove this device. There is no Add a device: a
 * device is paired on the hub only (`docs/remote.md`, "Pairing"). Removing asks once.
 */
@Composable
fun SettingsScreen(view: PhoneView, onBack: () -> Unit, onRemove: () -> Unit) {
    val palette = LocalPalette.current
    var asking by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxSize().background(palette.bg)) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(Space.Lg),
            verticalArrangement = Arrangement.spacedBy(Space.Lg),
        ) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                FButton("Back", onBack, kind = ButtonKind.Ghost)
                Label("Settings", style = Type.Title, modifier = Modifier.padding(start = Space.Sm))
            }
            Entrance(0) {
                Column(
                    Modifier
                        .fillMaxWidth()
                        .background(palette.card, RoundedCornerShape(12.dp))
                        .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                        .padding(Space.Md)
                        .semantics(mergeDescendants = true) {}
                        .testTag("this-device"),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    Label("This device", style = Type.Heading, color = palette.mute)
                    Label(view.name)
                    Label("Role: ${view.role}", style = Type.Label, color = palette.mute)
                }
            }
            Entrance(1) {
                Column(verticalArrangement = Arrangement.spacedBy(Space.Sm)) {
                    Label("Devices of your hub", style = Type.Heading, color = palette.mute)
                    view.members.forEach { member ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .background(palette.card, RoundedCornerShape(12.dp))
                                .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                                .padding(Space.Md)
                                .semantics(mergeDescendants = true) {}
                                .testTag("member:${member.name}"),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(Space.Md),
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Label(member.name)
                                Label(
                                    buildString {
                                        append(member.role)
                                        if (member.addedBy.isNotEmpty()) append(" · added by ${member.addedBy}")
                                    },
                                    style = Type.Small,
                                    color = palette.mute,
                                )
                            }
                            if (member.you) Tag("this device")
                        }
                    }
                }
            }
            Entrance(2) {
                Column(verticalArrangement = Arrangement.spacedBy(Space.Sm)) {
                    Label(
                        "A device is added on your hub's host, with flottille-hub pair. Removing this phone makes your hub forget it.",
                        style = Type.Label,
                        color = palette.mute,
                    )
                    FButton("Remove this device", { asking = true }, kind = ButtonKind.Danger)
                }
            }
        }
        Confirm(
            visible = asking,
            title = "Remove this device?",
            body = "${view.name} leaves your hub's list of devices. To join again, pair it on the hub.",
            confirm = "Remove",
            onConfirm = {
                asking = false
                onRemove()
            },
            onCancel = { asking = false },
        )
    }
}
