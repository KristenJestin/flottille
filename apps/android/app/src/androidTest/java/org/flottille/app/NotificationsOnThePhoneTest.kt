package org.flottille.app

import android.Manifest
import android.app.ActivityManager
import android.app.Notification
import android.app.NotificationManager
import android.content.Context
import android.service.notification.StatusBarNotification
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import org.flottille.core.Ask
import org.flottille.core.AskQuestion
import org.flottille.core.Dismissed
import org.flottille.core.Machine
import org.flottille.core.MachineState
import org.flottille.core.Notice
import org.flottille.core.PhoneView
import org.flottille.core.Project
import org.flottille.core.SessionRow
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test

/**
 * The notifications on the emulator (#828, `docs/remote.md`, "Notifications"): the service that keeps
 * the connection to the hub, its notifications, Allow and Deny and the activity they open are the
 * real ones; the core is played by [PlayedPhone], whose view says what the hub told.
 */
class NotificationsOnThePhoneTest {
    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context: Context = instrumentation.targetContext
    private val server = FakeRunServer()
    private val manager = context.getSystemService(NotificationManager::class.java)
    private lateinit var phone: PlayedPhone
    private val application get() = context.applicationContext as FlottilleApplication

    private val request = Notice("server", 12, "request", "a1", "Request #7 waits for you", "Run make", 7)
    private val question = Notice("server", 13, "question", "a1", "Asks you a question", "Which branch?", 0)

    @Before
    fun a_phone_joined_and_kept_connected() {
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
        manager.cancelAll()
        phone = PlayedPhone(server, joined("controller"))
        application.phone = phone
    }

    @After
    fun nothing_is_left_running() {
        context.stopService(android.content.Intent(context, HubService::class.java))
        instrumentation.runOnMainSync {
            listOf(Stage.RESUMED, Stage.STARTED, Stage.PAUSED, Stage.STOPPED, Stage.CREATED).forEach { stage ->
                ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(stage)
                    .filterIsInstance<MainActivity>()
                    .forEach { it.finish() }
            }
        }
        manager.cancelAll()
        server.close()
    }

    private fun joined(role: String) = PhoneView(
        role = role,
        name = "pixel",
        machines = listOf(Machine("server", MachineState.Online, 0)),
        projects = listOf(Project("app", listOf(SessionRow("server", "a1", "Fix the login", "idle", 5)))),
    )

    private fun until(what: String, seconds: Int = 20, holds: () -> Boolean) {
        val limit = System.currentTimeMillis() + seconds * 1_000L
        while (!holds()) {
            check(System.currentTimeMillis() < limit) { "$what within $seconds s" }
            Thread.sleep(100)
        }
    }

    private fun drawn(notice: Notice): StatusBarNotification? =
        manager.activeNotifications.firstOrNull { it.tag == notice.machine && it.id == notice.notification.toInt() }

    private fun serviceRuns(): Boolean =
        @Suppress("DEPRECATION")
        (context.getSystemService(ActivityManager::class.java).getRunningServices(100))
            .any { it.service.className == HubService::class.java.name }

    /** The service started, as the app starts it once the phone joined, and [notice] told by the hub. */
    private fun told(notice: Notice): Notification {
        HubService.start(context)
        until("the service's notification") { manager.activeNotifications.any { it.id == HubService.CONNECTED_ID } }
        phone.state.value = phone.state.value.copy(notices = phone.state.value.notices + notice)
        until("the notice drawn") { drawn(notice) != null }
        return drawn(notice)!!.notification
    }

    private fun Notification.titles(): List<String> = actions.orEmpty().map { it.title.toString() }

    @Test
    fun a_request_push_draws_a_notification_with_allow_and_deny() {
        val drawn = told(request)
        assertEquals(listOf("Allow", "Deny"), drawn.titles())
        assertEquals("Request #7 waits for you", drawn.extras.getCharSequence(Notification.EXTRA_TITLE).toString())
        assertEquals("server · Run make", drawn.extras.getCharSequence(Notification.EXTRA_TEXT).toString())
        assertEquals(HubService.CHANNEL_NEEDS_YOU, drawn.channelId)
        // The connection is held for the service, which says so in its own notification.
        assertTrue(serviceRuns())
        assertEquals(1, phone.holds)
    }

    @Test
    fun allow_from_the_notification_answers_the_request_with_the_app_killed() {
        val drawn = told(request)
        // Nothing of the app runs any more: no screen, no service, no connection held.
        context.stopService(android.content.Intent(context, HubService::class.java))
        until("the service to end") { !serviceRuns() && phone.holds == 0 }
        drawn.actions.single { it.title == "Allow" }.actionIntent.send()
        until("the request answered") { phone.answers.isNotEmpty() }
        val answered = phone.answers.single()
        assertEquals(Answered("server", 7, true, answered.holds), answered)
        // The answer went over a connection the receiver held for it.
        assertTrue("held ${answered.holds}", answered.holds >= 1)
        until("the notification gone") { drawn(request) == null }
    }

    @Test
    fun a_dismissed_push_removes_the_notification() {
        told(request)
        phone.state.value = phone.state.value.copy(dismissed = listOf(Dismissed("server", 12)))
        until("the notification removed") { drawn(request) == null }
        // The service and its own notification stay.
        assertTrue(manager.activeNotifications.any { it.id == HubService.CONNECTED_ID })
    }

    @Test
    fun a_question_opens_its_ask_card() {
        phone.state.value = phone.state.value.copy(
            asks = listOf(Ask("server", "a1", 3, "Which branch?", listOf(AskQuestion("Which branch?", emptyList(), false)))),
        )
        val drawn = told(question)
        assertTrue(drawn.titles().isEmpty())
        assertNotNull(drawn.contentIntent)
        drawn.contentIntent.send()
        rule.waitUntil(20_000) { rule.onAllNodesWithTag("ask:3").fetchSemanticsNodes().isNotEmpty() }
        rule.onNodeWithTag("ask:3").assertIsDisplayed()
    }

    @Test
    fun a_viewer_s_request_notification_has_no_allow_or_deny() {
        phone.state.value = joined("viewer")
        val drawn = told(request)
        assertTrue(drawn.titles().isEmpty())
        // It opens the request, read only.
        assertNotNull(drawn.contentIntent)
    }

    @Test
    fun a_phone_that_may_sleep_says_it_in_settings() {
        // Android's own list of the apps exempted from battery optimisation, without this one.
        instrumentation.uiAutomation.executeShellCommand("dumpsys deviceidle whitelist -${context.packageName}").close()
        rule.setContent {
            SettingsScreen(view = joined("controller"), onBack = {}, onRemove = {}, background = SystemBackground(rule.activity))
        }
        rule.waitUntil(5_000) { rule.onAllNodesWithTag("background-sleep").fetchSemanticsNodes().isNotEmpty() }
        rule.onNodeWithTag("background-sleep").assertIsDisplayed()
        rule.onNodeWithTag("background-exempt").assertIsDisplayed()
    }
}
