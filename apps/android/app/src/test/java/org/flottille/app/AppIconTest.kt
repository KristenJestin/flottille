package org.flottille.app

import android.content.pm.PackageManager
import android.graphics.drawable.AdaptiveIconDrawable
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The app wears Flottille's icon (#1242): its own resources, not Android's default. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], application = android.app.Application::class)
class AppIconTest {
    private val context: android.content.Context = ApplicationProvider.getApplicationContext()
    private val application = context.packageManager.getApplicationInfo(context.packageName, PackageManager.GET_META_DATA)

    private fun entry(id: Int): String = context.resources.getResourceName(id)

    @Test
    fun the_application_icon_is_the_app_s_own_resource() {
        assertNotEquals("the manifest names no android:icon: Android's default shows", 0, application.icon)
        assertEquals("org.flottille.app:mipmap/ic_launcher", entry(application.icon))
    }

    @Test
    fun the_round_icon_is_the_app_s_own_resource() {
        val id = context.resources.getIdentifier("ic_launcher_round", "mipmap", context.packageName)
        assertNotEquals("no mipmap/ic_launcher_round", 0, id)
        // The merged manifest is not read back by Robolectric's PackageManager for roundIcon: the
        // source manifest is the place the attribute is set.
        val manifest = java.io.File("src/main/AndroidManifest.xml").readText()
        assertTrue(manifest, manifest.contains("android:roundIcon=\"@mipmap/ic_launcher_round\""))
    }

    @Test
    fun the_launcher_icon_is_adaptive_with_a_themed_layer() {
        val icon = context.packageManager.getApplicationIcon(context.packageName)
        assertTrue("an adaptive icon, got ${icon::class.java.name}", icon is AdaptiveIconDrawable)
        icon as AdaptiveIconDrawable
        assertNotNull("the mark in the foreground", icon.foreground)
        assertNotNull("the brand colour behind it", icon.background)
        assertNotNull("a monochrome layer for themed icons", icon.monochrome)
    }

    @Test
    fun the_notification_s_small_icon_is_the_app_s_own_drawable() {
        val id = context.resources.getIdentifier("ic_notification", "drawable", context.packageName)
        assertNotEquals("no drawable/ic_notification: the system's stands in", 0, id)
        assertNotNull(context.getDrawable(id))
    }
}
