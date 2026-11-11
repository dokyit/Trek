import SwiftUI

/// What opens a thread's side chat: `/side` (what followed it is the first thing asked there, and
/// goes at once) or the thread's menu.
struct SideChatAsk: Identifiable {
    let id = UUID()
    var question = ""
    /// Photos that were on the message `/side` was typed in.
    var photos: [PickedPhoto] = []

    var isEmpty: Bool { question.isEmpty && photos.isEmpty }
}

/// What a thread's screen does differently in a side chat's sheet: its bar closes the sheet and
/// starts another side chat.
struct SideChatHost {
    var close: () -> Void
    var startNew: () -> Void
}

/// A side chat as a sheet over its thread: the Mac's Side Chat, a quick question in a session of
/// its own that the thread's agent doesn't hear. The thread's newest side chat is found again; with
/// none, the first message starts one. Under the sheet the thread stays as it was (its place in
/// the transcript, its draft, its turn), and closing the sheet is all it takes to be back there.
struct SideChatSheet: View {
    /// The thread it's beside (qualified).
    var parent: String
    var ask: SideChatAsk
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var phase: SideChatPhase = .finding
    @State private var opened = false
    /// The draft is one the user asked for ("New side chat"), not for want of a chat.
    @State private var fresh = false
    /// The chat left for a new one: the draft offers the way back.
    @State private var previous: String?
    @State private var composer = ComposerState()
    /// The first message is on its way to the Mac.
    @State private var starting = false
    /// What came with `/side`, until the Mac has said where it goes.
    @State private var waiting: SideChatAsk?

    var body: some View {
        NavigationStack {
            content
        }
        // What goes wrong here is said here: the app's own toast is under the sheet.
        .overlay(alignment: .top) { ToastOverlay(feels: false).padding(.top, 4) }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .sensoryFeedback(.impact(weight: .light), trigger: starting) { _, now in now }
        .onAppear(perform: open)
    }

    @ViewBuilder
    private var content: some View {
        switch phase {
        case .chat(let id):
            ThreadView(threadID: id, side: SideChatHost(close: { dismiss() }, startNew: startNew))
        case .draft:
            SideChatDraft(parent: parent, composer: composer, starting: starting,
                          previous: previous.flatMap { model.thread($0) },
                          back: { if let previous { withAnimation(.smooth(duration: 0.25)) { fresh = false; phase = .chat(previous) } } },
                          close: { dismiss() }, send: start)
        case .finding:
            ProgressView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Trek.background)
                .modifier(SideChatBar(parent: parent, close: { dismiss() }))
                .accessibilityLabel("Looking for this thread's side chat")
        case .unavailable(let why):
            ContentUnavailableView("No side chat", systemImage: "bubble.left.and.bubble.right", description: Text(why))
                .background(Trek.background)
                .modifier(SideChatBar(parent: parent, close: { dismiss() }))
        }
    }

    /// The side chat this thread was last found with shows at once; the Mac says whether it's
    /// still the one. Then what came with `/side` goes.
    private func open() {
        guard !opened else { return }
        opened = true
        if !ask.isEmpty { waiting = ask }
        if let known = model.sideChats[parent] { phase = .chat(known) }
        model.findSideChat(of: parent) { result in
            switch result {
            case .success(let found):
                phase = phase.found(found, fresh: fresh)
                deliverWaiting()
            case .failure(let why):
                let was = phase
                phase = phase.failed(why.message)
                // On a chat or a draft there's somewhere to say it; the question waits in the
                // composer of a draft, not lost.
                if was != .finding { model.show(why.message, error: true) }
                if let w = waiting, phase == .draft { composer.put(w.question); composer.photos = w.photos }
                waiting = nil
            }
        }
    }

    /// What came with `/side`: into the side chat there is, or as the first message of a new one.
    private func deliverWaiting() {
        guard let w = waiting else { return }
        waiting = nil
        switch phase {
        case .chat(let id):
            let uploads = w.photos.compactMap(\.upload)
            let words = w.question.isEmpty ? MessageFiles.caption(photos: uploads.count, videos: 0, files: 0) : w.question
            model.send(words, to: id, mode: nil, images: uploads)
        case .draft:
            composer.put(w.question)
            composer.photos = w.photos
            start()
        case .finding, .unavailable:
            break
        }
    }

    /// The first message starts the side chat; the sheet turns into it.
    private func start() {
        let text = composer.draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let uploads = composer.photos.compactMap(\.upload)
        guard !starting, !text.isEmpty || !uploads.isEmpty else { return }
        guard model.link(for: parent)?.connection == .connected else {
            model.show(AppModel.notConnected, error: true)
            return
        }
        starting = true
        let words = text.isEmpty ? MessageFiles.caption(photos: uploads.count, videos: 0, files: 0) : text
        model.startSideChat(beside: parent, text: words, images: uploads) { result in
            starting = false
            switch result {
            case .success(let id?):
                composer.cleared(after: composer.draft)
                fresh = false
                withAnimation(.smooth(duration: 0.25)) { phase = .chat(id) }
            case .success(nil):
                model.show("The side chat didn't start.", error: true)
            case .failure(let why):
                // What was typed stays in the composer.
                model.show(why.message, error: true)
            }
        }
    }

    private func startNew() {
        if case .chat(let id) = phase { previous = id }
        fresh = true
        withAnimation(.smooth(duration: 0.25)) { phase = .draft }
    }
}

