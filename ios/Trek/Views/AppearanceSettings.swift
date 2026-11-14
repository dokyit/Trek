import SwiftUI
import UIKit

/// Settings › Appearance, kept on this iPhone: the Trek theme (or the Mac's), light or dark (Match
/// iPhone, Night, Paper, as the Mac names them), app icon, text size, background art (or the
/// Mac's), motion and row density.
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
    @AppStorage(AppearanceKey.trekTheme) private var trekTheme = AppearanceKey.matchMac
    @AppStorage(AppearanceKey.theme) private var theme = ThemeChoice.system.rawValue
    @AppStorage(AppearanceKey.textSize) private var textSize = TextSizeChoice.system.rawValue
    @AppStorage(AppearanceKey.calm) private var calm = false
    @AppStorage(AppearanceKey.compact) private var compact = false
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @Environment(AppModel.self) private var model
    @State private var icon = AppIconChoice.of(UIApplication.shared.alternateIconName)

    private static let steps: [TextSizeChoice] = [.small, .medium, .large, .xLarge, .xxLarge]

    private var size: TextSizeChoice { TextSizeChoice(rawValue: textSize) ?? .system }
    private var store: LookStore { LookStore.shared }
    /// The Mac's name, as the phone knows it.
    private var macName: String { model.selected?.name ?? "your Mac" }

    var body: some View {
        Section {
            LookRow(look: store.mac?.look ?? .trek, title: "Match your Mac", detail: macDetail,
                    selected: trekTheme == AppearanceKey.matchMac, mac: true) {
                pickTheme(AppearanceKey.matchMac, mode: nil)
            }
            ForEach(Look.builtIn) { look in
                LookRow(look: look, title: look.name, detail: look.description, selected: trekTheme == look.id) {
                    pickTheme(look.id, mode: look.mode)
                }
            }
        } header: {
            Text("Theme")
        } footer: {
            Text("The Mac's themes, drawn here: colours, fonts, corners and density. Match your Mac follows the one it has on, its own and its agents' themes too.")
                .font(.ui(.footnote))
        }

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
            Text("Light or dark")
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
            Text("Shown on the Home Screen. Night is Trek's own; Paper and Glass are the Mac's alternatives.")
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

        BackgroundSection(macName: macName)

        Section {
            Toggle(isOn: $calm) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Reduce motion")
                    Text(motionNote).font(.ui(.footnote)).foregroundStyle(Trek.muted)
                }
            }
            Toggle(isOn: $compact) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Compact threads")
                    Text(store.look.density == .compact ? "\(store.look.name) has them compact already" : "Fit more threads on the list")
                        .font(.ui(.footnote)).foregroundStyle(Trek.muted)
                }
            }
        } header: {
            Text("Motion and density")
        }
        .tint(Trek.done)
    }

    private var macDetail: String {
        guard let mac = store.mac else { return "Trek's own look until \(macName) says which it has on" }
        return "\(mac.theme.name) on \(macName)"
    }

    private var motionNote: String {
        if systemReduceMotion { return "iOS already reduces motion" }
        if store.look.motion == .still { return "\(store.look.name) holds still already" }
        return "The hiker stands still; nothing shimmers or breathes"
    }

    /// A theme picked: one that's dark or light puts the phone in it too, as on the Mac (Light
    /// or dark can change it back).
    private func pickTheme(_ id: String, mode: Look.Mode?) {
        withAnimation(.snappy(duration: 0.25)) {
            trekTheme = id
            switch mode {
            case .dark: theme = ThemeChoice.dark.rawValue
            case .light: theme = ThemeChoice.light.rawValue
            case .any, nil: break
            }
        }
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

/// A theme to pick: a little picture of a thread in its colours, corners and faces, its name in
/// its own interface face, and what it is.
private struct LookRow: View {
    var look: Look
    var title: String
    var detail: String
    var selected: Bool
    var mac = false
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 14) {
                LookMini(look: look, mac: mac)
                    .frame(width: 66, height: 50)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.custom(look.uiFont.family, size: 17, relativeTo: .body).weight(.semibold))
                        .foregroundStyle(Trek.foreground)
                    Text(detail)
                        .font(.ui(.footnote))
                        .foregroundStyle(Trek.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
                Image(systemName: "checkmark")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(Trek.foreground)
                    .opacity(selected ? 1 : 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(title). \(detail)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// A theme's picture: a row of a thread list and a line of an answer on its window, the composer
/// with its accent, in its corners; Match your Mac's has the Mac's laptop mark.
private struct LookMini: View {
    var look: Look
    var mac = false
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let dark = scheme == .dark
        let c = LookColors(look)
        let col = { (p: LookColors.Pair) in Color(hex: p.rgb(dark: dark)) }
        let r = 10 * CGFloat(look.corners)
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 4) {
                Circle().fill(col(c.ember)).frame(width: 5, height: 5)
                Capsule().fill(col(c.foreground).opacity(0.75)).frame(width: 26, height: 3)
            }
            Text("Aa")
                .font(.custom(look.conversationFont.family, fixedSize: 13))
                .foregroundStyle(col(c.foreground))
            Spacer(minLength: 0)
            HStack {
                Capsule().fill(col(c.muted).opacity(0.6)).frame(width: 18, height: 2.5)
                Spacer(minLength: 0)
                Circle().fill(col(c.ember)).frame(width: 5, height: 5)
            }
            .padding(.horizontal, 4)
            .frame(height: 10)
            .background(col(c.surface), in: RoundedRectangle(cornerRadius: min(5, 4 * CGFloat(look.corners)), style: .continuous))
        }
        .padding(6)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(col(c.background))
        .clipShape(RoundedRectangle(cornerRadius: r, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: r, style: .continuous).strokeBorder(col(c.border), lineWidth: 1))
        .overlay(alignment: .topTrailing) {
            if mac {
                Image(systemName: "laptopcomputer")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(col(c.muted))
                    .padding(5)
            }
        }
        .accessibilityHidden(true)
    }
}

