package org.flottille.app

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
}