/// The sheet's bar while there's no thread to draw one: the title, the thread it's beside, Done.
private struct SideChatBar: ViewModifier {
    var parent: String
    var close: () -> Void
    @Environment(AppModel.self) private var model

    func body(content: Content) -> some View {
        content
            .navigationTitle("Side chat")
            .navigationSubtitle(model.thread(parent).map { "Beside “\($0.title)”" } ?? "")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done", systemImage: "xmark", action: close)
                }
            }
    }
}

/// A side chat not started yet: what it is, and the composer whose first message starts it.
private struct SideChatDraft: View {
    var parent: String
    let composer: ComposerState
    var starting: Bool
    /// The side chat left for this new one.
    var previous: ThreadSummary?
    var back: () -> Void
    var close: () -> Void
    var send: () -> Void
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var c = composer
        let thread = model.thread(parent)
        let agent = thread?.agent.name ?? "The agent"
        ScrollView {
            VStack(spacing: 12) {
                Image(systemName: "bubble.left.and.bubble.right")
                    .font(.system(size: 30, weight: .regular))
                    .foregroundStyle(Trek.muted)
                    .padding(.bottom, 2)
                    .accessibilityHidden(true)
                Text("Ask beside the thread")
                    .font(.ui(.title3).weight(.semibold))
                    .foregroundStyle(Trek.foreground)
                Text(SideChatDraftWords.blurb(agent: agent, project: thread?.project?.name))
                    .font(.ui(.subheadline))
                    .foregroundStyle(Trek.muted)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                if let previous {
                    Button(action: back) {
                        // A long title gives way at its end, and keeps its closing quote.
                        HStack(spacing: 0) {
                            Image(systemName: "arrow.uturn.backward")
                                .font(.system(size: 13, weight: .semibold))
                                .padding(.trailing, 8)
                            Text("Back to “\(previous.title)")
                                .lineLimit(1)
                                .truncationMode(.tail)
                            Text("”").fixedSize()
                        }
                            .font(.ui(.subheadline).weight(.medium))
                            .foregroundStyle(Trek.foreground)
                            .padding(.horizontal, 14)
                            .frame(minHeight: 40)
                            .background(Trek.foreground.opacity(0.07), in: Capsule())
                            .contentShape(Capsule())
                    }
                    .buttonStyle(.press)
                    .padding(.top, 6)
                    .accessibilityLabel("Back to \(previous.title)")
                    .accessibilityHint("The side chat you were in")
                }
            }
            .padding(.horizontal, 32)
            .padding(.top, 56)
            .frame(maxWidth: .infinity)
        }
        .scrollBounceBehavior(.basedOnSize)
        .scrollDismissesKeyboard(.interactively)
        .background(Trek.background)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            Composer(text: $c.draft, mode: .constant(.steer), photos: $c.photos, files: .constant([]),
                     placeholder: "Ask \(thread?.agent.name ?? "the agent")", working: false, uploads: false, uploading: starting,
                     focusRequest: composer.focusRequest, send: send, stop: {})
                .padding(.horizontal, 14)
                .padding(.bottom, 6)
                .padding(.top, 8)
        }
        .modifier(SideChatBar(parent: parent, close: close))
        // It's here to be typed in: the keyboard comes up once the sheet has.
        .task {
            try? await Task.sleep(for: .seconds(0.45))
            if !Task.isCancelled, !starting { composer.focusRequest += 1 }
        }
    }
}

nonisolated enum SideChatDraftWords {
    /// What a side chat is, in a sentence or two, naming the thread's agent and project.
    static func blurb(agent: String, project: String?) -> String {
        // A project's name stays in one piece: no line ends at the hyphen in "trek-api".
        let place = project.map { " in \($0.replacingOccurrences(of: "-", with: "\u{2060}-\u{2060}"))" } ?? ""
        return "A side chat has a session of its own\(place), so \(agent) carries on in the thread and hears none of it. It asks before every edit and command."
    }
}
