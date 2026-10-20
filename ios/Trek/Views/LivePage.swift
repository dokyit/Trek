import SwiftUI
import UIKit
import WebKit

/// An interactive artifact (a `trek-html` block): the agent's own HTML page, which the Mac runs live
/// in the answer. The phone shows its title and runs it on a tap, so a long transcript with a few of
/// them stays light; then it runs inline in a sandboxed web view (`LivePageView`), styled with
/// Trek's `--trek-*` variables and as tall as its content.
struct LivePageCard: View {
    var title: String?
    var html: String
    var size: CGFloat

    @State private var running = false
    @State private var showingSource = false
    @State private var height: CGFloat
    @Environment(\.colorScheme) private var scheme
    @Environment(\.dynamicTypeSize) private var dynamicType
    @Environment(AppModel.self) private var model: AppModel?

    init(title: String?, html: String, size: CGFloat) {
        self.title = title
        self.html = html
        self.size = size
        _height = State(initialValue: LivePage.heights[html.hashValue] ?? LivePage.defaultHeight)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            if running {
                Rectangle().fill(Trek.border).frame(height: 0.5)
                LivePageView(html: html, theme: theme) { h in
                    LivePage.heights[html.hashValue] = h
                    height = h
                }
                .frame(height: height)
                .background(Trek.background)
                .transition(.opacity)
            }
        }
        .background(Trek.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        .sheet(isPresented: $showingSource) { LongTextSheet(title: title ?? "Page source", text: html) }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: "cube.transparent")
                .font(.system(size: size * 1.05, weight: .medium))
                .foregroundStyle(Trek.ember)
                .frame(width: 34, height: 34)
                .background(Trek.foreground.opacity(0.05), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(title ?? "Interactive page")
                    .scaledFont(size * 0.95, weight: .semibold)
                    .foregroundStyle(Trek.foreground)
                    .lineLimit(2)
                Text(LivePage.summary(html))
                    .scaledFont(size * 0.8)
                    .foregroundStyle(Trek.muted)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Button {
                withAnimation(.easeOut(duration: 0.2)) { running.toggle() }
            } label: {
                Label(running ? "Stop" : "Run", systemImage: running ? "stop.fill" : "play.fill")
                    .scaledFont(size * 0.82, weight: .semibold)
                    .foregroundStyle(Trek.foreground)
                    .padding(.horizontal, 11)
                    .padding(.vertical, 6)
                    .background(Trek.foreground.opacity(0.07), in: Capsule())
                    .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityHint(running ? "Stops the page" : "Runs the page here")
            Menu {
                Button("View source", systemImage: "chevron.left.forwardslash.chevron.right") { showingSource = true }
                Button("Copy HTML", systemImage: "doc.on.doc") {
                    UIPasteboard.general.string = html
                    model?.show("HTML copied")
                }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Trek.muted)
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("More")
        }
        .padding(12)
    }

    private var theme: LivePageTheme {
        // The Mac's page text is its transcript's, a shade smaller.
        let px = size * 0.93 * TextScale.factor(dynamicType)
        return LivePageTheme(dark: scheme == .dark, fontSize: (px * 2).rounded() / 2)
    }
}

/// What the card knows of a page before it runs, and what it keeps between runs.
enum LivePage {
    /// The web view is the page's height, within these (the Mac's, in points).
    static let minHeight: CGFloat = 160
    static let maxHeight: CGFloat = 720
    /// Before the page has said how tall it is.
    static let defaultHeight: CGFloat = 360
    /// A page that keeps changing its height is let be at this many changes.
    static let maxHeightChanges = 60

    /// Pages' heights as they last said, so a card made again (scrolled back to, the thread shown
    /// again) takes its height at once and the transcript doesn't jump.
    static var heights: [Int: CGFloat] = [:]

    /// "Uses cdn.jsdelivr.net" (where it loads from), or just what it is. Short: it shares a line
    /// with the Run button.
    static func summary(_ html: String) -> String {
        let hosts = hosts(html, max: 2)
        return hosts.isEmpty ? "Interactive page" : "Uses " + hosts.joined(separator: ", ")
    }

