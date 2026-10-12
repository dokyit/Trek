import SwiftUI

/// The agent's logo (the desktop's Lobe icons), rounded as the Mac rounds it, or the Mac's neutral
/// chip glyph for agents without one.
struct AgentGlyph: View {
    var key: String
    var size: CGFloat = 22

    init(key: String, size: CGFloat = 22) {
        self.key = key
        self.size = size
    }

    /// An agent's logo: the one the Mac names for it, else its key's.
    init(_ agent: AgentRef, size: CGFloat = 22) {
        self.init(key: agent.logo ?? agent.key, size: size)
    }

    var body: some View {
        Group {
            if let asset = AgentLogo.asset(key) {
                Image(asset).resizable().interpolation(.high).scaledToFit()
                    .clipShape(RoundedRectangle(cornerRadius: size * 0.22))
            } else {
                Image(systemName: "cpu").resizable().scaledToFit().fontWeight(.light).foregroundStyle(Trek.muted).padding(size * 0.06)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// Two-letter project badge in the project's hue (desktop `monogram_in`): a little wider than
/// tall, square-cornered, bold letters.
struct ProjectBadge: View {
    var project: ProjectRef
    var size: CGFloat = 16

    var body: some View {
        Text(project.monogram)
            .font(.ui(size: size * 0.59, weight: .bold))
            .foregroundStyle(ProjectColor.ink(project.hue))
            .padding(.horizontal, size * 0.19)
            .frame(minWidth: size * 1.25, minHeight: size, maxHeight: size)
            .background(ProjectColor.fill(project.hue), in: RoundedRectangle(cornerRadius: size * 0.25))
            .fixedSize()
            .accessibilityHidden(true)
    }
}

/// The beacon: a dot that breathes while an agent works (design spec: "beacon status dot").
struct BeaconDot: View {
    var color: Color
    var pulses: Bool
    var size: CGFloat = 7
    @State private var lit = false
    @MotionAllowed private var motion

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .opacity(pulses && motion ? (lit ? 1 : 0.4) : 1)
            .background {
                if pulses && motion {
                    Circle().fill(color.opacity(0.35)).frame(width: size * 2.2, height: size * 2.2).blur(radius: 3)
                        .opacity(lit ? 0.9 : 0.1)
                }
            }
            // Started whenever it's to pulse, not only if it was when the row first showed: a
            // thread that starts working with its row on screen had a dim dot that never moved.
            .onChange(of: pulses && motion, initial: true) { _, on in
                if on {
                    lit = false
                    withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) { lit = true }
                } else {
                    var still = Transaction()
                    still.disablesAnimations = true
                    withTransaction(still) { lit = false }
                }
            }
    }
}

/// "Working", "Approval", "Question"… as the Mac's sidebar shows a thread's state: a dot (breathing
/// while it works) and the word, both in Trek's status colour.
struct StatusPill: View {
    var look: StatusLook
    var compact = false

    var body: some View {
        HStack(spacing: 6) {
            BeaconDot(color: look.color, pulses: look.pulses, size: 7)
            if !compact {
                Text(look.label).font(.ui(.footnote).weight(.medium))
            }
        }
        .foregroundStyle(look.color)
        .fixedSize()
        .accessibilityElement(children: .combine)
    }
}

struct DiffStat: View {
    var additions: Int
    var deletions: Int
    var font: Font = .ui(.caption).monospacedDigit()

    var body: some View {
        HStack(spacing: 4) {
            Text("+\(additions)").foregroundStyle(Trek.additions)
            Text("−\(deletions)").foregroundStyle(Trek.deletions)
        }
        .font(font)
    }
}

/// A file's icon (`file_icon::badge`): Catppuccin's for its name or type, a folder's when the
/// path ends in "/".
struct FileBadge: View {
    var path: String
    var size: CGFloat = 14

    var body: some View {
        Image(FileIcons.any(path))
            .resizable()
            .interpolation(.high)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

/// A file in a chip of its type's colour (`file_icon::chip`): its badge, then its name in a shade
/// of the colour, on a wash of it with a rim. What tool rows show for the files they touched.
struct FileChip: View {
    var path: String
    var added: Int? = nil
    var removed: Int? = nil
    var size: CGFloat = 12.5
    @Environment(\.projectHue) private var projectHue
    @Environment(\.dynamicTypeSize) private var dynamicType

    var body: some View {
        let tint = FileTint.of(path, projectHue: projectHue)
        let s = size * TextScale.factor(dynamicType, style: .footnote)
        HStack(spacing: s * 0.4) {
            FileBadge(path: path, size: s * 1.1)
            Text(name)
                .font(.code(size: s, weight: .medium))
                .foregroundStyle(tint.ink)
                .lineLimit(1)
                .truncationMode(.middle)
            if let added, let removed, added + removed > 0 {
                DiffStat(additions: added, deletions: removed, font: .code(size: s * 0.88, weight: .medium))
            }
        }
        .padding(.leading, s * 0.38)
        .padding(.trailing, s * 0.6)
        .frame(minHeight: s * 1.85)
        .background(tint.fill, in: RoundedRectangle(cornerRadius: s * 0.48, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: s * 0.48, style: .continuous).strokeBorder(tint.edge, lineWidth: 0.75))
        .accessibilityElement(children: .combine)
    }

    private var name: String {
        let folder = path.hasSuffix("/")
        let last = ((folder ? String(path.dropLast()) : path) as NSString).lastPathComponent
        return folder ? last + "/" : last
    }
}

/// The ridge art behind the large titles, fading into the background (the desktop's backdrop), when
/// Settings › Appearance › Background art is on; `always` for the welcome screen. Otherwise the
/// plain background, as a new Mac shows it.
struct RidgeBackdrop: View {
    var height: CGFloat = 320
    var always = false
    @AppStorage(AppearanceKey.backgroundArt) private var art = false

    var body: some View {
        ZStack(alignment: .top) {
            Trek.background
            if art || always {
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
            Text(toast.text).font(.ui(.subheadline).weight(.medium)).lineLimit(2)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        .glassEffect(.regular, in: .capsule)
        .padding(.horizontal, 24)
    }
}
