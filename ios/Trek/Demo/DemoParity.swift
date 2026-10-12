import UIKit

/// The demo Mac's side of what the phone gained to keep up with the Mac: Codex's granted resets,
/// queued follow-ups, a message's images, background tasks' output, and added agents' and
/// projects' icons.
extension DemoMac {
    /// An agent added from the ACP registry on the Mac: no logo of the app's own, an icon of its own.
    static let vibe = AgentRef(key: "acp:mistral-vibe", name: "Mistral Vibe", icon: "demo-vibe")
    static let addedAgents = [
        AgentOption(key: vibe.key, name: vibe.name, defaultModel: "devstral-2",
                    models: [ModelOption(id: "devstral-2", label: "Devstral 2")], icon: vibe.icon),
    ]

    /// The projects given an icon on the Mac.
    static func projectIcon(_ id: String) -> String? {
        switch id {
        case "p-atlas": "demo-compass"
        case "p-field": "demo-photo"
        default: nil
        }
    }

    /// Codex's granted resets not spent yet.
    static var resetCredits = [
        ResetCredit(id: "rc-1", title: "Full reset (Weekly + 5 hr)", expiresAt: now + 9 * 86_400_000),
    ]

    /// A drawn stand-in for a photo the user attached: a JPEG of a screen, a few hundred pixels.
    static func attachment(_ index: Int, maxPx: Int) -> (Data, Int, Int)? {
        let full = CGSize(width: 1170, height: 2532)
        let scale = min(1, CGFloat(maxPx) / max(full.width, full.height))
        let size = CGSize(width: (full.width * scale).rounded(), height: (full.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let image = UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            let hues: [(CGFloat, CGFloat)] = [(0.58, 0.72), (0.08, 0.95)]
            let (a, b) = hues[index % hues.count]
            let colors = [UIColor(hue: a, saturation: 0.55, brightness: 0.95, alpha: 1).cgColor,
                          UIColor(hue: b, saturation: 0.6, brightness: 0.55, alpha: 1).cgColor] as CFArray
            let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1])!
            ctx.cgContext.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: size.width, y: size.height), options: [])
            // A banner across the top, as the screenshot of an offline app would show it.
            let u = size.width / 390
            UIColor.black.withAlphaComponent(0.55).setFill()
            UIBezierPath(roundedRect: CGRect(x: 16 * u, y: 70 * u, width: size.width - 32 * u, height: 54 * u), cornerRadius: 14 * u).fill()
            let text = index == 0 ? "You're offline. Sends wait until you're back." : "Back online. 2 sends went out."
            (text as NSString).draw(in: CGRect(x: 30 * u, y: 87 * u, width: size.width - 60 * u, height: 30 * u),
                                    withAttributes: [.font: UIFont.systemFont(ofSize: 14 * u, weight: .semibold), .foregroundColor: UIColor.white])
            UIColor.white.withAlphaComponent(0.85).setFill()
            for row in 0..<6 {
                UIBezierPath(roundedRect: CGRect(x: 16 * u, y: (160 + CGFloat(row) * 92) * u, width: size.width - 32 * u, height: 76 * u),
                             cornerRadius: 16 * u).fill()
            }
        }
        guard let data = image.jpegData(compressionQuality: 0.82) else { return nil }
        return (data, Int(size.width), Int(size.height))
    }

    /// An icon as the Mac sends it: a 96 px PNG, a shape to tint (an SF Symbol stands in for the
    /// registry's SVG and Lucide's icons) or a picture.
    static func icon(_ hash: String) -> (Data, Bool)? {
        let side = CGSize(width: 96, height: 96)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        switch hash {
        case "demo-photo":
            // A picture the user chose: a dusk over hills.
            let image = UIGraphicsImageRenderer(size: side, format: format).image { ctx in
                let colors = [UIColor.systemIndigo.cgColor, UIColor.systemOrange.cgColor] as CFArray
                let g = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1])!
                ctx.cgContext.drawLinearGradient(g, start: .zero, end: CGPoint(x: 0, y: 96), options: [])
                UIColor(white: 0.12, alpha: 1).setFill()
                let hills = UIBezierPath()
                hills.move(to: CGPoint(x: 0, y: 96))
                hills.addLine(to: CGPoint(x: 0, y: 66))
                hills.addQuadCurve(to: CGPoint(x: 52, y: 60), controlPoint: CGPoint(x: 24, y: 44))
                hills.addQuadCurve(to: CGPoint(x: 96, y: 58), controlPoint: CGPoint(x: 76, y: 40))
                hills.addLine(to: CGPoint(x: 96, y: 96))
                hills.fill()
            }
            return image.pngData().map { ($0, false) }
        default:
            let symbol = hash == "demo-compass" ? "safari" : "wind"
            let config = UIImage.SymbolConfiguration(pointSize: 72, weight: .semibold)
            guard let mark = UIImage(systemName: symbol, withConfiguration: config) else { return nil }
            let image = UIGraphicsImageRenderer(size: side, format: format).image { _ in
                let fit = min(side.width / mark.size.width, side.height / mark.size.height)
                let size = CGSize(width: mark.size.width * fit, height: mark.size.height * fit)
                mark.withTintColor(.black).draw(in: CGRect(x: (96 - size.width) / 2, y: (96 - size.height) / 2, width: size.width, height: size.height))
            }
            return image.pngData().map { ($0, true) }
        }
    }

    /// How often a background task's output was asked for: it grows a little each time.
    static var backgroundReads = 0

    /// The end of a background task's output.
    static func backgroundOutput(_ task: String, tick: Int) -> String {
        switch task {
        case "bg-xcode":
            return """
            Test Suite 'All tests' started at 2026-10-12 14:02:11.204.
            Test Suite 'FieldnotesTests.xctest' started at 2026-10-12 14:02:11.205.
            Test Suite 'OfflineBannerTests' started at 2026-10-12 14:02:11.205.
            Test Case '-[OfflineBannerTests testShowsWhenUnreachable]' passed (0.012 seconds).
            Test Case '-[OfflineBannerTests testQueuesSends]' passed (0.031 seconds).
            Test Case '-[OfflineBannerTests testFlushesInOrder]' passed (0.019 seconds).
            Test Suite 'OfflineBannerTests' passed at 2026-10-12 14:02:11.270.
            """
        default:
            var lines = [
                "[Running 'cargo test -p trek-core']",
                "   Compiling trek-core v0.4.0 (/Users/tobias/Developer/Trek/crates/trek-core)",
                "    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.42s",
                "     Running unittests src/lib.rs (target/debug/deps/trek_core-3f9a1c2d)",
                "",
                "running 214 tests",
                "test inbox::snoozed_thread_raises_its_hand_on_approval ... ok",
                "test inbox::snoozed_thread_raises_its_hand_on_failure ... ok",
                "test inbox::snoozed_thread_stays_down_while_working ... ok",
                "test limit::pause_resumes_at_reset ... ok",
            ]
            for n in 0..<min(tick, 6) { lines.append("test store::transcript_round_trip_\(n) ... ok") }
            lines += ["", "test result: ok. 214 passed; 0 failed; 0 ignored; finished in 1.84s", "[Finished running. Exit status: 0]"]
            return lines.joined(separator: "\n")
        }
    }
}
