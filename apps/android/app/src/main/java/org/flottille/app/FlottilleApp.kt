package org.flottille.app

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import java.time.ZoneId
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.FButton
import org.flottille.app.ui.FlottilleTheme
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Message
import org.flottille.app.ui.Motion
import org.flottille.app.ui.Space
import org.flottille.app.ui.Type
import org.flottille.core.Phone
import org.flottille.core.PhoneException
import org.flottille.core.Picture

private enum class Tab(val label: String) {
    Sessions("Sessions"),
    Machines("Machines"),
}

/**
 * The app (`docs/remote.md`, "The Android app"). A phone that joined no hub is asked to connect to
 * one; one that did lists its sessions (one list, every machine) and its machines, and has
 * Settings. The view is the core's, followed while the app is on screen; the screens compute
 * nothing of their own. Tapping a session opens its page (`SessionPage`); Needs you and the
 * notifications come with the issues after #826.
 */
@Composable
fun FlottilleApp(
    phone: Phone,
    deviceName: String = "phone",
    zone: ZoneId = ZoneId.systemDefault(),
    now: () -> Long = { System.currentTimeMillis() },
    scanner: @Composable (onCode: (String) -> Unit) -> Unit = { CameraScanner(it) },
    onOpenSession: (machine: String, agent: String) -> Unit = { _, _ -> },
    pictures: PictureSource = PictureSource.None,
    shared: List<Picture> = emptyList(),
    onSharedPlaced: () -> Unit = {},
    open: SessionKey? = null,
) {
    FlottilleTheme {
        val palette = LocalPalette.current
        val lifecycle = LocalLifecycleOwner.current.lifecycle
        var view by remember { mutableStateOf(phone.view) }
        // Joining ends when the user says so (the result is shown first), and starts again when the
        // phone is no member: it was removed.
        var joining by rememberSaveable { mutableStateOf(!phone.view.joined) }
        var tab by rememberSaveable { mutableStateOf(Tab.Sessions) }
        var settings by rememberSaveable { mutableStateOf(false) }
        var toast by remember { mutableStateOf<String?>(null) }
        // The session page on screen: one entry point, by machine and session id, for the list and
        // for a notification that opens it.
        var page by rememberSaveable(stateSaver = SessionKey.Saver) { mutableStateOf(open) }
        var shownPage by remember { mutableStateOf(page) }
        if (page != null) shownPage = page
        LaunchedEffect(open) { if (open != null) page = open }
        // Pictures shared to the app wait for the user to say where they go: never sent unasked.
        var carried by remember(shared) { mutableStateOf(shared) }
        val scope = rememberCoroutineScope()

        // Connected while the app is on screen; the screens draw what the core says.
        LaunchedEffect(phone, view.joined) {
            lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
                phone.views().collect { view = it }
            }
        }
        LaunchedEffect(view.joined) {
            if (!view.joined) {
                joining = true
                settings = false
            }
        }
        LaunchedEffect(toast) {
            if (toast != null) {
                delay(TOAST_MILLIS)
                toast = null
            }
        }

        Box(Modifier.fillMaxSize().background(palette.bg).windowInsetsPadding(WindowInsets.safeDrawing)) {
            if (joining) {
                JoinScreen(
                    phone = phone,
                    said = view.said.lastOrNull(),
                    deviceName = deviceName,
                    scanner = scanner,
                    onContinue = {
                        view = phone.view
                        joining = false
                    },
                )
            } else {
                Main(
                    view = view,
                    zone = zone,
                    tab = tab,
                    onTab = { tab = it },
                    onSettings = { settings = true },
                    onOpenSession = { machine, agent ->
                        onOpenSession(machine, agent)
                        page = SessionKey(machine, agent)
                    },
                    sharing = carried.isNotEmpty(),
                    onDiscard = {
                        carried = emptyList()
                        onSharedPlaced()
                    },
                )
                AnimatedVisibility(
                    visible = page != null,
                    enter = slideInHorizontally(Motion.spec(Motion.PANEL)) { it / 4 } + fadeIn(Motion.spec(Motion.PANEL)),
                    exit = slideOutHorizontally(Motion.spec(Motion.PANEL)) { it / 4 } + fadeOut(Motion.spec(Motion.PANEL)),
                ) {
                    BackHandler { page = null }
                    shownPage?.let { key ->
                        SessionPage(
                            phone = phone,
                            view = view,
                            key = key,
                            zone = zone,
                            pictures = pictures,
                            carried = carried,
                            onCarried = {
                                carried = emptyList()
                                onSharedPlaced()
                            },
                            onToast = { toast = it },
                            onBack = { page = null },
                        )
                    }
                }
                AnimatedVisibility(
                    visible = settings,
                    enter = slideInHorizontally(Motion.spec(Motion.PANEL)) { it / 4 } + fadeIn(Motion.spec(Motion.PANEL)),
                    exit = slideOutHorizontally(Motion.spec(Motion.PANEL)) { it / 4 } + fadeOut(Motion.spec(Motion.PANEL)),
                ) {
                    BackHandler { settings = false }
                    SettingsScreen(
                        view = view,
                        onBack = { settings = false },
                        onRemove = {
                            scope.launch {
                                try {
                                    phone.leave()
                                    view = phone.view
                                } catch (refused: PhoneException) {
                                    toast = refused.why
                                }
                            }
                        },
                    )
                }
            }
            // A result, as a toast: it comes in and goes out, and goes by itself.
            Message(
                toast,
                modifier = Modifier.align(Alignment.BottomCenter).padding(Space.Lg),
                tag = "toast",
            )
        }
    }
}