/// Background art: the Mac's, none, or a piece of one of the Mac's four collections, and for the
/// phone's own Classic art where it shows; Dim for the phone's own.
private struct BackgroundSection: View {
    var macName: String
    @AppStorage(AppearanceKey.background) private var art = LookStore.artSetting(.standard)
    @AppStorage(AppearanceKey.placement) private var placement = ArtPlacement.newThread.rawValue
    @AppStorage(AppearanceKey.dim) private var dim = 0.35

    private var store: LookStore { LookStore.shared }
    private var own: ArtPiece? { ArtPiece.named(art) }

    var body: some View {
        Section {
            ArtRow(title: "Match your Mac", detail: macDetail, selected: art == AppearanceKey.matchMac) {
                choose(AppearanceKey.matchMac)
            }
            ArtRow(title: "None", detail: "The plain ground", selected: art == "none") { choose("none") }
        } header: {
            Text("Background")
        } footer: {
            Text("Art lies under a veil of the theme's ground, just enough that text keeps 4.5:1 against it.")
                .font(.ui(.footnote))
        }

        ForEach(ArtCollection.allCases, id: \.self) { collection in
            Section {
                ScrollView(.horizontal) {
                    HStack(spacing: 10) {
                        ForEach(ArtPiece.all.filter { $0.collection == collection }) { piece in
                            PieceTile(piece: piece, selected: own == piece) { choose(piece.spec) }
                        }
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                }
                .scrollIndicators(.hidden)
                .listRowInsets(EdgeInsets())
            } header: {
                Text(collection.title)
            } footer: {
                if let own, own.collection == collection, !own.credit.isEmpty {
                    Text("\(collection.note) \(own.credit)").font(.ui(.footnote))
                } else {
                    Text(collection.note).font(.ui(.footnote))
                }
            }
        }

        if let own {
            Section {
                if own.collection.placement == nil {
                    Picker("Where", selection: $placement) {
                        ForEach(ArtPlacement.allCases, id: \.rawValue) { p in Text(p.label).tag(p.rawValue) }
                    }
                    .pickerStyle(.segmented)
                }
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text("Dim")
                        Spacer()
                        Text("\(Int((dim * 100).rounded()))%").foregroundStyle(Trek.muted).monospacedDigit()
                    }
                    Slider(value: $dim, in: 0...0.9)
                        .accessibilityLabel("Dim")
                        .accessibilityValue("\(Int((dim * 100).rounded())) percent")
                }
            } header: {
                Text(own.name)
            } footer: {
                Text(placementNote(own)).font(.ui(.footnote))
            }
        }
    }

    private var macDetail: String {
        guard let mac = store.mac else { return "None until \(macName) says which it has" }
        guard let art = mac.art else { return "None on \(macName)" }
        return "\(art.piece.name) on \(macName)"
    }

    private func placementNote(_ piece: ArtPiece) -> String {
        let where_ = piece.collection.placement ?? ArtPlacement(rawValue: placement) ?? .newThread
        switch where_ {
        case .chat: return "Behind your threads and the chat."
        case .sidebar: return "Behind the thread list. Threads stay plain."
        case .newThread: return "At the top of your threads, Basecamp and Notes, and behind a new thread."
        }
    }

    private func choose(_ setting: String) {
        withAnimation(.snappy(duration: 0.25)) { art = setting }
    }
}