    /// The hosts the page loads from over https, in the order it names them.
    static func hosts(_ html: String, max: Int) -> [String] {
        var out: [String] = []
        var from = html.startIndex
        while out.count < max, let at = html.range(of: "https://", range: from..<html.endIndex) {
            let host = html[at.upperBound...].prefix { $0.isLetter || $0.isNumber || $0 == "." || $0 == "-" }.lowercased()
            if host.contains("."), !out.contains(host) { out.append(host) }
            from = at.upperBound
        }
        return out
    }

    /// `url` if it's one to hand Safari (or Mail): http, https or mailto, and not absurdly long.
    static func external(_ url: URL?) -> URL? {
        guard let url, let scheme = url.scheme?.lowercased(), url.absoluteString.count <= 2048 else { return nil }
        switch scheme {
        case "http", "https": return url.host?.isEmpty == false ? url : nil
        case "mailto": return url.absoluteString.count > 7 ? url : nil
        default: return nil
        }
    }

    /// What the page may load and run: the Mac's policy. Scripts, styles, images, fonts and media
    /// over https (CDNs), inline code; nothing over http or from files, no forms, no plugins.
    static let csp = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' https: blob:; style-src 'unsafe-inline' https:; img-src https: data: blob:; font-src https: data:; media-src https: data: blob:; connect-src https: data: blob:; worker-src blob: https:; child-src blob: data:; frame-src blob: data:; object-src 'none'; form-action 'none'; base-uri 'none'"

    /// The id of the stylesheet the theme goes in.
    static let themeStyleID = "trek-theme"

    /// The page as it's loaded: `html` with Trek's head first (charset, the policy, the viewport
    /// and the theme's stylesheet), so the page's own styles come after and win. A fragment (no
    /// `<html>` or `<head>`) becomes the body.
    static func document(_ html: String, theme: LivePageTheme) -> String {
        let ours = "<meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"\(csp)\">"
            + "<meta name=\"viewport\" content=\"width=device-width,initial-scale=1,maximum-scale=1\">"
            + "<style id=\"\(themeStyleID)\">\(theme.css.replacingOccurrences(of: "</", with: "<\\/"))</style>"
        // Ours is the very first thing the parser sees, so the policy is in force before any of
        // the page's markup, however the page begins (a script ahead of its `<head>`, a comment
        // that names one). The page's own doctype, `<html>` and `<head>` after it merge into
        // this head, and a fragment falls into the body the parser makes.
        return "<!doctype html><html><head>\(ours)</head>\(html)"
    }

    /// A script that swaps the page's theme stylesheet for `css` in place: a switch between Night
    /// and Paper reaches a running page without reloading it.
    static func themeUpdate(_ css: String) -> String {
        let quoted = (try? String(data: JSONEncoder().encode(css), encoding: .utf8)) ?? "\"\""
        return "(()=>{let s=document.getElementById('\(themeStyleID)');if(!s){s=document.createElement('style');s.id='\(themeStyleID)';(document.head||document.documentElement).prepend(s);}s.textContent=\(quoted);})();"
    }

