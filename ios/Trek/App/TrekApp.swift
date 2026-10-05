import SwiftUI

@main
struct TrekApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .task {
                    model.boot(demo: Launch.demo)
                    // Screenshots: show the confirmation for a link, as if it had been opened.
                    if let s = Launch.link, let link = PairingLink(s) { model.offer(link) }
                    #if DEBUG
                    // Scripted live screenshots only (ios/scripts/live-screenshots.sh): pair
                    // without the sheet. Debug builds, launch arguments: never from a link.
                    if let s = Launch.pairNow, let link = PairingLink(s) {
                        model.pair(address: link.address, code: link.code, transport: link.transport)
                    }
                    #endif
                }
                .onOpenURL { url in
                    // A notification from the Mac: open its thread.
                    if url.scheme == "trek", url.host == "open",
                       let thread = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == "thread" })?.value {
                        model.openRequest = thread
                        return
                    }
                    // Never pairs by itself: the link waits in a confirmation sheet.
                    if let link = PairingLink(url.absoluteString) {
                        model.offer(link)
                    } else if url.scheme == "trek" {
                        model.show("That pairing link is incomplete or damaged. Scan the code on your Mac again.", error: true)
                    }
                }
        }
    }
}

/// Launch arguments for demos and screenshots, e.g. `-TrekDemo YES -TrekOpen t-flaky -TrekAppearance dark`.
enum Launch {
    static var demo: Bool { UserDefaults.standard.bool(forKey: "TrekDemo") }
    static var open: String? { UserDefaults.standard.string(forKey: "TrekOpen") }
    /// Seconds to wait before opening `open`, as if tapped once the app has settled (to measure).
    static var openDelay: Double { UserDefaults.standard.double(forKey: "TrekOpenDelay") }
    /// `basecamp`, `notes`, `settings` or `search`.
    static var tab: String? { UserDefaults.standard.string(forKey: "TrekTab") }
    /// A note to open on the Notes tab.
    static var note: String? { UserDefaults.standard.string(forKey: "TrekNote") }
    static var sheet: String? { UserDefaults.standard.string(forKey: "TrekSheet") }
    static var appearance: String? { UserDefaults.standard.string(forKey: "TrekAppearance") }
    /// A text size for screenshots: `small`, `large`, `xxLarge`…
    static var textSize: String? { UserDefaults.standard.string(forKey: "TrekTextSize") }
    static var expandAll: Bool { UserDefaults.standard.bool(forKey: "TrekExpand") }
    static var link: String? { UserDefaults.standard.string(forKey: "TrekLink") }
    static var pairNow: String? { UserDefaults.standard.string(forKey: "TrekPair") }
}

enum MainTab: Hashable {
    case threads, basecamp, notes, settings, search
}

struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    @AppStorage(AppearanceKey.theme) private var appearance = ThemeChoice.system.rawValue
    @AppStorage(AppearanceKey.textSize) private var textSize = TextSizeChoice.system.rawValue
    @AppStorage(AppearanceKey.calm) private var calm = false
    @AppStorage(AppearanceKey.compact) private var compact = false

    var body: some View {
        @Bindable var model = model
        Group {
            #if DEBUG
            if UserDefaults.standard.bool(forKey: "TrekGallery") {
                DesignGallery()
            } else if model.mode == .unpaired {
                PairingView()
                    .transition(.opacity)
            } else {
                MainView()
                    .transition(.opacity)
            }
            #else
            if model.mode == .unpaired {
                PairingView()
                    .transition(.opacity)
            } else {
                MainView()
                    .transition(.opacity)
            }
            #endif
        }
        .animation(.smooth, value: model.mode)
        // Back in front: iOS shut the connection while the app was away, and waiting out a
        // retry (or a ping) before saying so would show stale threads as live.
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { model.foregrounded() }
        }
        .overlay(alignment: .top) {
            if let toast = model.toast {
                ToastView(toast: toast)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .padding(.top, 6)
            }
        }
        .tint(Trek.foreground)
        .sheet(item: $model.pendingLink) { link in
            PairingConfirmSheet(link: link)
        }
        .preferredColorScheme(scheme)
        .modifier(TextSizeModifier(size: TextSizeChoice(rawValue: Launch.textSize ?? textSize)?.dynamicType))
        .environment(\.trekCalm, calm)
        .environment(\.compactRows, compact)
    }

    private var scheme: ColorScheme? {
        ThemeChoice(rawValue: Launch.appearance ?? appearance)?.scheme
    }
}