private struct ArtRow: View {
    var title: String
    var detail: String
    var selected: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).foregroundStyle(Trek.foreground)
                    Text(detail).font(.ui(.footnote)).foregroundStyle(Trek.muted)
                }
                Spacer()
                Image(systemName: "checkmark")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(Trek.foreground)
                    .opacity(selected ? 1 : 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// A piece to pick: its phone-shaped thumbnail (Night or Paper, as the phone is), its name under.
private struct PieceTile: View {
    var piece: ArtPiece
    var selected: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(piece.thumbnail)
                    .resizable()
                    .interpolation(.medium)
                    .scaledToFill()
                    .frame(width: 52, height: 104)
                    .clipShape(RoundedRectangle(cornerRadius: Trek.corner(9), style: .continuous))
                    .padding(3)
                    .overlay(RoundedRectangle(cornerRadius: Trek.corner(11), style: .continuous)
                        .strokeBorder(selected ? Trek.foreground : .clear, lineWidth: 2))
                Text(piece.name)
                    .font(.ui(.caption).weight(selected ? .semibold : .medium))
                    .foregroundStyle(selected ? Trek.foreground : Trek.muted)
                    .lineLimit(1)
            }
            .frame(width: 66)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(piece.name)
        .accessibilityAddTraits(selected ? .isSelected : [])
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

/// Light or dark with a little picture of it: a thread list in the theme's colours.
private struct ThemeCard: View {
    var choice: ThemeChoice
    var selected: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 8) {
                preview
                    .frame(height: 70)
                    .clipShape(RoundedRectangle(cornerRadius: Trek.corner(12), style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: Trek.corner(12), style: .continuous)
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
        let colors = LookStore.shared.colors
        switch choice {
        case .dark: Mini(colors: colors, dark: true)
        case .light: Mini(colors: colors, dark: false)
        case .system:
            Mini(colors: colors, dark: false)
                .overlay {
                    Mini(colors: colors, dark: true).mask {
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
    /// accent send dot, in the look's dark or light colours.
    private struct Mini: View {
        var colors: LookColors
        var dark: Bool

        var body: some View {
            let col = { (p: LookColors.Pair) in Color(hex: p.rgb(dark: dark)) }
            let bg = col(colors.background)
            let sidebar = col(colors.grouped)
            let border = col(colors.border)
            let muted = col(colors.muted)
            HStack(spacing: 0) {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(0..<3, id: \.self) { _ in Capsule().fill(muted.opacity(0.45)).frame(width: 11, height: 2.5) }
                    Spacer(minLength: 0)
                }
                .padding(.top, 8)
                .padding(.horizontal, 5)
                .frame(maxHeight: .infinity)
                .background(dark ? col(colors.surface) : sidebar)
                VStack(alignment: .leading, spacing: 4) {
                    Spacer(minLength: 0)
                    Capsule().fill(muted.opacity(0.6)).frame(width: 34, height: 3)
                    Capsule().fill(muted.opacity(0.45)).frame(width: 24, height: 3)
                    HStack {
                        Spacer(minLength: 0)
                        Circle().fill(col(colors.ember)).frame(width: 4, height: 4)
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

/// How an answer reads at this size and in this theme, file chips included.
private struct TextSample: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            RichText(markdown: "Moved the expiry check under the lock in `session.rs` and added a test to `Cargo.toml`.", size: 16)
                .lineSpacing(3)
                .environment(\.trekConversation, true)
            HStack(spacing: 6) {
                StatusPill(look: StatusLook(label: "Working", color: Trek.working, symbol: nil, pulses: false))
                StatusPill(look: StatusLook(label: "Approval", color: Trek.approval, symbol: "hand.raised.fill", pulses: false))
            }
        }
        .padding(.vertical, 4)
        .environment(\.projectHue, 24)
    }
}
