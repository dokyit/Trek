import SwiftUI

/// What a paired machine is, as its `machine_info` said (`platform`): the phone marks each
/// machine's threads with it, so a Mac's and a Windows PC's read apart at a glance.
nonisolated enum MachineKind: Equatable, Sendable {
    case mac, windows, linux, unknown

    init(platform: String?) {
        switch platform?.lowercased() {
        case "macos", "mac", "darwin": self = .mac
        case "windows": self = .windows
        case "linux": self = .linux
        default: self = .unknown
        }
    }

    /// "Mac", "Windows", "Linux"; nil before the machine has said.
    var word: String? {
        switch self {
        case .mac: "Mac"
        case .windows: "Windows"
        case .linux: "Linux"
        case .unknown: nil
        }
    }

    /// Its colour: Windows blue for a PC, the foreground for a Mac (Apple's own mark is mono).
    @MainActor var tint: Color {
        switch self {
        case .windows: Color(hex: 0x3B8EEA)
        case .linux: Color(hex: 0xE0A030)
        case .mac, .unknown: Trek.foreground.opacity(0.85)
        }
    }
}

/// A machine's platform mark: the Apple logo, Windows' four panes, a terminal for Linux, a
/// laptop before the machine has said what it is.
struct MachineMark: View {
    var kind: MachineKind
    var size: CGFloat = 12

    var body: some View {
        Group {
            switch kind {
            case .mac: Image(systemName: "apple.logo").font(.system(size: size, weight: .semibold))
            case .windows: WindowsPanes().frame(width: size * 0.92, height: size * 0.92)
            case .linux: Image(systemName: "terminal").font(.system(size: size * 0.9, weight: .semibold))
            case .unknown: Image(systemName: "laptopcomputer").font(.system(size: size * 0.9, weight: .medium))
            }
        }
        .foregroundStyle(kind.tint)
        .frame(width: size + 2, height: size + 2)
        .accessibilityLabel(kind.word ?? "Machine")
    }
}

/// Windows' mark: four panes, a gap between them.
private struct WindowsPanes: View {
    var body: some View {
        GeometryReader { g in
            let gap = g.size.width * 0.1
            let side = (g.size.width - gap) / 2
            ZStack(alignment: .topLeading) {
                ForEach(0..<4, id: \.self) { i in
                    RoundedRectangle(cornerRadius: side * 0.12, style: .continuous)
                        .frame(width: side, height: side)
                        .offset(x: CGFloat(i % 2) * (side + gap), y: CGFloat(i / 2) * (side + gap))
                }
            }
        }
    }
}

/// A machine named with its mark, in a capsule washed with its colour: where a thread runs,
/// on its row and in the machine filter.
struct MachineTag: View {
    var name: String
    var kind: MachineKind
    var selected = false
    var compact = false

    var body: some View {
        HStack(spacing: 5) {
            MachineMark(kind: kind, size: compact ? 10 : 12)
            Text(compact ? Self.short(name) : name)
                .lineLimit(1)
        }
        .font(.ui(compact ? .caption : .subheadline).weight(.medium))
        .foregroundStyle(selected ? Trek.foreground : Trek.foreground.opacity(0.85))
        .padding(.horizontal, compact ? 7 : 11)
        .frame(height: compact ? 20 : 32)
        .background(kind.tint.opacity(selected ? 0.24 : (compact ? 0.12 : 0.1)), in: Capsule())
        .overlay(Capsule().strokeBorder(kind.tint.opacity(selected ? 0.55 : 0), lineWidth: 1))
        .fixedSize()
    }

    /// A machine's name for a row: without its owner ("Tobias's MacBook Pro" → "MacBook Pro"),
    /// which every machine of one person shares.
    static func short(_ name: String) -> String {
        for mark in ["’s ", "'s "] {
            if let r = name.range(of: mark), r.lowerBound > name.startIndex {
                let rest = name[r.upperBound...].trimmingCharacters(in: .whitespaces)
                if !rest.isEmpty { return rest }
            }
        }
        return name
    }
}