    /// Run first, in a world of its own the page can't see (so it can't post as Trek): reports the
    /// content's height as it changes (`h`), and sends a tapped link to elsewhere to Safari (`open`);
    /// the page itself never leaves its document.
    static let bootstrap = #"""
    (() => {
      const post = (o) => { try { window.webkit.messageHandlers.trek.postMessage(o); } catch (_) {} };
      let last = -1, queued = false;
      const measure = () => {
        queued = false;
        const b = document.body;
        if (!b) return;
        let bottom = b.getBoundingClientRect().bottom;
        for (const c of b.children) {
          const r = c.getBoundingClientRect();
          bottom = Math.max(bottom, r.bottom + (parseFloat(getComputedStyle(c).marginBottom) || 0));
        }
        const h = Math.ceil(bottom + window.scrollY + (parseFloat(getComputedStyle(b).marginBottom) || 0));
        if (h > 0 && Math.abs(h - last) >= 1) { last = h; post({ t: 'h', v: h }); }
      };
      const later = () => { if (!queued) { queued = true; setTimeout(() => requestAnimationFrame(measure), 50); } };
      const watch = () => {
        try { const ro = new ResizeObserver(later); ro.observe(document.documentElement); if (document.body) ro.observe(document.body); } catch (_) {}
        try { new MutationObserver(later).observe(document.documentElement, { childList: true, subtree: true, attributes: true }); } catch (_) {}
        later();
      };
      if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', watch); else watch();
      window.addEventListener('load', later);
      window.addEventListener('click', (e) => {
        if (!e.isTrusted || e.defaultPrevented || e.button !== 0) return;
        const a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
        if (!a) return;
        let u;
        try { u = new URL(a.href, location.href); } catch (_) { return; }
        if (u.protocol === 'about:' || (u.origin === location.origin && u.pathname === location.pathname && u.hash)) return;
        e.preventDefault();
        if (/^(https?|mailto):$/.test(u.protocol)) post({ t: 'open', u: u.href });
      });
    })();
    """#
}

/// Trek's look as a page sees it: the `--trek-*` variables for Night or Paper, and quiet defaults
/// (transparent page, Trek's font and colours, no body margin) under `:where()`, so any rule the
/// page has wins over them.
struct LivePageTheme: Equatable {
    var dark: Bool
    var fontSize: CGFloat

    var css: String {
        let traits = UITraitCollection(userInterfaceStyle: dark ? .dark : .light)
        func c(_ color: Color) -> String {
            var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
            UIColor(color).resolvedColor(with: traits).getRed(&r, green: &g, blue: &b, alpha: &a)
            return String(format: "rgba(%.0f,%.0f,%.0f,%.3f)", r * 255, g * 255, b * 255, a)
        }
        let vars = [
            "color-scheme:\(dark ? "dark" : "light")",
            "--trek-bg:\(c(Trek.background))",
            "--trek-fg:\(c(Trek.foreground))",
            "--trek-muted:\(c(Trek.muted))",
            "--trek-accent:\(c(Trek.ember))",
            "--trek-border:\(c(Trek.border))",
            "--trek-surface:\(c(Trek.surface))",
            "--trek-positive:\(c(Trek.done))",
            "--trek-warning:\(c(Trek.approval))",
            "--trek-negative:\(c(Trek.failed))",
            "--trek-info:\(c(Trek.sky))",
            "--trek-font:\"\(TrekFont.ui)\", -apple-system, system-ui, sans-serif",
            "--trek-mono:\"\(TrekFont.code)\", ui-monospace, Menlo, monospace",
            "--trek-font-size:\(String(format: "%.1f", fontSize))px",
            "--trek-radius:8px",
        ]
        return ":root{\(vars.joined(separator: ";"))}"
            + ":where(html){background:transparent}"
            + ":where(body){margin:0;padding:12px 16px 16px;background:transparent;color:var(--trek-fg);font-family:var(--trek-font);font-size:var(--trek-font-size);line-height:1.5;-webkit-font-smoothing:antialiased;-webkit-text-size-adjust:100%}"
            + ":where(a){color:var(--trek-accent)}:where(code,pre,kbd){font-family:var(--trek-mono)}"
    }
}

/// The page, running: a web view with nothing kept (a store of its own, forgotten when it goes),
/// that loads only its own document and https subresources, sends links to Safari, takes the
/// theme as it changes, and is as tall as its content (`onHeight`). It doesn't scroll by itself
/// unless the page is taller than the card can be, so the transcript scrolls straight past it.
struct LivePageView: UIViewRepresentable {
    var html: String
    var theme: LivePageTheme
    var onHeight: (CGFloat) -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let coordinator = context.coordinator
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = true
        config.preferences.javaScriptCanOpenWindowsAutomatically = false
        config.allowsInlineMediaPlayback = true
        config.mediaTypesRequiringUserActionForPlayback = .all
        config.dataDetectorTypes = []
        let controller = config.userContentController
        controller.addUserScript(WKUserScript(source: LivePage.bootstrap, injectionTime: .atDocumentStart, forMainFrameOnly: true, in: Coordinator.world))
        // The controller keeps its handlers: a weak go-between, so it doesn't keep the coordinator.
        controller.add(WeakScriptHandler(coordinator), contentWorld: Coordinator.world, name: "trek")

        let web = WKWebView(frame: .zero, configuration: config)
        web.isOpaque = false
        web.backgroundColor = .clear
        web.scrollView.backgroundColor = .clear
        web.scrollView.isScrollEnabled = false
        web.scrollView.bounces = false
        web.scrollView.contentInsetAdjustmentBehavior = .never
        web.allowsLinkPreview = false
        web.allowsBackForwardNavigationGestures = false
        web.overrideUserInterfaceStyle = theme.dark ? .dark : .light
        web.navigationDelegate = coordinator
        web.uiDelegate = coordinator
        #if DEBUG
        web.isInspectable = true
        #endif
        coordinator.web = web
        coordinator.onHeight = onHeight
        coordinator.load(html, theme: theme)
        return web
    }

    func updateUIView(_ web: WKWebView, context: Context) {
        let coordinator = context.coordinator
        coordinator.onHeight = onHeight
        if html != coordinator.html {
            coordinator.load(html, theme: theme)
        } else if theme != coordinator.theme {
            coordinator.theme = theme
            web.overrideUserInterfaceStyle = theme.dark ? .dark : .light
            web.evaluateJavaScript(LivePage.themeUpdate(theme.css))
        }
    }

    static func dismantleUIView(_ web: WKWebView, coordinator: Coordinator) {
        web.stopLoading()
        web.configuration.userContentController.removeAllScriptMessageHandlers()
        web.navigationDelegate = nil
        web.uiDelegate = nil
    }

    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler {
        /// Where Trek's script runs and hears from: the page shares the document with it but not
        /// its variables, nor its message channel.
        static let world = WKContentWorld.world(name: "trek")

        weak var web: WKWebView?
        var onHeight: (CGFloat) -> Void = { _ in }
        private(set) var html = ""
        var theme = LivePageTheme(dark: true, fontSize: 15)
        /// The page's own load is under way and may go ahead; anything after it may not.
        private var loading = false
        private var heightChanges = 0
        private var height: CGFloat = 0

        func load(_ html: String, theme: LivePageTheme) {
            self.html = html
            self.theme = theme
            heightChanges = 0
            loading = true
            web?.loadHTMLString(LivePage.document(html, theme: theme), baseURL: nil)
        }

        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let body = message.body as? [String: Any], let kind = body["t"] as? String else { return }
            switch kind {
            case "h":
                guard let v = body["v"] as? Double, v.isFinite else { return }
                let h = min(max(CGFloat(v), LivePage.minHeight), LivePage.maxHeight)
                // Past the card's tallest, the page scrolls inside it.
                web?.scrollView.isScrollEnabled = v > LivePage.maxHeight
                guard abs(h - height) >= 1, heightChanges < LivePage.maxHeightChanges else { return }
                heightChanges += 1
                height = h
                onHeight(h)
            case "open":
                if let url = LivePage.external((body["u"] as? String).flatMap(URL.init(string:))) {
                    UIApplication.shared.open(url)
                }
            default:
                break
            }
        }

        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
            let scheme = action.request.url?.scheme?.lowercased() ?? ""
            if action.targetFrame?.isMainFrame != false {
                // Only the page's own document; a link that got past the script goes to Safari.
                if loading && scheme == "about" { return .allow }
                if action.navigationType == .linkActivated, let url = LivePage.external(action.request.url) {
                    await UIApplication.shared.open(url)
                }
                return .cancel
            }
            // Frames inside the page: only ones it made itself.
            return ["about", "blob", "data"].contains(scheme) ? .allow : .cancel
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            loading = false
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            loading = false
        }

        /// `target=_blank` and `window.open`: no new windows; a tapped link goes to Safari.
        func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                     for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
            if action.navigationType == .linkActivated, let url = LivePage.external(action.request.url) {
                UIApplication.shared.open(url)
            }
            return nil
        }

        /// The page crashed or was let go for memory: run it again.
        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            load(html, theme: theme)
        }
    }
}

/// Holds the coordinator weakly for the content controller, which keeps its handlers strongly.
private final class WeakScriptHandler: NSObject, WKScriptMessageHandler {
    weak var target: WKScriptMessageHandler?

    init(_ target: WKScriptMessageHandler) { self.target = target }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        target?.userContentController(controller, didReceive: message)
    }
}
