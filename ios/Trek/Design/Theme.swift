import SwiftUI
import UIKit

// Trek's palette (docs/DESIGN.md): Night in dark mode, Paper in light. Ember is for the logo and
// for status, never for buttons; colour only where something wants action, is moving, or broke.

nonisolated extension Color {
    init(hex: UInt32, alpha: Double = 1) {
        self.init(.sRGB, red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255, opacity: alpha)
    }

    init(light: UInt32, dark: UInt32) {
        self.init(uiColor: UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: dark) : UIColor(hex: light) })
    }

    /// HSL like the desktop's `hsla()`, with a light and a dark lightness.
    static func hsl(_ hue: Double, _ s: Double, light: Double, dark: Double) -> Color {
        Color(uiColor: UIColor { tc in
            UIColor(hue: hue, saturation: s, lightness: tc.userInterfaceStyle == .dark ? dark : light)
        })
    }
}

nonisolated extension UIColor {
    convenience init(hex: UInt32) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }

    convenience init(hue: Double, saturation s: Double, lightness l: Double) {
        let c = (1 - abs(2 * l - 1)) * s
        let x = c * (1 - abs((hue * 6).truncatingRemainder(dividingBy: 2) - 1))
        let m = l - c / 2
        let (r, g, b): (Double, Double, Double) = switch Int(hue * 6) % 6 {
        case 0: (c, x, 0)
        case 1: (x, c, 0)
        case 2: (0, c, x)
        case 3: (0, x, c)
        case 4: (x, 0, c)
        default: (c, 0, x)
        }
        self.init(red: r + m, green: g + m, blue: b + m, alpha: 1)
    }
}

enum Trek {
    static let background = Color(light: 0xFAF7F2, dark: 0x0E0F12)
    static let surface = Color(light: 0xFFFFFF, dark: 0x171A1F)
    static let elevated = Color(light: 0xFFFFFF, dark: 0x1D2027)
    static let border = Color(light: 0xE4DDD1, dark: 0x262A32)
    static let foreground = Color(light: 0x17191F, dark: 0xE8ECF3)
    static let muted = Color(light: 0x6B6558, dark: 0x8A93A3)
    static let ember = Color(light: 0xE85D1F, dark: 0xFF7A3D)
    static let sunrise = LinearGradient(colors: [Color(hex: 0xFF4D2E), Color(hex: 0xFF8A3D), Color(hex: 0xFFC56B)],
                                        startPoint: .bottomLeading, endPoint: .topTrailing)

    // Status
    static let working = ember
    static let approval = Color(light: 0xC98500, dark: 0xFFB020)
    static let question = Color(light: 0x5B5BD6, dark: 0x8B8CFF)
    static let plan = Color(light: 0x7C4DDB, dark: 0xB48CFF)
    static let done = Color(light: 0x1E9E62, dark: 0x3FCF8E)
    static let failed = Color(light: 0xD93A3F, dark: 0xFF5A5F)
    static let additions = done
    static let deletions = failed

    static let mono = Font.system(.footnote, design: .monospaced)
}

// MARK: Thread status

/// How a thread's state shows in a row: what to say and in which colour.
struct StatusLook {
    var label: String
    var color: Color
    var symbol: String?
    var pulses: Bool

    static func of(_ t: ThreadSummary) -> StatusLook? {
        if let needs = t.needs {
            switch needs.kind {
            case .approval: return StatusLook(label: "Approval", color: Trek.approval, symbol: "hand.raised.fill", pulses: false)
            case .question: return StatusLook(label: "Question", color: Trek.question, symbol: "questionmark.bubble.fill", pulses: false)
            case .plan: return StatusLook(label: "Plan", color: Trek.plan, symbol: "list.bullet.clipboard.fill", pulses: false)
            case .failed: return StatusLook(label: "Failed", color: Trek.failed, symbol: "exclamationmark.triangle.fill", pulses: false)
            case .limit: return StatusLook(label: "Paused", color: Trek.approval, symbol: "clock.fill", pulses: false)
            }
        }
        switch t.runState {
        case .working: return StatusLook(label: "Working", color: Trek.working, symbol: nil, pulses: true)
        case .needsYou: return StatusLook(label: "Needs you", color: Trek.approval, symbol: "hand.raised.fill", pulses: false)
        case .failed: return StatusLook(label: "Failed", color: Trek.failed, symbol: "exclamationmark.triangle.fill", pulses: false)
        case .idle: return nil
        }
    }
}

