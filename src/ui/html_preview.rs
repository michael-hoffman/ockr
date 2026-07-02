//! HTML preview pane backed by `WKWebView`.
//!
//! ## Architecture
//!
//! `HtmlWebView` is a thin Rust wrapper around a macOS `WKWebView` that is
//! added as an `NSView` subview of the main window's content view.  GPUI
//! renders its own Metal-backed content layer; the `WKWebView` composites
//! on top of it for the preview area.
//!
//! ## Preloading
//!
//! On construction, `preload()` loads a skeleton HTML document that contains
//! the full Oxide CSS theme.  This forces the WKWebView process to:
//! - Start its JS / layout engine,
//! - Parse and cache the stylesheet,
//! - Warm up its GPU compositor.
//!
//! When the first real compile result arrives, `load_html()` swaps only the
//! `<body>` content via `document.body.innerHTML = …` JavaScript injection
//! (no full page reload), so the transition is near-instant.
//!
//! ## Thread safety
//!
//! All methods must be called from the **main thread**.  GPUI's render and
//! action handlers satisfy this requirement.

use futures::channel::mpsc::UnboundedSender;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use objc2_app_kit::NSApplication;
use objc2_web_kit::{
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController,
    WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
};

// ── Wikilink click handler (WKScriptMessageHandler) ───────────────────────────

/// Instance variables for `OckrLinkHandler`.
///
/// Holds the channel sender used to report clicked `ockr://` link paths back
/// to the main-window async loop where they are resolved and opened.
struct LinkHandlerIvars {
    sender: UnboundedSender<String>,
}

define_class!(
    // SAFETY: Super is NSObject (no subclassing requirements); we don't impl Drop.
    #[unsafe(super(NSObject))]
    // Mark as main-thread-only to satisfy the WKScriptMessageHandler requirement.
    #[thread_kind = MainThreadOnly]
    #[name = "OckrLinkHandler"]
    #[ivars = LinkHandlerIvars]
    struct OckrLinkHandler;

    unsafe impl NSObjectProtocol for OckrLinkHandler {}

    unsafe impl WKScriptMessageHandler for OckrLinkHandler {
        /// Called by WebKit when JS posts a message to the `"ockrLink"` handler.
        ///
        /// The message body is the `ockr://` path stripped of its scheme, e.g.
        /// `"zettels/bayes-theorem.typ"`.  We forward it to the main-window
        /// async loop via the unbounded channel.
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        unsafe fn user_content_controller_did_receive_message(
            &self,
            _controller: &WKUserContentController,
            message: &WKScriptMessage,
        ) {
            let body = message.body();
            if let Some(s) = body.downcast_ref::<NSString>() {
                let path = s.to_string();
                let _ = self.ivars().sender.unbounded_send(path);
            }
        }
    }
);

impl OckrLinkHandler {
    fn new(sender: UnboundedSender<String>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(LinkHandlerIvars { sender });
        unsafe { msg_send![super(this), init] }
    }
}

// ── CSS theme (Oxide — warm dark) ─────────────────────────────────────────────

/// Oxide theme CSS injected into every HTML preview document.
///
/// Colors sourced directly from `themes/oxide.toml`.  Scrollbar styling,
/// selection highlight, code blocks, and headings all follow the app palette.
const OXIDE_CSS: &str = r#"
:root { color-scheme: dark; }

* { box-sizing: border-box; }

body {
    font-family: -apple-system, "Helvetica Neue", Arial, sans-serif;
    background: #0B0907;
    color: #F5F0EA;
    max-width: 740px;
    margin: 0 auto;
    padding: 2.2em 2em 4em;
    line-height: 1.65;
    font-size: 15px;
}

