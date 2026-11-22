package org.flottille.app

import android.annotation.SuppressLint
import android.webkit.WebResourceRequest
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
import androidx.compose.runtime.remember
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
                    webViewClient = object : WebViewClient() {
                        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                            val uri = request.url
                            val host = uri.host.orEmpty()
                            val here = (host == "localhost" || host == "127.0.0.1" || host.endsWith(".localhost")) &&
                                uri.port == opened.tunnel.localPort
                            if (!here) onOutside(uri.toString())
                            return !here
                        }
                    }
                    loadUrl(opened.url)
                    web[0] = this
                }
            },
            onRelease = {
                it.stopLoading()
                it.destroy()
            },
        )
    }
}
