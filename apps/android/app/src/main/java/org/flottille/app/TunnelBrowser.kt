package org.flottille.app

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.webkit.CookieManager
import android.webkit.WebResourceRequest
import android.webkit.WebStorage
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import android.webkit.WebResourceError
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.viewinterop.AndroidView
import org.flottille.app.ui.ButtonKind
import org.flottille.app.ui.FButton
import org.flottille.app.ui.Label
import org.flottille.app.ui.LocalPalette
import org.flottille.app.ui.Message
import org.flottille.app.ui.Space
import org.flottille.app.ui.Tag
import org.flottille.app.ui.Type
import org.flottille.core.OpenedTunnel

/**
 * A run's page in the app (`docs/remote.md`, "Tunnels", "On the phone"): a web view on the address
 * read through the tunnel, `http://localhost:<port>/…` on this phone's own loopback. The tunnel
 * lives while the app is on screen; when it ends (the run ended, the link dropped) the page says so
 * and the user closes it. Only that origin loads here: a link to anywhere else is handed to the
 * user's browser through [onOutside], never loaded inside the app. The screen comes in and leaves
 * with the panel's duration (the caller's transition); the page's own colours are the page's.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun TunnelBrowser(
    opened: OpenedTunnel,
    ended: String?,
    onOutside: (String) -> Unit,
    onClose: () -> Unit,
) {
    val palette = LocalPalette.current
    // The page's view, for Back: held in a box, never in state (it is not something to draw).
    val web = remember { arrayOfNulls<WebView>(1) }
    var failed by remember { mutableStateOf<String?>(null) }
    BackHandler {
        val shown = web[0]
        if (shown != null && shown.canGoBack()) shown.goBack() else onClose()
    }
    Column(Modifier.fillMaxSize().background(palette.bg).testTag("browser")) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = Space.Lg, vertical = Space.Sm),
            horizontalArrangement = Arrangement.spacedBy(Space.Sm),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            FButton("Close", onClose, kind = ButtonKind.Ghost, modifier = Modifier.testTag("browser-close"))
            Label(opened.url, style = Type.Small, color = palette.mute, modifier = Modifier.weight(1f))
            Tag(opened.tunnel.machine)
        }
        Message(ended?.let { "The tunnel ended: $it" }, modifier = Modifier.padding(horizontal = Space.Lg), tag = "browser-ended")
        Message(failed?.let { "The page could not be loaded: $it" }, modifier = Modifier.padding(horizontal = Space.Lg), tag = "browser-failed")
        AndroidView(
            modifier = Modifier.weight(1f).fillMaxWidth().testTag("browser-page"),
            factory = { context ->
                WebView(context).apply {
                    settings.javaScriptEnabled = true
                    settings.domStorageEnabled = true
                    // A page of a run is reached through the tunnel and nothing else: no file, no
                    // content of this phone, no mixed content.
                    settings.allowFileAccess = false
                    settings.allowContentAccess = false
                    settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                    webViewClient = TunnelWebViewClient(opened.tunnel.localPort, onOutside) { failed = it }
                    loadUrl(opened.url)
                    web[0] = this
                }
            },
            onRelease = {
                it.stopLoading()
                // The tunnel closes with the panel: nothing of its page stays for the next one, whose
                // origin (`localhost`, whatever the port) the cookies and the storage do not tell apart.
                clearTunnelSite(it)
                it.destroy()
            },
        )
    }
    // A tunnel that ended on its own leaves the page up, to say so: its site goes with it.
    LaunchedEffect(ended != null) { if (ended != null) clearTunnelSite(web[0]) }
}

/**
 * Forgets what the pages of tunnels stored in the web view: cookies and web storage are shared by
 * every `localhost` page whatever its port, so a run's page would hand them to the next run's;
 * the HTTP cache of [page] goes too.
 */
internal fun clearTunnelSite(page: WebView?) {
    CookieManager.getInstance().removeAllCookies(null)
    CookieManager.getInstance().flush()
    WebStorage.getInstance().deleteAllData()
    page?.clearCache(true)
}

/**
 * What the in-app web view does with a navigation. The tunnel's own origin loads. Anything else is
 * never loaded here, and is handed to the user's browser only when it is an `http(s)` address the
 * main frame goes to because the user acted (a tap): a page cannot start another app with a
 * `intent:` or `market:` address, nor open one by itself, nor from a frame.
 */
internal class TunnelWebViewClient(
    private val localPort: Int,
    private val onOutside: (String) -> Unit,
    private val onFailed: (String) -> Unit,
) : WebViewClient() {
    override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
        if (request.isForMainFrame) onFailed(error.description.toString())
    }

    private fun here(uri: android.net.Uri): Boolean {
        val host = uri.host.orEmpty()
        return (host == "localhost" || host == "127.0.0.1" || host.endsWith(".localhost")) && uri.port == localPort
    }

    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
        val uri = request.url
        if (here(uri)) return false
        if (request.isForMainFrame && request.hasGesture() && isWebAddress(uri.toString())) onOutside(uri.toString())
        return true
    }

    /**
     * The check [shouldOverrideUrlLoading] cannot make: it is not called for a POST, so a form that
     * posts to another site would load that site here, under the tunnel's address. A main frame
     * that starts loading a web address outside the tunnel's origin is stopped, and the page goes
     * back to what it showed when the load had got as far as showing something else.
     */
    override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
        if (!isWebAddress(url) || here(android.net.Uri.parse(url))) return
        view.stopLoading()
        val shown = view.url
        if (shown != null && !here(android.net.Uri.parse(shown)) && view.canGoBack()) view.goBack()
    }
}

/** Whether [url] is an `http` or `https` address: the only ones the user's browser is handed. */
fun isWebAddress(url: String): Boolean {
    val scheme = android.net.Uri.parse(url).scheme?.lowercase()
    return scheme == "http" || scheme == "https"
}
