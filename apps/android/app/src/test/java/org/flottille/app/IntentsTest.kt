package org.flottille.app

import android.content.ComponentName
import android.content.Intent
import android.net.Uri
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

/** What an intent may ask of the app (#826): the share sheet is open to every app, a session is not. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], application = android.app.Application::class)
class IntentsTest {
    private val own = "org.flottille.app"

    private fun opening(component: ComponentName?) =
        Intent().apply {
            this.component = component
            putExtra(MainActivity.EXTRA_MACHINE, "server")
            putExtra(MainActivity.EXTRA_AGENT, "a1")
        }

    @Test
    fun the_app_s_own_entry_point_opens_a_session() {
        val intent = opening(ComponentName(own, OPEN_SESSION))
        assertEquals(SessionKey("server", "a1"), asked(intent, own, recreated = false).open)
    }

    @Test
    fun the_extras_of_any_other_app_open_nothing() {
        // The launcher's activity is exported: an intent of another app names it, never the
        // entry point the app's own notifications go through.
        val direct = opening(ComponentName(own, "$own.MainActivity"))
        assertNull(asked(direct, own, recreated = false).open)
        assertNull(asked(opening(null), own, recreated = false).open)
        assertNull(asked(opening(ComponentName("evil.app", OPEN_SESSION)), own, recreated = false).open)
    }

    @Test
    fun the_entry_point_is_not_exported_so_that_only_the_app_reaches_it() {
        val info = RuntimeEnvironment.getApplication().packageManager.getActivityInfo(ComponentName(own, OPEN_SESSION), 0)
        assertFalse("another app can start it", info.exported)
        assertEquals("$own.MainActivity", info.targetActivity)
    }

    @Test
    fun a_share_is_read_once_and_not_again_when_the_activity_is_recreated() {
        val share = Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, Uri.parse("content://media/external/images/1"))
        assertTrue(asked(share, own, recreated = false).share)
        // A rotation recreates the activity with the same intent: the picture is not read again.
        assertFalse(asked(share, own, recreated = true).share)
    }
}
