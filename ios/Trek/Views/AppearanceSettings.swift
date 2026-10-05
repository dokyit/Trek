import SwiftUI

/// Settings › Appearance, kept on this iPhone: theme (System, Night, Paper, as the Mac names them),
/// text size, motion and row density.
struct AppearanceSection: View {
    @AppStorage(AppearanceKey.theme) private var theme = ThemeChoice.system.rawValue
    @AppStorage(AppearanceKey.textSize) private var textSize = TextSizeChoice.system.rawValue
    @AppStorage(AppearanceKey.calm) private var calm = false
    @AppStorage(AppearanceKey.compact) private var compact = false
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion

    private static let steps: [TextSizeChoice] = [.small, .medium, .large, .xLarge, .xxLarge]

    private var size: TextSizeChoice { TextSizeChoice(rawValue: textSize) ?? .system }

    var body: some View {
        Section {
            HStack(spacing: 10) {
                ForEach(ThemeChoice.allCases) { choice in
                    ThemeCard(choice: choice, selected: theme == choice.rawValue) {
                        withAnimation(.snappy(duration: 0.25)) { theme = choice.rawValue }
                    }
                }
            }
            .padding(.vertical, 6)
            .listRowInsets(EdgeInsets(top: 8, leading: 14, bottom: 8, trailing: 14))
        } header: {
            Text("Appearance")
        }

        Section {
            Toggle("Match iPhone", isOn: Binding(
                get: { size == .system },
                set: { textSize = ($0 ? TextSizeChoice.system : .large).rawValue }))
                .tint(Trek.done)
            if size != .system {
                VStack(spacing: 8) {
                    HStack(spacing: 14) {
                        Image(systemName: "textformat.size.smaller").foregroundStyle(Trek.muted)
                        Slider(value: Binding(
                            get: { Double(Self.steps.firstIndex(of: size) ?? 2) },
                            set: { textSize = Self.steps[Int($0.rounded())].rawValue }), in: 0...Double(Self.steps.count - 1), step: 1)
                        Image(systemName: "textformat.size.larger").foregroundStyle(Trek.muted)
                    }
                    Text(size.label).font(.footnote.weight(.medium)).foregroundStyle(Trek.muted)
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Text size")
                .accessibilityValue(size.label)
            }
            TextSample()
        } header: {
            Text("Text size")
        }

        Section {
            Toggle(isOn: $calm) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Reduce motion")
                    Text(systemReduceMotion ? "iOS already reduces motion" : "The hiker stands still; nothing shimmers or breathes")
                        .font(.footnote).foregroundStyle(Trek.muted)
                }
            }
            Toggle(isOn: $compact) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Compact threads")
                    Text("Fit more threads on the list").font(.footnote).foregroundStyle(Trek.muted)
                }
            }
        } header: {
            Text("Motion and density")
        }
        .tint(Trek.done)
    }
}

/// A theme with a little picture of it: a thread list in its colours.
private struct ThemeCard: View {
    var choice: ThemeChoice
    var selected: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 8) {
                preview
                    .frame(height: 70)
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(selected ? Trek.foreground : Trek.border, lineWidth: selected ? 2 : 0.75))
                HStack(spacing: 4) {
                    if selected { Image(systemName: "checkmark").font(.system(size: 11, weight: .bold)) }
                    Text(choice.label)
                }
                .font(.footnote.weight(selected ? .semibold : .medium))
                .foregroundStyle(selected ? Trek.foreground : Trek.muted)
            }
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(choice.label)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    @ViewBuilder
    private var preview: some View {
        switch choice {
        case .dark: Mini(dark: true)
        case .light: Mini(dark: false)
        case .system:
            Mini(dark: false)
                .overlay {
                    Mini(dark: true).mask {
                        GeometryReader { g in
                            Path { p in
                                p.move(to: CGPoint(x: g.size.width * 0.62, y: 0))
                                p.addLine(to: CGPoint(x: g.size.width, y: 0))
                                p.addLine(to: CGPoint(x: g.size.width, y: g.size.height))
                                p.addLine(to: CGPoint(x: g.size.width * 0.38, y: g.size.height))
                            }
                        }
                    }
                }
        }
    }

    /// Two thread rows in Night or Paper: a logo, a title, a line of detail, a status dot.
    private struct Mini: View {
        var dark: Bool

        var body: some View {
            let bg = Color(hex: dark ? 0x0E0F12 : 0xFAF7F2)
            let fg = Color(hex: dark ? 0xE8ECF3 : 0x17191F)
            let muted = Color(hex: dark ? 0x8A93A3 : 0x6B6558)
            VStack(alignment: .leading, spacing: 8) {
                ForEach(0..<2, id: \.self) { i in
                    HStack(spacing: 5) {
                        Circle().fill(i == 0 ? Color(hex: 0xD97757) : muted.opacity(0.5)).frame(width: 8, height: 8)
                        VStack(alignment: .leading, spacing: 3) {
                            Capsule().fill(fg.opacity(0.85)).frame(width: i == 0 ? 38 : 30, height: 4)
                            Capsule().fill(muted.opacity(0.6)).frame(width: i == 0 ? 26 : 34, height: 3)
                        }
                        Spacer(minLength: 0)
                        if i == 0 { Circle().fill(Color(hex: dark ? 0xFF7A3D : 0xE85D1F)).frame(width: 5, height: 5) }
                    }
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(bg)
        }
    }
}

/// How an answer reads at this size, file chips included.
private struct TextSample: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            RichText(markdown: "Moved the expiry check under the lock in `session.rs` and added a test to `Cargo.toml`.", size: 16)
                .lineSpacing(3)
            HStack(spacing: 6) {
                StatusPill(look: StatusLook(label: "Working", color: Trek.working, symbol: nil, pulses: false))
                StatusPill(look: StatusLook(label: "Approval", color: Trek.approval, symbol: "hand.raised.fill", pulses: false))
            }
        }
        .padding(.vertical, 4)
        .environment(\.projectHue, 24)
    }
}