private const val TOAST_MILLIS = 5_000L

@Composable
private fun Main(
    view: org.flottille.core.PhoneView,
    zone: ZoneId,
    tab: Tab,
    onTab: (Tab) -> Unit,
    onSettings: () -> Unit,
    onOpenSession: (String, String) -> Unit,
    sharing: Boolean,
    onDiscard: () -> Unit,
) {
    val palette = LocalPalette.current
    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = Space.Lg, vertical = Space.Sm),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Label(tab.label, style = Type.Title, modifier = Modifier.weight(1f))
            FButton("Settings", onSettings, kind = ButtonKind.Ghost, modifier = Modifier.testTag("open-settings"))
        }
        // A picture shared to the app waits for the session it goes to.
        Row(
            Modifier.fillMaxWidth().padding(horizontal = Space.Lg),
            horizontalArrangement = Arrangement.spacedBy(Space.Sm),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Message(
                if (sharing) "Pick a session for the picture" else null,
                modifier = Modifier.weight(1f),
                tag = "share-banner",
                failed = false,
            )
            AnimatedVisibility(sharing, enter = fadeIn(Motion.spec(Motion.ENTER)), exit = fadeOut(Motion.spec(Motion.HOVER))) {
                FButton("Discard", onDiscard, kind = ButtonKind.Ghost, modifier = Modifier.testTag("share-discard"))
            }
        }
        Box(Modifier.weight(1f)) {
            AnimatedContent(
                targetState = tab,
                transitionSpec = { fadeIn(Motion.spec(Motion.ENTER)) togetherWith fadeOut(Motion.spec(Motion.HOVER)) },
                label = "tab",
            ) { shown ->
                when (shown) {
                    Tab.Sessions -> SessionsScreen(view, onOpenSession)
                    Tab.Machines -> MachinesScreen(view, zone)
                }
            }
        }
        Row(
            Modifier.fillMaxWidth().background(palette.side).padding(Space.Xs),
            horizontalArrangement = Arrangement.SpaceEvenly,
        ) {
            Tab.entries.forEach { entry ->
                TabButton(entry.label, selected = entry == tab, tag = "tab:${entry.name.lowercase()}", modifier = Modifier.weight(1f)) {
                    onTab(entry)
                }
            }
        }
    }
}

@Composable
private fun TabButton(text: String, selected: Boolean, tag: String, modifier: Modifier, onClick: () -> Unit) {
    val palette = LocalPalette.current
    val fill by animateColorAsState(if (selected) palette.sel else palette.side, Motion.spec(Motion.PANEL), label = "tab fill")
    val ink by animateColorAsState(if (selected) palette.text else palette.mute, Motion.spec(Motion.PANEL), label = "tab ink")
    Box(
        modifier
            .padding(Space.Xs)
            .background(fill, RoundedCornerShape(10.dp))
            .testTag(tag)
            .semantics { this.selected = selected }
            .clickable(role = Role.Tab, indication = null, interactionSource = remember { MutableInteractionSource() }, onClick = onClick)
            .padding(vertical = Space.Md),
        contentAlignment = Alignment.Center,
    ) {
        Label(text, color = ink)
    }
}
