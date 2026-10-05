import SwiftUI

/// The agent's logo (the desktop's Lobe icons), or a neutral glyph for agents without one.
struct AgentGlyph: View {
    var key: String
    var size: CGFloat = 22

    var body: some View {
        Group {
            if let asset = AgentLogo.asset(key) {
                Image(asset).resizable().interpolation(.high).scaledToFit()
            } else {
                Image(systemName: "sparkle").resizable().scaledToFit().foregroundStyle(Trek.muted).padding(size * 0.12)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// Two-letter project badge in the project's hue (desktop `monogram_in`).
struct ProjectBadge: View {
    var project: ProjectRef
    var size: CGFloat = 16

    var body: some View {
        Text(project.monogram)
            .font(.system(size: size * 0.56, weight: .bold, design: .rounded))
            .foregroundStyle(ProjectColor.ink(project.hue))
            .frame(width: size, height: size)
            .background(ProjectColor.fill(project.hue), in: RoundedRectangle(cornerRadius: size * 0.28, style: .continuous))
            .accessibilityHidden(true)
    }
}

/// The beacon: a dot that breathes while an agent works (DESIGN.md "Beacon status dot").
struct BeaconDot: View {
    var color: Color
    var pulses: Bool
    var size: CGFloat = 7
    @State private var lit = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .opacity(pulses && !reduceMotion ? (lit ? 1 : 0.4) : 1)
            .background {
                if pulses {
                    Circle().fill(color.opacity(0.35)).frame(width: size * 2.2, height: size * 2.2).blur(radius: 3)
                        .opacity(lit ? 0.9 : 0.1)
                }
            }
            .onAppear {
                guard pulses, !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) { lit = true }
            }
    }
}

/// "Working", "Approval", "Question"… in Trek's status colours.
struct StatusPill: View {
    var look: StatusLook
    var compact = false

    var body: some View {
        HStack(spacing: 5) {
            if let symbol = look.symbol {
                Image(systemName: symbol).font(.system(size: 10, weight: .semibold))
            } else {
                BeaconDot(color: look.color, pulses: look.pulses, size: 6)
            }
            if !compact {
                Text(look.label).font(.subheadline.weight(.medium))
            }
        }
        .foregroundStyle(look.color)
        .accessibilityElement(children: .combine)
    }
}

struct DiffStat: View {
    var additions: Int
    var deletions: Int
    var font: Font = .caption.monospacedDigit()

    var body: some View {
        HStack(spacing: 4) {
            Text("+\(additions)").foregroundStyle(Trek.additions)
            Text("−\(deletions)").foregroundStyle(Trek.deletions)
        }
        .font(font)
    }
}

/// A file named in a tool row: its type's glyph and colour, then the file name.
struct FileChip: View {
    var path: String
    var added: Int? = nil
    var removed: Int? = nil

    var body: some View {
        let look = FileType.look(path)
        HStack(spacing: 5) {
            Image(systemName: look.symbol).font(.system(size: 10, weight: .semibold)).foregroundStyle(look.color)
            Text((path as NSString).lastPathComponent)
                .font(.system(.caption, design: .monospaced).weight(.medium))
                .foregroundStyle(Trek.foreground)
                .lineLimit(1)
            if let added, let removed, added + removed > 0 {
                DiffStat(additions: added, deletions: removed, font: .system(size: 10.5, design: .monospaced))
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(look.color.opacity(0.12), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(look.color.opacity(0.22), lineWidth: 0.5))
    }
}

/// The ridge art behind the large titles, fading into the background (the desktop's backdrop).
struct RidgeBackdrop: View {
    var height: CGFloat = 320

    var body: some View {
        ZStack(alignment: .top) {
            Trek.background
            Image("Backdrop")
                .resizable()
                .interpolation(.none)
                .scaledToFill()
                .frame(height: height)
                .clipped()
                .opacity(0.9)
                .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black.opacity(0.6), location: 0.55),
                                             .init(color: .clear, location: 1)], startPoint: .top, endPoint: .bottom))
        }
        .ignoresSafeArea()
    }
}

/// A floating message at the top: pairing done, an action that failed.
struct ToastView: View {
    var toast: Toast

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: toast.isError ? "exclamationmark.circle.fill" : "checkmark.circle.fill")
                .foregroundStyle(toast.isError ? Trek.failed : Trek.done)
            Text(toast.text).font(.subheadline.weight(.medium)).lineLimit(2)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        .glassEffect(.regular, in: .capsule)
        .padding(.horizontal, 24)
    }
}