/// Pins Dynamic Type to the size chosen in Settings, or leaves the iPhone's own.
private struct TextSizeModifier: ViewModifier {
    var size: DynamicTypeSize?

    func body(content: Content) -> some View {
        if let size { content.dynamicTypeSize(size) } else { content }
    }
}

struct MainView: View {
    @Environment(AppModel.self) private var model
    @State private var tab: MainTab = .threads
    @State private var path: [String] = []
    @State private var basecampPath: [BasecampRoute] = []
    @State private var notesPath: [String] = []
    @State private var showNew = false

    /// No "New thread" while the Mac can't be trusted or has refused this iPhone.
    private var canStartThreads: Bool {
        switch model.connection {
        case .identityChanged, .unauthorized: false
        default: true
        }
    }

    var body: some View {
        TabView(selection: $tab.timed("tab switch")) {
            Tab("Threads", systemImage: "bubble.left.and.text.bubble.right", value: MainTab.threads) {
                ThreadsView(path: $path, showNew: $showNew)
            }
            .badge(model.needsYouCount)
            Tab("Basecamp", systemImage: "mountain.2", value: MainTab.basecamp) {
                BasecampView(path: $basecampPath, showNew: $showNew)
            }
            Tab("Notes", systemImage: "note.text", value: MainTab.notes) {
                NotesView(path: $notesPath)
            }
            Tab("Settings", systemImage: "gearshape", value: MainTab.settings) {
                SettingsView()
            }
            Tab(value: MainTab.search, role: .search) {
                SearchView()
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        // The accessory stays on every tab. Shown on Threads alone, it left the bar to shrink to
        // its tabs on the others, and the bar resizing under the moving selection sent the
        // selection through Search on its way from Threads to Settings.
        .modifier(NewThreadAccessoryModifier(enabled: !inside && canStartThreads) {
            NewThreadAccessory(working: model.workingCount, needsYou: model.needsYouCount) { showNew = true }
        })
        .sheet(isPresented: $showNew) {
            NewThreadSheet { tid in
                tab = .threads
                path = [tid]
            }
        }
        .onAppear {
            switch Launch.tab {
            case "basecamp": tab = .basecamp
            case "notes": tab = .notes
            case "settings": tab = .settings
            case "search": tab = .search
            default: break
            }
            if let open = Launch.open {
                if Launch.openDelay > 0 {
                    // Measuring: open each thread listed in turn, `openDelay` seconds apart, back to
                    // the list in between (the first warms the app up; the later ones are typical).
                    for (i, tid) in open.split(separator: ",").map(String.init).enumerated() {
                        let at = Launch.openDelay * Double(i + 1)
                        DispatchQueue.main.asyncAfter(deadline: .now() + at - 1) { path = [] }
                        DispatchQueue.main.asyncAfter(deadline: .now() + at) {
                            Perf.navigated = Perf.now
                            path = [tid]
                        }
                    }
                } else {
                    path = [open]
                }
            }
            if let note = Launch.note {
                tab = .notes
                notesPath = [note]
            }
            if Launch.sheet == "new" { showNew = true }
            openRequested()
        }
        .onChange(of: model.openRequest) { openRequested() }
    }
}

extension MainView {
    /// In a thread or a note (the tab bar hides there, and the pill with it).
    fileprivate var inside: Bool {
        switch tab {
        case .threads: !path.isEmpty
        case .basecamp: basecampPath.contains { if case .thread = $0 { true } else { false } }
        case .notes: !notesPath.isEmpty
        case .settings, .search: false
        }
    }

    /// Bring up the thread a notification asked for.
    fileprivate func openRequested() {
        guard let id = model.openRequest else { return }
        model.openRequest = nil
        showNew = false
        tab = .threads
        path = [id]
    }
}

/// Shows the accessory on the Threads list only (iOS 26.1+); on 26.0 it stays on everywhere.
struct NewThreadAccessoryModifier<Accessory: View>: ViewModifier {
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

/// The glass pill above the tab bar: start a new thread, and how many agents are at work.
struct NewThreadAccessory: View {
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
                Text("New thread")
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
        .accessibilityLabel("New thread. \(working) working, \(needsYou) need you.")
    }
}
