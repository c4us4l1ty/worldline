package com.worldline.app

import android.os.Bundle
import android.view.View
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        // The recovery phrase and BYOK fields can be visible during entry.
        // Keep them out of screenshots, screen sharing and the Recents thumbnail.
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        val canvas = getColor(R.color.worldline_canvas)
        enableEdgeToEdge(SystemBarStyle.dark(canvas), SystemBarStyle.dark(canvas))
        super.onCreate(savedInstanceState)

        // Resize the WebView's frame rather than scrolling the whole document
        // behind the keyboard. One set of insets covers bars, cutouts and IME.
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val safe = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
            )
            view.setPadding(safe.left, safe.top, safe.right, safe.bottom)
            WindowInsetsCompat.CONSUMED
        }
        ViewCompat.requestApplyInsets(content)
    }

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            private var inFlight = false

            override fun handleOnBackPressed() {
                if (inFlight) return
                inFlight = true
                webView.evaluateJavascript("typeof window.wlHandleBack === 'function' && window.wlHandleBack()") { handled ->
                    inFlight = false
                    if (handled != "true" && !isFinishing && !isDestroyed) {
                        // At the canvas, let Android's normal Back action run.
                        isEnabled = false
                        onBackPressedDispatcher.onBackPressed()
                        isEnabled = true
                    }
                }
            }
        })
    }
}
