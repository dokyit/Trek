import SwiftUI

@main
struct TrekApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .task { model.boot(demo: Launch.demo) }
                .onOpenURL { url in
                    if let link = PairingLink(url.absoluteString) { model.pair(address: link.address, code: link.code) }
                }
        }
    }
}

/// Launch arguments for demos and screenshots, e.g. `-TrekDemo YES -TrekOpen t-flaky -TrekAppearance dark`.
enum Launch {
    static var demo: Bool { UserDefaults.standard.bool(forKey: "TrekDemo") }
    static var open: String? { UserDefaults.standard.string(forKey: "TrekOpen") }
    static var tab: String? { UserDefaults.standard.string(forKey: "TrekTab") }
    static var sheet: String? { UserDefaults.standard.string(forKey: "TrekSheet") }
    static var appearance: String? { UserDefaults.standard.string(forKey: "TrekAppearance") }
    static var expandAll: Bool { UserDefaults.standard.bool(forKey: "TrekExpand") }
}

enum MainTab: Hashable {
    case sessions, settings, search
}

struct RootView: View {
    @Environment(AppModel.self) private var model
    @AppStorage("appearance") private var appearance = "system"

    var body: some View {
        Group {
            if model.mode == .unpaired {
                PairingView()
                    .transition(.opacity)
            } else {
                MainView()
                    .transition(.opacity)
            }
        }
        .animation(.smooth, value: model.mode)
        .overlay(alignment: .top) {
            if let toast = model.toast {
                ToastView(toast: toast)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .padding(.top, 6)
            }
        }
        .tint(Trek.foreground)
        .preferredColorScheme(scheme)
    }

    private var scheme: ColorScheme? {
        switch Launch.appearance ?? appearance {
        case "dark": .dark
        case "light": .light
        default: nil
        }
    }
}

struct MainView: View {
    @Environment(AppModel.self) private var model
    @State private var tab: MainTab = .sessions
    @State private var path: [String] = []
    @State private var showNew = false

    var body: some View {
        TabView(selection: $tab) {
            Tab("Sessions", systemImage: "bubble.left.and.text.bubble.right", value: MainTab.sessions) {
                SessionsView(path: $path, showNew: $showNew)
            }
            .badge(model.needsYouCount)
            Tab("Settings", systemImage: "gearshape", value: MainTab.settings) {
                SettingsView()
            }
            Tab(value: MainTab.search, role: .search) {
                SearchView()
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        .modifier(NewSessionAccessoryModifier(enabled: tab == .sessions && path.isEmpty) {
            NewSessionAccessory(working: model.workingCount, needsYou: model.needsYouCount) { showNew = true }
        })
        .sheet(isPresented: $showNew) {
            NewThreadSheet { tid in
                tab = .sessions
                path = [tid]
            }
        }
        .onAppear {
            switch Launch.tab {
            case "settings": tab = .settings
            case "search": tab = .search
            default: break
            }
            if let open = Launch.open { path = [open] }
            if Launch.sheet == "new" { showNew = true }
        }
    }
}

/// Shows the accessory on the Sessions list only (iOS 26.1+); on 26.0 it stays on everywhere.
struct NewSessionAccessoryModifier<Accessory: View>: ViewModifier {
    var enabled: Bool
    @ViewBuilder var accessory: () -> Accessory

    func body(content: Content) -> some View {
        if #available(iOS 26.1, *) {
            content.tabViewBottomAccessory(isEnabled: enabled, content: accessory)
        } else {
            content.tabViewBottomAccessory(content: accessory)
        }
    }
}

/// The glass pill above the tab bar: start a new session, and how many agents are at work.
struct NewSessionAccessory: View {
    var working: Int
    var needsYou: Int
    var action: () -> Void
    @Environment(\.tabViewBottomAccessoryPlacement) private var placement

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: "plus")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(Trek.foreground)
                Text("New session")
                    .font(.body.weight(.medium))
                    .foregroundStyle(Trek.foreground)
                Spacer(minLength: 8)
                if placement != .inline {
                    if needsYou > 0 {
                        Label("\(needsYou)", systemImage: "hand.raised.fill")
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(Trek.approval)
                            .labelStyle(.titleAndIcon)
                    }
                    if working > 0 {
                        HStack(spacing: 6) {
                            BeaconDot(color: Trek.working, pulses: true, size: 6)
                            Text("\(working) working").font(.footnote.weight(.medium)).foregroundStyle(Trek.muted)
                        }
                    }
                }
            }
            .padding(.horizontal, 18)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("New session. \(working) working, \(needsYou) need you.")
    }
}
