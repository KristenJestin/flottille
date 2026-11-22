package org.flottille.app

import androidx.activity.ComponentActivity
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The app starts (#826): the system builds the activity first and hands it its context after. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], application = android.app.Application::class)
class MainActivityStartTest {
    @Test
    fun the_activity_is_built_before_it_has_a_context() {
        // The system calls the constructor, then attach(): a property initialiser that asks the
        // activity for its cache folder crashed the app at launch.
        Robolectric.buildActivity(MainActivity::class.java)
    }

    @Test
    fun a_shot_left_by_a_dead_process_is_cleared_once_the_activity_has_its_context() {
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).create().get()
        val folder = java.io.File(activity.cacheDir, "pictures").apply { mkdirs() }
        java.io.File(folder, "photo-1.jpg").writeText("left")

        SystemPictures(activity).clearLeftovers()

        assertEquals(emptyList<String>(), folder.list()!!.toList())
    }
}
