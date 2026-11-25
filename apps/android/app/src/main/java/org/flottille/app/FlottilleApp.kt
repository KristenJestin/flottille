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
import androidx.compose.foundation.Canvas
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
import androidx.compose.foundation.layout.size
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
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
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

/** The bottom bar's destinations (`docs/remote.md`, "The Android app"); the app opens on the first. */
private enum class Tab(val label: String, val tag: String) {
    NeedsYou("Needs you", "needs-you"),
    Sessions("Sessions", "sessions"),
    Machines("Machines", "machines"),
}

/**
 * The app (`docs/remote.md`, "The Android app"). A phone that joined no hub is asked to connect to
 * one; one that did opens on Needs you (what waits for the user on every machine), and has its
 * sessions (one list, every machine), its machines and Settings. The view is the core's, followed
 * while the app is on screen; the screens compute nothing of their own. Tapping a session or a
 * card opens its page (`SessionPage`); the notifications come with the issues after #826.
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
    sharedNote: String? = null,
    onSharedPlaced: () -> Unit = {},
    open: SessionKey? = null,
    tunnels: TunnelHost = TunnelHost.None,
    background: Background = Background.None,
) {
    FlottilleTheme {
        val palette = LocalPalette.current
        val lifecycle = LocalLifecycleOwner.current.lifecycle
        var view by remember { mutableStateOf(phone.view) }
        // Joining ends when the user says so (the result is shown first), and starts again when the
        // phone is no member: it was removed.
        var joining by rememberSaveable { mutableStateOf(!phone.view.joined) }
        var tab by rememberSaveable { mutableStateOf(Tab.NeedsYou) }
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
        // What of a share could not be taken is said once, as the other results are.
        LaunchedEffect(sharedNote) { if (sharedNote != null) toast = sharedNote }
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
                    phone = phone,
                    view = view,
                    zone = zone,
                    now = now,
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
                    onToast = { toast = it },
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
                            tunnels = tunnels,
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
                        background = background,
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
    phone: Phone,
    view: org.flottille.core.PhoneView,
    zone: ZoneId,
    now: () -> Long,
    tab: Tab,
    onTab: (Tab) -> Unit,
    onSettings: () -> Unit,
    onOpenSession: (String, String) -> Unit,
    sharing: Boolean,
    onDiscard: () -> Unit,
    onToast: (String) -> Unit,
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
                    Tab.NeedsYou -> NeedsYouScreen(phone, view, onOpenSession, onToast)
                    Tab.Sessions -> SessionsScreen(view, zone, now, onOpenSession)
                    Tab.Machines -> MachinesScreen(view, zone)
                }
            }
        }
        Row(
            Modifier.fillMaxWidth().background(palette.side).padding(Space.Xs),
            horizontalArrangement = Arrangement.SpaceEvenly,
        ) {
            Tab.entries.forEach { entry ->
                TabButton(entry, selected = entry == tab, modifier = Modifier.weight(1f)) {
                    onTab(entry)
                }
            }
        }
    }
}

@Composable
private fun TabButton(entry: Tab, selected: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val palette = LocalPalette.current
    val fill by animateColorAsState(if (selected) palette.sel else palette.side, Motion.spec(Motion.PANEL), label = "tab fill")
    val ink by animateColorAsState(if (selected) palette.text else palette.mute, Motion.spec(Motion.PANEL), label = "tab ink")
    Box(
        modifier
            .padding(Space.Xs)
            .background(fill, RoundedCornerShape(10.dp))
            .testTag("tab:${entry.tag}")
            .semantics { this.selected = selected }
            .clickable(role = Role.Tab, indication = null, interactionSource = remember { MutableInteractionSource() }, onClick = onClick)
            .padding(vertical = Space.Sm),
        contentAlignment = Alignment.Center,
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Space.Xs)) {
            TabIcon(entry, ink, Modifier.size(20.dp).testTag("tab-icon:${entry.tag}"))
            Label(entry.label, style = Type.Label, color = ink)
        }
    }
}

/**
 * A destination's icon, drawn in its ink (which moves with the selection): a bell for Needs you,
 * lines of a list for Sessions, a screen on its stand for Machines.
 */
@Composable
private fun TabIcon(entry: Tab, ink: Color, modifier: Modifier) {
    Canvas(modifier) {
        val w = size.width
        val h = size.height
        val stroke = Stroke(width = w * 0.09f, cap = StrokeCap.Round, join = StrokeJoin.Round)
        when (entry) {
            Tab.NeedsYou -> {
                val bell = Path().apply {
                    moveTo(w * 0.18f, h * 0.74f)
                    lineTo(w * 0.82f, h * 0.74f)
                    cubicTo(w * 0.72f, h * 0.62f, w * 0.74f, h * 0.5f, w * 0.74f, h * 0.42f)
                    cubicTo(w * 0.74f, h * 0.24f, w * 0.62f, h * 0.14f, w * 0.5f, h * 0.14f)
                    cubicTo(w * 0.38f, h * 0.14f, w * 0.26f, h * 0.24f, w * 0.26f, h * 0.42f)
                    cubicTo(w * 0.26f, h * 0.5f, w * 0.28f, h * 0.62f, w * 0.18f, h * 0.74f)
                    close()
                }
                drawPath(bell, ink, style = stroke)
                drawLine(ink, Offset(w * 0.42f, h * 0.88f), Offset(w * 0.58f, h * 0.88f), stroke.width, StrokeCap.Round)
            }
            Tab.Sessions -> listOf(0.25f, 0.5f, 0.75f).forEach { y ->
                drawCircle(ink, w * 0.06f, Offset(w * 0.16f, h * y))
                drawLine(ink, Offset(w * 0.34f, h * y), Offset(w * 0.86f, h * y), stroke.width, StrokeCap.Round)
            }
            Tab.Machines -> {
                drawRoundRect(ink, Offset(w * 0.12f, h * 0.16f), Size(w * 0.76f, h * 0.52f), CornerRadius(w * 0.08f), style = stroke)
                drawLine(ink, Offset(w * 0.5f, h * 0.68f), Offset(w * 0.5f, h * 0.84f), stroke.width, StrokeCap.Round)
                drawLine(ink, Offset(w * 0.32f, h * 0.86f), Offset(w * 0.68f, h * 0.86f), stroke.width, StrokeCap.Round)
            }
        }
    }
}