h1, h2, h3, h4, h5, h6 {
    color: #F5F0EA;
    font-weight: 600;
    margin-top: 1.6em;
    margin-bottom: 0.4em;
    line-height: 1.3;
}
h1 {
    font-size: 1.8em;
    border-bottom: 1px solid #2C251C;
    padding-bottom: 0.35em;
    margin-top: 0.4em;
}
h2 { font-size: 1.4em; }
h3 { font-size: 1.15em; }
h4, h5, h6 { font-size: 1em; color: #A89F93; }

p { margin: 0.75em 0; }

a { color: #CC7722; text-decoration: none; }
a:hover { text-decoration: underline; color: #e8932e; }

code, kbd {
    font-family: "SF Mono", "Menlo", monospace;
    background: #16120E;
    border: 1px solid #2C251C;
    border-radius: 3px;
    padding: 0.1em 0.35em;
    font-size: 0.88em;
}

pre {
    font-family: "SF Mono", "Menlo", monospace;
    background: #16120E;
    border: 1px solid #2C251C;
    border-radius: 5px;
    padding: 1em 1.25em;
    overflow-x: auto;
    font-size: 0.85em;
    line-height: 1.5;
    margin: 1em 0;
}
pre code { background: none; border: none; padding: 0; }

blockquote {
    border-left: 3px solid #CC7722;
    margin: 1.2em 0;
    padding: 0.3em 1.2em;
    color: #A89F93;
    background: #110E0B;
    border-radius: 0 4px 4px 0;
}

hr { border: none; border-top: 1px solid #2C251C; margin: 2em 0; }

table { border-collapse: collapse; width: 100%; margin: 1em 0; }
th { background: #16120E; color: #F5F0EA; font-weight: 600; }
th, td { border: 1px solid #2C251C; padding: 0.5em 0.85em; text-align: left; }
tr:nth-child(even) td { background: #0E0B09; }

ul, ol { padding-left: 1.6em; margin: 0.5em 0; }
li { margin: 0.3em 0; }

strong { color: #F5F0EA; font-weight: 600; }
em { color: #D6CFC5; }

::selection { background: #4A2810; color: #F5F0EA; }

::-webkit-scrollbar { width: 7px; height: 7px; }
::-webkit-scrollbar-track { background: #0B0907; }
::-webkit-scrollbar-thumb { background: #2C251C; border-radius: 4px; }
::-webkit-scrollbar-thumb:hover { background: #3A3A3A; }
"#;

/// JavaScript injected into every preview page to intercept `ockr://` link clicks.
///
/// When the user clicks a `<a href="ockr://some/path.typ">` element the default
/// navigation is cancelled and the path (everything after `ockr://`) is posted
/// to the native `WKScriptMessageHandler` registered under `"ockrLink"`.
const LINK_INTERCEPT_JS: &str = r#"
(function() {
    document.addEventListener('click', function(evt) {
        var a = evt.target.closest('a[href]');
        if (!a) return;
        var href = a.getAttribute('href') || '';
        if (href.startsWith('ockr://')) {
            evt.preventDefault();
            evt.stopPropagation();
            var path = href.slice('ockr://'.length);
            window.webkit.messageHandlers.ockrLink.postMessage(path);
        }
    }, true);
})();
"#;

// ── Wrapper struct ─────────────────────────────────────────────────────────────

/// A live `WKWebView` positioned over the GPUI preview area.
///
/// Dropped when the owning `MainWindow` drops it; `Drop` removes the
/// WKWebView from its superview so it doesn't linger in the window.
pub struct HtmlWebView {
    webview: Retained<WKWebView>,
}

impl HtmlWebView {
    /// Create a new `WKWebView`, attach it as a subview of the main window's
    /// content view, and immediately pre-warm it with the Oxide CSS skeleton.
    ///
    /// `link_sender` receives the vault-relative path (e.g.
    /// `"zettels/bayes-theorem.typ"`) whenever the user clicks an `ockr://`
    /// link inside the preview.  The caller is responsible for resolving this
    /// to an absolute path and opening it.
    ///
    /// Returns `None` if no main window is available yet (should not happen
    /// in normal usage since we create this lazily during first render).
    pub fn new(link_sender: UnboundedSender<String>) -> Option<Self> {
        unsafe {
            let mtm = MainThreadMarker::new_unchecked();
            let app = NSApplication::sharedApplication(mtm);
            let ns_window = app.mainWindow()?;
            let content_view = ns_window.contentView()?;

            // Build WKWebView configuration.
            let config = WKWebViewConfiguration::new(mtm);

            // ── Wikilink message handler ──────────────────────────────────
            // OckrLinkHandler implements WKScriptMessageHandler and forwards
            // `ockr://` link click payloads to `link_sender`.
            let handler = OckrLinkHandler::new(link_sender, mtm);
            let ucc = config.userContentController();
            let handler_proto = ProtocolObject::from_ref(&*handler);
            let handler_name = NSString::from_str("ockrLink");
            ucc.addScriptMessageHandler_name(handler_proto, &handler_name);

            // ── Inject link-interception user script ──────────────────────
            // Runs at document-end in every page so freshly loaded HTML is
            // already in the DOM.  Any click on an <a href="ockr://..."> is
            // cancelled and the path (after stripping the scheme) is posted
            // to the native handler above.
            let js_source = NSString::from_str(LINK_INTERCEPT_JS);
            let user_script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &js_source,
                WKUserScriptInjectionTime::AtDocumentEnd,
                true,
            );
            ucc.addUserScript(&user_script);

            // Create with a zero frame; `update_frame` sets the real position.
            let frame = objc2_foundation::NSRect {
                origin: objc2_foundation::NSPoint { x: 0.0, y: 0.0 },
                size: objc2_foundation::NSSize { width: 1.0, height: 1.0 },
            };
            let webview = WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                frame,
                &config,
            );

            // Layer-backed view for proper Metal compositing.
            webview.setWantsLayer(true);

            // Start hidden; MainWindow::render reveals it on first frame.
            webview.setHidden(true);

            content_view.addSubview(&webview);

            let this = Self { webview };
            this.preload();
            Some(this)
        }
    }

    /// Load the Oxide CSS skeleton so the WKWebView process warms up
    /// immediately.  Called once at construction time.
    pub fn preload(&self) {
        let skeleton = format!(
            "<!DOCTYPE html><html><head>\
             <meta charset='utf-8'>\
             <meta name='viewport' content='width=device-width,initial-scale=1'>\
             <style>{OXIDE_CSS}</style>\
             </head><body></body></html>"
        );
        self.load_html_string(&skeleton);
    }

    /// Display a compiled HTML document in the web view.
    ///
    /// Injects the Oxide CSS into the document's `<head>` so Typst's own
    /// styles are augmented with our dark-theme defaults.
    pub fn load_html(&self, typst_html: &str) {
        let styled = inject_css(typst_html);
        self.load_html_string(&styled);
    }

    /// Show an error message (e.g. compiler diagnostic) in the web view.
    pub fn load_error(&self, msg: &str) {
        let safe_msg = html_escape(msg);
        let html = format!(
            "<!DOCTYPE html><html><head>\
             <meta charset='utf-8'>\
             <style>{OXIDE_CSS}\
             .err {{ color: #ff6b6b; font-family: 'SF Mono','Menlo',monospace; \
                     font-size: 13px; padding: 2em; white-space: pre-wrap; }}\
             </style></head>\
             <body><div class='err'>{safe_msg}</div></body></html>"
        );
        self.load_html_string(&html);
    }

    /// Reposition the WKWebView to cover `(x, y, width, height)` in the
    /// window's content-view coordinate system (AppKit: y=0 at bottom).
    pub fn update_frame(&self, x: f64, y: f64, width: f64, height: f64) {
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        self.webview.setFrame(NSRect {
            origin: NSPoint { x, y },
            size: NSSize { width, height },
        });
    }

    /// Show or hide the web view without removing it from the hierarchy.
    pub fn set_hidden(&self, hidden: bool) {
        self.webview.setHidden(hidden);
    }

    // ── Private helpers ────────────────────────────────────────────────────

    fn load_html_string(&self, html: &str) {
        unsafe {
            let ns_html = NSString::from_str(html);
            self.webview.loadHTMLString_baseURL(&ns_html, None);
        }
    }
}

impl Drop for HtmlWebView {
    fn drop(&mut self) {
        self.webview.removeFromSuperview();
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Inject the Oxide `<style>` block into a complete HTML document string.
///
/// Inserts just before `</head>`.  If no `</head>` is present (malformed or
/// minimal Typst output) the CSS is prepended as a `<style>` block anyway.
fn inject_css(html: &str) -> String {
    let tag = format!("<style>{OXIDE_CSS}</style>");
    if let Some(pos) = html.find("</head>") {
        let mut out = String::with_capacity(html.len() + tag.len());
        out.push_str(&html[..pos]);
        out.push_str(&tag);
        out.push_str(&html[pos..]);
        out
    } else {
        // Fallback: wrap with a minimal head containing our CSS.
        format!(
            "<!DOCTYPE html><html><head><meta charset='utf-8'>{tag}</head>{}",
            html
        )
    }
}

/// Escape characters that are significant in HTML contexts.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}
