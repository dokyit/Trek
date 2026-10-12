import SwiftUI
import UIKit

/// Settings › Appearance, kept on this iPhone: theme (Match iPhone, Night, Paper, as the Mac names
/// them), app icon, text size, background art, motion and row density.
struct AppearanceView: View {
    var body: some View {
        Form {
            AppearanceSection()
        }
        .scrollContentBackground(.hidden)
        .background(Trek.grouped)
        .navigationTitle("Appearance")
        .navigationBarTitleDisplayMode(.inline)
    }
}

struct AppearanceSection: View {
    @AppStorage(AppearanceKey.theme) private var theme = ThemeChoice.system.rawValue
    @AppStorage(AppearanceKey.textSize) private var textSize = TextSizeChoice.system.rawValue
    @AppStorage(AppearanceKey.calm) private var calm = false
    @AppStorage(AppearanceKey.compact) private var compact = false
    @AppStorage(AppearanceKey.backgroundArt) private var art = false
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @Environment(AppModel.self) private var model
    @State private var icon = AppIconChoice.of(UIApplication.shared.alternateIconName)

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
            Text("Theme")
        }

        Section {
            HStack(spacing: 0) {
                ForEach(AppIconChoice.allCases) { choice in
                    IconCard(choice: choice, selected: icon == choice) { pick(choice) }
                        .frame(maxWidth: .infinity)
                }
            }
            .padding(.vertical, 6)
            .listRowInsets(EdgeInsets(top: 8, leading: 14, bottom: 8, trailing: 14))
        } header: {
            Text("App icon")
        } footer: {
            Text("Shown on the Home Screen. Ember is Trek's own; Night and Glass are the Mac's alternatives.")
                .font(.ui(.footnote))
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
                    Text(size.label).font(.ui(.footnote).weight(.medium)).foregroundStyle(Trek.muted)
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
            Toggle(isOn: $art) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Background art")
                    Text("The ridge at dusk behind your threads, Basecamp and a new thread").font(.ui(.footnote)).foregroundStyle(Trek.muted)
                }
            }
            Toggle(isOn: $calm) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Reduce motion")
                    Text(systemReduceMotion ? "iOS already reduces motion" : "The hiker stands still; nothing shimmers or breathes")
                        .font(.ui(.footnote)).foregroundStyle(Trek.muted)
                }
            }
            Toggle(isOn: $compact) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Compact threads")
                    Text("Fit more threads on the list").font(.ui(.footnote)).foregroundStyle(Trek.muted)
                }
            }
        } header: {
            Text("Background, motion and density")
        }
        .tint(Trek.done)
    }

    /// iOS confirms the change itself, with an alert of its own.
    private func pick(_ choice: AppIconChoice) {
        guard choice != icon, UIApplication.shared.supportsAlternateIcons else { return }
        let before = icon
        withAnimation(.snappy(duration: 0.2)) { icon = choice }
        UIApplication.shared.setAlternateIconName(choice.iconName) { error in
            guard let error else { return }
            Task { @MainActor in
                withAnimation(.snappy(duration: 0.2)) { icon = before }
                model.show("Couldn't change the icon: \(error.localizedDescription)", error: true)
            }
        }
    }
}

/// An app icon to pick: the icon, its name under it, a ring round the chosen one.
private struct IconCard: View {
    var choice: AppIconChoice
    var selected: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 8) {
                Image(choice.preview)
                    .resizable()
                    .interpolation(.high)
                    .frame(width: 58, height: 58)
                    .clipShape(RoundedRectangle(cornerRadius: 13.5, style: .continuous))
                    .padding(4)
                    .overlay(RoundedRectangle(cornerRadius: 17, style: .continuous)
                        .strokeBorder(selected ? Trek.foreground : .clear, lineWidth: 2))
                Text(choice.label)
                    .font(.ui(.footnote).weight(selected ? .semibold : .medium))
                    .foregroundStyle(selected ? Trek.foreground : Trek.muted)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(choice.label) icon")
        .accessibilityAddTraits(selected ? .isSelected : [])
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
                .font(.ui(.footnote).weight(selected ? .semibold : .medium))
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
                                p.addRect(CGRect(x: g.size.width / 2, y: 0, width: g.size.width / 2, height: g.size.height))
                            }
                        }
                    }
                }
        }
    }

    /// The Mac's picture of a theme: a sidebar, two lines of an answer, and the composer with its
    /// ember send dot, in Night's or Paper's colours.
    private struct Mini: View {
        var dark: Bool

        var body: some View {
            let bg = Color(hex: dark ? 0x0A0A0B : 0xFFFFFF)
            let sidebar = Color(hex: dark ? 0x111113 : 0xF6F5F3)
            let border = Color(hex: dark ? 0x232327 : 0xE7E5E1)
            let muted = Color(hex: dark ? 0x8B8B94 : 0x6E6C68)
            HStack(spacing: 0) {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(0..<3, id: \.self) { _ in Capsule().fill(muted.opacity(0.45)).frame(width: 11, height: 2.5) }
                    Spacer(minLength: 0)
                }
                .padding(.top, 8)
                .padding(.horizontal, 5)
                .frame(maxHeight: .infinity)
                .background(sidebar)
                VStack(alignment: .leading, spacing: 4) {
                    Spacer(minLength: 0)
                    Capsule().fill(muted.opacity(0.6)).frame(width: 34, height: 3)
                    Capsule().fill(muted.opacity(0.45)).frame(width: 24, height: 3)
                    HStack {
                        Spacer(minLength: 0)
                        Circle().fill(Color(hex: dark ? 0xFF7A3D : 0xE85D1F)).frame(width: 4, height: 4)
                    }
                    .padding(.horizontal, 4)
                    .frame(height: 10)
                    .overlay(RoundedRectangle(cornerRadius: 3).strokeBorder(border, lineWidth: 0.75))
                    .padding(.top, 4)
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 12)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                .background(bg)
            }
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
