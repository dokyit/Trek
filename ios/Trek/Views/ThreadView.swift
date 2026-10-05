import SwiftUI

struct ThreadView: View {
    var threadID: String
    @Environment(AppModel.self) private var model
    @State private var draft = ""
    @State private var mode: SendMode = .steer
    @State private var groupOpen: [String: Bool] = [:]

    private var thread: ThreadSummary? { model.thread(threadID) }
    private var items: [TItem] { model.transcripts[threadID] ?? [] }

    var body: some View {
        let blocks = Block.build(items)
        let lastGroup = blocks.last { if case .group = $0 { true } else { false } }?.id
        let working = thread?.runState == .working
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                if !model.loadedTranscripts.contains(threadID) {
                    ProgressView().frame(maxWidth: .infinity).padding(.top, 80)
                }
                ForEach(blocks) { block in
                    blockView(block, isLatestGroup: block.id == lastGroup && working)
                        .id(block.id)
                }
                if working, let t = thread {
                    WorkingLine(since: t.workingSince, activity: t.activity)
                        .padding(.top, 2)
                        .transaction { $0.animation = nil }
                }
                Color.clear.frame(height: 4).id("end")
            }
            .padding(.horizontal, 20)
            .padding(.top, 12)
            .padding(.bottom, 16)
        }
        .defaultScrollAnchor(.bottom)
        .scrollDismissesKeyboard(.interactively)
        .background(Trek.background)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 10) {
                ForEach(items.filter(\.isPendingRequest)) { item in
                    PendingRequestCard(item: item, agentName: thread?.agent.name ?? "The agent") { response in
                        if let rid = requestId(item) { model.answer(threadID, requestId: rid, response) }
                    }
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                Composer(text: $draft, mode: $mode, placeholder: "Message \(thread?.agent.name ?? "the agent")",
                         working: working, send: send, stop: { model.interrupt(threadID) })
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 6)
            .padding(.top, 8)
            .animation(.smooth(duration: 0.3), value: items.filter(\.isPendingRequest).map(\.id))
        }
        .navigationTitle(thread?.title ?? "Thread")
        .navigationSubtitle(subtitle)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarVisibility(.hidden, for: .tabBar)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    if let t = thread {
                        Section {
                            Label(t.agent.name + (t.modelLabel.map { " · \($0)" } ?? ""), systemImage: "cpu")
                            if let b = t.branch { Label(b, systemImage: "arrow.triangle.branch") }
                            if t.additions + t.deletions > 0 { Label("+\(t.additions) −\(t.deletions)", systemImage: "plus.forwardslash.minus") }
                        }
                    }
                    Button("Copy title", systemImage: "doc.on.doc") { UIPasteboard.general.string = thread?.title }
                    if working {
                        Button("Stop", systemImage: "stop.fill", role: .destructive) { model.interrupt(threadID) }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                }
            }
        }
        .onAppear {
            mode = model.followUpMode
            model.subscribe(threadID)
        }
        .onDisappear { model.unsubscribe(threadID) }
    }

    private var subtitle: String {
        guard let t = thread else { return "" }
        return [t.project?.name, t.branch].compactMap { $0 }.joined(separator: " · ")
    }

    @ViewBuilder
    private func blockView(_ block: Block, isLatestGroup: Bool) -> some View {
        switch block {
        case .user(_, let text, let images):
            UserBubble(text: text, images: images)
        case .assistant(_, let text):
            MarkdownText(text: text)
                .textSelection(.enabled)
        case .group(let id, let items):
            ToolGroupView(items: items, expanded: Binding(
                get: { groupOpen[id] ?? (isLatestGroup || Launch.expandAll) },
                set: { groupOpen[id] = $0 }))
        case .resolved(let item):
            ResolvedRequestRow(item: item)
        case .turnEnd(_, let secs):
            HStack(spacing: 10) {
                Rectangle().fill(Trek.border).frame(height: 0.5)
                Text("Worked for \(When.duration(secs))").font(.caption).foregroundStyle(Trek.muted.opacity(0.8)).fixedSize()
                Rectangle().fill(Trek.border).frame(height: 0.5)
            }
        case .notice(_, let text):
            Label(text, systemImage: "info.circle").font(.subheadline).foregroundStyle(Trek.muted)
        case .error(_, let text):
            Label {
                Text(text)
            } icon: {
                Image(systemName: "exclamationmark.triangle.fill")
            }
            .font(.subheadline)
            .foregroundStyle(Trek.failed)
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Trek.failed.opacity(0.08), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        case .limit(_, let text, let resets):
            Label {
                VStack(alignment: .leading, spacing: 2) {
                    Text(text)
                    if let resets {
                        Text("Resets \(Date(timeIntervalSince1970: Double(resets) / 1000), style: .relative)").font(.caption)
                    }
                }
            } icon: {
                Image(systemName: "clock.fill")
            }
            .font(.subheadline)
            .foregroundStyle(Trek.approval)
        case .handoff(_, let from, let to):
            Label("Handed over from \(from) to \(to)", systemImage: "arrow.left.arrow.right")
                .font(.subheadline).foregroundStyle(Trek.muted)
        }
    }

    private func requestId(_ item: TItem) -> String? {
        switch item.body {
        case .approval(let a): a.requestId
        case .question(let q): q.requestId
        case .plan(let p): p.requestId
        default: nil
        }
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        model.send(text, to: threadID, mode: thread?.runState == .working ? mode : nil)
        draft = ""
    }
}
