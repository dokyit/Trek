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
    /// Hand-holding tints (DESIGN.md): auto-accept edits sky, auto amber, full access red.
    static let sky = Color(light: 0x1F78B4, dark: 0x5DB7F2)
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

extension Access {
    /// The chip's tint: none for Supervised, then rising with what the agent may do unasked.
    var tint: Color? {
        switch self {
        case .supervised: nil
        case .autoAcceptEdits: Trek.sky
        case .auto: Trek.approval
        case .fullAccess: Trek.failed
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

/// A file's type as the Mac badges it (`file_icon::file_type`): a mark ("RS", "{}", a lock…), the
/// type's colour (none: neutral, for plain files and lockfiles), and whether the mark is dark.
nonisolated struct FileType: Hashable {
    enum Mark: Hashable { case text(String), lock, image, file, folder }
    var mark: Mark
    var fill: UInt32?
    var darkMark = false

    private static func text(_ label: String, _ fill: UInt32, dark: Bool = false) -> FileType {
        FileType(mark: .text(label), fill: fill, darkMark: dark)
    }
    static let plain = FileType(mark: .file, fill: nil)
    static let folder = FileType(mark: .folder, fill: nil)

    static let lockfiles: Set<String> = ["package-lock.json", "pnpm-lock.yaml", "npm-shrinkwrap.json", "go.sum"]

    /// The badge for `path`, from its name and extension.
    static func of(_ path: String) -> FileType {
        if path.hasSuffix("/") { return .folder }
        let name = (path.split(separator: "/").last.map(String.init) ?? path).lowercased()
        if name.hasSuffix(".lock") || lockfiles.contains(name) { return FileType(mark: .lock, fill: nil) }
        guard let dot = name.lastIndex(of: "."), dot != name.startIndex else {
            switch name {
            case "makefile", "dockerfile", "justfile": return text("$", 0x4E9A2E)
            default: return .plain
            }
        }
        switch name[name.index(after: dot)...] {
        case "rs": return text("RS", 0xC8572D)
        case "ts", "tsx", "mts", "cts": return text("TS", 0x3178C6)
        case "js", "jsx", "mjs", "cjs": return text("JS", 0xF0D23C, dark: true)
        case "py", "pyi": return text("PY", 0x3572A5)
        case "go": return text("GO", 0x00A3CC)
        case "swift": return text("SW", 0xF05138)
        case "kt", "kts": return text("KT", 0x7F52FF)
        case "java": return text("JV", 0xB07219)
        case "rb": return text("RB", 0xCC342D)
        case "md", "mdx", "markdown": return text("MD", 0x56677A)
        case "json", "jsonc", "json5": return text("{}", 0xD9B23A, dark: true)
        case "toml": return text("TM", 0x9C4A26)
        case "yaml", "yml": return text("YM", 0xB8466A)
        case "html", "htm": return text("<>", 0xE34C26)
        case "css", "scss", "sass", "less": return text("#", 0x6B45B5)
        case "sh", "bash", "zsh", "fish": return text("$", 0x4E9A2E)
        case "sql": return text("DB", 0x336791)
        case "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "heic", "bmp", "tiff", "avif":
            return FileType(mark: .image, fill: 0x8E6CC9)
        default: return .plain
        }
    }

    /// Whether inline code reads as a file or folder (and gets a file chip) rather than code: no
    /// spaces or call syntax, and a known type, a lockfile, or a path with a slash.
    static func looksLikePath(_ s: String) -> Bool {
        guard s.count > 1, s.count < 160, !s.contains(where: { $0.isWhitespace || "()[]{}=<>,;'\"|*$!?@`".contains($0) }) else { return false }
        if s.hasPrefix("-") || s.hasPrefix("http") || s.allSatisfy({ $0 == "." || $0 == "/" }) { return false }
        let t = of(s)
        if t.fill != nil { return true }
        let name = (s.split(separator: "/").last.map(String.init) ?? s).lowercased()
        if lockfiles.contains(name) || ["cargo.lock", "yarn.lock", "gemfile.lock", "poetry.lock", "flake.lock", "composer.lock", "bun.lock"].contains(name) { return true }
        // A path: `src/net/`, `crates/trek-app/src`, `./build` (not `a/b` fractions or `and/or`).
        if s.contains("/") {
            let parts = s.split(separator: "/")
            return s.hasSuffix("/") || s.hasPrefix("./") || s.hasPrefix("~/") || s.hasPrefix("/") || parts.count >= 3
                || parts.last?.contains(".") == true
        }
        return false
    }
}

/// A file chip's colours (`file_icon::tint_in`): a wash of the type's colour behind the name, a rim
/// of it, and the name in a shade of it that reads on Night and Paper. Plain files stay neutral.
struct FileTint: Hashable {
    var fill: Color
    var edge: Color
    var ink: Color

    static let neutral = FileTint(fill: Trek.foreground.opacity(0.06), edge: Trek.foreground.opacity(0.14), ink: Trek.foreground.opacity(0.9))

    static func of(_ path: String, projectHue: Int? = nil) -> FileTint {
        let type = FileType.of(path)
        if let fill = type.fill { return of(rgb: fill) }
        if type.mark == .folder, let hue = projectHue { return of(hue: Double(hue % 360) / 360, s: 0.55, l: 0.5) }
        return .neutral
    }

    static func of(rgb: UInt32) -> FileTint {
        let (h, s, l) = HSL.from(rgb)
        return of(hue: h, s: s, l: l)
    }

    /// `tint_in`, for both themes at once.
    static func of(hue h: Double, s: Double, l: Double) -> FileTint {
        // Yellows through cyans are bright for their shade: on paper their ink comes down further.
        let bright = (0.09..<0.55).contains(h)
        let inkS = min(s, 0.75)
        let inkLight = bright ? 0.26 : 0.34 - max(l - 0.5, 0) * 0.3
        func dyn(light: (Double, Double), dark: (Double, Double)) -> Color {
            Color(uiColor: UIColor { tc in
                let (ll, a) = tc.userInterfaceStyle == .dark ? dark : light
                return UIColor(hue: h, saturation: s, lightness: ll).withAlphaComponent(a)
            })
        }
        return FileTint(
            fill: dyn(light: (l, 0.11), dark: (l, 0.16)),
            edge: dyn(light: (l, 0.30), dark: (l, 0.38)),
            ink: Color(uiColor: UIColor { tc in
                UIColor(hue: h, saturation: inkS, lightness: tc.userInterfaceStyle == .dark ? 0.78 : inkLight)
            }))
    }
}

nonisolated enum HSL {
    /// Hue (0…1), saturation and lightness of an `0xRRGGBB` colour.
    static func from(_ rgb: UInt32) -> (Double, Double, Double) {
        let r = Double((rgb >> 16) & 0xFF) / 255, g = Double((rgb >> 8) & 0xFF) / 255, b = Double(rgb & 0xFF) / 255
        let mx = max(r, g, b), mn = min(r, g, b)
        let l = (mx + mn) / 2
        guard mx != mn else { return (0, 0, l) }
        let d = mx - mn
        let s = l > 0.5 ? d / (2 - mx - mn) : d / (mx + mn)
        var h: Double
        if mx == r { h = (g - b) / d + (g < b ? 6 : 0) } else if mx == g { h = (b - r) / d + 2 } else { h = (r - g) / d + 4 }
        h /= 6
        return (h, s, l)
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

    private static let time: DateFormatter = {
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate("jmm")
        return f
    }()
    private static let dayTime: DateFormatter = {
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate("EEEjmm")
        return f
    }()
    private static let dateTime: DateFormatter = {
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate("MMMdjmm")
        return f
    }()

    /// When something happened, as a message's footer says it: "10:42" today, "Mon 10:42" this
    /// week, else "3 Oct, 10:42".
    static func clock(_ ms: Int64, now: Date = .now) -> String {
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        let cal = Calendar.current
        if cal.isDate(date, inSameDayAs: now) { return time.string(from: date) }
        if now.timeIntervalSince(date) < 6 * 86_400 { return dayTime.string(from: date) }
        return dateTime.string(from: date)
    }

    static func duration(_ secs: Int) -> String {
        if secs < 60 { return "\(secs)s" }
        if secs < 3600 { return "\(secs / 60)m \(secs % 60)s" }
        return "\(secs / 3600)h \((secs % 3600) / 60)m"
    }
}