// MARK: Projects

enum ProjectColor {
    /// The badge's letters (desktop `project_ink`).
    static func ink(_ hue: Int) -> Color { .hsl(Double(hue % 360) / 360, 0.55, light: 0.38, dark: 0.75) }
    /// The fill behind them (desktop `project_fill`).
    static func fill(_ hue: Int) -> Color { .hsl(Double(hue % 360) / 360, 0.35, light: 0.9, dark: 0.22) }
}

// MARK: Files

/// The colour and glyph a file chip gets from its extension.
enum FileType {
    static func look(_ path: String) -> (color: Color, symbol: String) {
        let ext = (path as NSString).pathExtension.lowercased()
        let name = (path as NSString).lastPathComponent.lowercased()
        switch ext {
        case "rs": return (Color(hex: 0xDE7A45), "gearshape.2")
        case "swift": return (Color(hex: 0xF05138), "swift")
        case "ts", "tsx": return (Color(hex: 0x3178C6), "curlybraces")
        case "js", "jsx", "mjs", "cjs": return (Color(hex: 0xD4A72C), "curlybraces")
        case "py": return (Color(hex: 0x3D7AB8), "chevron.left.forwardslash.chevron.right")
        case "go": return (Color(hex: 0x00ADD8), "chevron.left.forwardslash.chevron.right")
        case "md", "mdx", "txt": return (Color(light: 0x6B6558, dark: 0x8A93A3), "doc.text")
        case "json", "jsonc": return (Color(hex: 0x3FA66B), "curlybraces.square")
        case "toml", "yaml", "yml", "ini", "lock": return (Color(hex: 0x9C6ADE), "slider.horizontal.3")
        case "css", "scss": return (Color(hex: 0x7B61FF), "paintbrush")
        case "html", "htm": return (Color(hex: 0xE5532D), "chevron.left.forwardslash.chevron.right")
        case "sh", "zsh", "bash": return (Color(hex: 0x4EAA25), "terminal")
        case "sql": return (Color(hex: 0xC27C0E), "cylinder")
        case "png", "jpg", "jpeg", "gif", "svg", "webp": return (Color(hex: 0xD6407B), "photo")
        default:
            if name == "dockerfile" { return (Color(hex: 0x2496ED), "shippingbox") }
            if name.hasPrefix("cargo") || name == "makefile" { return (Color(hex: 0x9C6ADE), "slider.horizontal.3") }
            return (Color(light: 0x6B6558, dark: 0x8A93A3), "doc")
        }
    }
}

// MARK: Agents

enum AgentLogo {
    /// The asset for an `AgentId::key()`: `claude-code`, `acp:cursor`, `direct:openai`…
    static func asset(_ key: String) -> String? {
        let id: String
        if key.hasPrefix("acp:") { id = String(key.dropFirst(4)) } else if key.hasPrefix("direct:") { id = String(key.dropFirst(7)) } else { id = key }
        let known: Set<String> = ["amp", "anthropic", "claude-code", "codex", "cursor", "deepseek", "devin", "droid", "gemini",
                                  "github-copilot", "google", "goose", "grok", "groq", "kimi", "lmstudio", "mistral", "ollama",
                                  "openai", "opencode", "openrouter", "pi", "qwen-code", "xai"]
        let aliases = ["gemini-cli": "gemini", "copilot": "github-copilot", "qwen": "qwen-code", "factory": "droid"]
        let name = aliases[id] ?? id
        return known.contains(name) ? "logo-\(name)" : nil
    }
}

// MARK: Time

enum When {
    static func short(_ ms: Int64, now: Date = .now) -> String {
        let secs = max(0, now.timeIntervalSince1970 - Double(ms) / 1000)
        switch secs {
        case ..<60: return "now"
        case ..<3600: return "\(Int(secs / 60))m"
        case ..<86_400: return "\(Int(secs / 3600))h"
        case ..<(86_400 * 7): return "\(Int(secs / 86_400))d"
        default: return "\(Int(secs / (86_400 * 7)))w"
        }
    }

    static func duration(_ secs: Int) -> String {
        if secs < 60 { return "\(secs)s" }
        if secs < 3600 { return "\(secs / 60)m \(secs % 60)s" }
        return "\(secs / 3600)h \((secs % 3600) / 60)m"
    }
}
