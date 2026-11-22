package org.flottille.app

import android.net.Uri
import android.webkit.WebResourceRequest
import android.webkit.WebView
import androidx.activity.ComponentActivity
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

/**
 * What the in-app web view does with the navigations of a run's page (#831): the tunnel's origin
 * loads, a tapped `http(s)` link of the main frame goes to the user's browser, and everything else
 * is dropped: a page cannot start another app, nor open the browser by itself.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], application = android.app.Application::class)
class TunnelWebViewClientTest {
    private val handed = mutableListOf<String>()
    private val client = TunnelWebViewClient(41000, { handed += it }, { })
    private val view = WebView(RuntimeEnvironment.getApplication())

    private fun request(url: String, mainFrame: Boolean = true, gesture: Boolean = true) = object : WebResourceRequest {
        override fun getUrl(): Uri = Uri.parse(url)
        override fun isForMainFrame() = mainFrame
        override fun isRedirect() = false
        override fun hasGesture() = gesture
        override fun getMethod() = "GET"
        override fun getRequestHeaders(): Map<String, String> = emptyMap()
    }

    @Test
    fun the_tunnel_s_own_origin_loads_in_the_app() {
        assertFalse(client.shouldOverrideUrlLoading(view, request("http://localhost:41000/docs")))
        assertFalse(client.shouldOverrideUrlLoading(view, request("http://app.localhost:41000/", gesture = false)))
        assertTrue(handed.isEmpty())
    }

    @Test
    fun a_tapped_web_link_of_the_main_frame_goes_to_the_user_s_browser() {
        assertTrue(client.shouldOverrideUrlLoading(view, request("https://example.org/docs")))
        assertTrue(client.shouldOverrideUrlLoading(view, request("HTTP://example.org/")))
        assertEquals(listOf("https://example.org/docs", "HTTP://example.org/"), handed)
    }

    @Test
    fun a_page_that_navigates_by_itself_opens_nothing() {
        // A redirect or a script, no tap: dropped, never loaded in the app.
        assertTrue(client.shouldOverrideUrlLoading(view, request("https://example.org/", gesture = false)))
        assertTrue(handed.isEmpty())
    }

    @Test
    fun a_frame_of_the_page_opens_nothing() {
        assertTrue(client.shouldOverrideUrlLoading(view, request("https://example.org/", mainFrame = false)))
        assertTrue(handed.isEmpty())
    }

    @Test
    fun an_address_of_another_scheme_starts_no_app() {
        for (url in listOf(
            "intent://scan/#Intent;scheme=zxing;package=com.example;end",
            "market://details?id=com.example",
            "file:///sdcard/Download/a.html",
            "content://com.example.provider/a",
            "tel:+33123456789",
            "sms:123",
            "javascript:alert(1)",
            "android-app://com.example",
        )) {
            assertTrue(url, client.shouldOverrideUrlLoading(view, request(url)))
        }
        assertTrue(handed.toString(), handed.isEmpty())
    }

    @Test
    fun an_address_on_another_port_of_the_loopback_is_not_the_tunnel_s() {
        // Another tunnel's, or another app's: handed out as any web address, never loaded here.
        assertTrue(client.shouldOverrideUrlLoading(view, request("http://localhost:41001/")))
        assertEquals(listOf("http://localhost:41001/"), handed)
    }

    @Test
    fun the_system_hands_the_browser_a_web_address_only() {
        val launched = mutableListOf<String>()
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).create().get()
        val host = SystemTunnelHost(activity, { launched += it; true })
        host.browse("intent://scan/#Intent;scheme=zxing;end")
        host.browse("market://details?id=com.example")
        host.browse("file:///sdcard/a.html")
        host.browse("https://example.org/docs")
        assertEquals(listOf("https://example.org/docs"), launched)
    }
}
