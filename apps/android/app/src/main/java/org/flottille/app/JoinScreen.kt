package org.flottille.app

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.Choice
import org.flottille.app.ui.Entrance
import org.flottille.app.ui.FButton
import org.flottille.app.ui.Field
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Message
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Type
import org.flottille.core.HubCard
import org.flottille.core.Phone
import org.flottille.core.PhoneException

/** Where the user is in joining: reading the code, confirming the card, told the result. */
private sealed interface Step {
    data object Scan : Step
    data class Card(val code: String, val card: HubCard) : Step
    data class Done(val role: String) : Step
}

/**
 * Connect to your hub (`docs/remote.md`, "Pairing"): the QR of `flottille-hub pair` is read (or its
 * code pasted), the hub's card is shown (its address and its key's fingerprint, which the phone pins),
 * the user confirms this phone's name and role, and the role the hub gave is said.
 */
@Composable
fun JoinScreen(
    phone: Phone,
    said: String?,
    deviceName: String,
    scanner: @Composable (onCode: (String) -> Unit) -> Unit,
    onContinue: () -> Unit,
) {
    var step by remember { mutableStateOf<Step>(Step.Scan) }
    AnimatedContent(
        targetState = step,
        modifier = Modifier.fillMaxWidth(),
        contentKey = { it::class },
        transitionSpec = {
            (fadeIn(Motion.spec(Motion.ENTER)) + slideInVertically(Motion.spec(Motion.ENTER)) { it / 24 }) togetherWith
                fadeOut(Motion.spec(Motion.HOVER))
        },
        label = "join step",
    ) { current ->
        Column(
            Modifier.verticalScroll(rememberScrollState()).padding(Space.Xl),
            verticalArrangement = Arrangement.spacedBy(Space.Lg),
        ) {
            Entrance(0) { Label("Connect to your hub", style = Type.Title) }
            when (current) {
                is Step.Scan -> ScanStep(phone, said, scanner) { code, card -> step = Step.Card(code, card) }
                is Step.Card -> CardStep(phone, current, deviceName, back = { step = Step.Scan }) { role -> step = Step.Done(role) }
                is Step.Done -> DoneStep(current.role, onContinue)
            }
        }
    }
}

@Composable
private fun ScanStep(
    phone: Phone,
    said: String?,
    scanner: @Composable (onCode: (String) -> Unit) -> Unit,
    found: (code: String, card: HubCard) -> Unit,
) {
    val palette = LocalPalette.current
    var problem by remember { mutableStateOf<String?>(null) }
    var pasted by remember { mutableStateOf("") }
    // The scanner reads the same QR many times a second: the first reading counts.
    var read by remember { mutableStateOf(false) }
    fun take(code: String) {
        if (read) return
        try {
            val card = phone.card(code)
            read = true
            problem = null
            found(code.trim(), card)
        } catch (refused: PhoneException) {
            problem = refused.why
        }
    }
    Entrance(1) {
        Label(
            "On your hub's host, run flottille-hub pair, then scan the code it shows, or paste it.",
            color = palette.mute,
        )
    }
    Message(said, tag = "said")
    Entrance(2) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(260.dp)
                .clip(RoundedCornerShape(16.dp))
                .background(palette.card)
                .border(1.dp, palette.line, RoundedCornerShape(16.dp)),
            contentAlignment = androidx.compose.ui.Alignment.Center,
        ) {
            scanner(::take)
        }
    }
    Entrance(3) {
        Column(verticalArrangement = Arrangement.spacedBy(Space.Sm)) {
            Field("Pair code", pasted, { pasted = it }, tag = "code-field", hidden = true)
            FButton("Use this code", { take(pasted) }, enabled = pasted.isNotBlank())
        }
    }
    Message(problem, tag = "problem")
}

@Composable
private fun CardStep(phone: Phone, step: Step.Card, deviceName: String, back: () -> Unit, joined: (role: String) -> Unit) {
    val palette = LocalPalette.current
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf(deviceName) }
    var role by remember { mutableStateOf("controller") }
    var joining by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    Entrance(1) {
        Column(
            Modifier
                .fillMaxWidth()
                .background(palette.card, RoundedCornerShape(16.dp))
                .border(1.dp, palette.line, RoundedCornerShape(16.dp))
                .padding(Space.Lg),
            verticalArrangement = Arrangement.spacedBy(Space.Md),
        ) {
            Label("Your hub", style = Type.Heading, color = palette.mute)
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Label("Address", style = Type.Small, color = palette.faint)
                Label(step.card.url)
            }
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Label("Fingerprint", style = Type.Small, color = palette.faint)
                Label(step.card.fingerprint)
            }
        }
    }
    Entrance(2) {
        Field("This device is", name, { name = it }, tag = "device-name")
    }
    Entrance(3) {
        Column(verticalArrangement = Arrangement.spacedBy(Space.Sm)) {
            Label("This device can", style = Type.Label, color = palette.mute)
            Row(horizontalArrangement = Arrangement.spacedBy(Space.Sm)) {
                Choice("Control", role == "controller", { role = "controller" })
                Choice("View only", role == "viewer", { role = "viewer" })
            }
        }
    }
    Message(problem, tag = "problem")
    Entrance(4) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.Sm, androidx.compose.ui.Alignment.End)) {
            FButton("Back", back, kind = ButtonKind.Ghost, enabled = !joining)
            FButton(
                if (joining) "Joining…" else "Join",
                {
                    joining = true
                    problem = null
                    scope.launch {
                        try {
                            phone.join(step.code, name.trim(), role)
                            joined(phone.view.role.ifEmpty { role })
                        } catch (refused: PhoneException) {
                            problem = refused.why
                        } finally {
                            joining = false
                        }
                    }
                },
                kind = ButtonKind.Primary,
                enabled = name.isNotBlank() && !joining,
            )
        }
    }
}

@Composable
private fun DoneStep(role: String, onContinue: () -> Unit) {
    val palette = LocalPalette.current
    val meaning = if (role == "viewer") "it can look, and answer nothing" else "it can answer requests and control sessions"
    Entrance(1) {
        Column(
            Modifier
                .fillMaxWidth()
                .background(palette.okBg, RoundedCornerShape(16.dp))
                .padding(Space.Lg)
                .testTag("joined"),
            verticalArrangement = Arrangement.spacedBy(Space.Sm),
        ) {
            Label("Joined", style = Type.Heading, color = palette.ok)
            Label("Your hub made this phone a $role: $meaning.")
        }
    }
    Entrance(2) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            FButton("Continue", onContinue, kind = ButtonKind.Primary)
        }
    }
}
