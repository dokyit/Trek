import SwiftUI

/// A sub-agent's row in its parent's transcript, as the Mac draws it (`thread_view.rs`,
/// `Row::SubAgent`): whose it is (its agent's logo, with a dot for how it's doing), what it was
/// sent to do, where it stands, how long it has worked. A tap opens it: Trek's own in its thread,
/// the agent's own in `SubAgentView`. A Mac that serves neither shows the row, unopened.
struct SubAgentCallRow: View {
    var call: ToolCall
    var threadID: String
    @Environment(AppModel.self) private var model
    @Environment(\.openThread) private var openThread

    private var link: SubAgentLink? { call.subAgent }

    /// A tap opens it here.
    private var opens: Bool {
        guard let link else { return false }
        if link.threadId != nil { return model.serves(.subAgents, on: threadID) }
        return model.serves(.subAgentView, on: threadID)
    }

    var body: some View {
        let link = link ?? SubAgentLink(agent: AgentRef(key: "", name: ""), label: call.detail, state: .running)
        Button {
            model.open(subAgent: link, threadId: nil, callId: call.callId, in: threadID, open: openThread.map { o in { o($0) } })
        } label: {
            TimelineView(.periodic(from: .now, by: 1)) { ctx in
                HStack(spacing: 11) {
                    ZStack(alignment: .bottomTrailing) {
                        AgentGlyph(link.agent, size: 26)
                        BeaconDot(color: SubAgentsCard.color(link.state), pulses: link.state == .running, size: 9)
                            .padding(2)
                            .background(Trek.background, in: Circle())
                            .offset(x: 4, y: 4)
                    }
                    VStack(alignment: .leading, spacing: 2) {
                        Text(link.label.isEmpty ? "Sub-agent" : link.label)
                            .font(.ui(.subheadline).weight(.medium))
                            .foregroundStyle(Trek.foreground)
                            .lineLimit(1)
                        Text(SubAgentCallRow.detail(link))
                            .font(.ui(.footnote))
                            .foregroundStyle(link.state == .failed ? Trek.failed : Trek.muted)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    if let time = SubAgentCallRow.time(link, now: ctx.date) {
                        Text(time).font(.ui(.footnote).monospacedDigit()).foregroundStyle(Trek.muted).fixedSize()
                    }
                    if opens {
                        Image(systemName: "chevron.right").font(.ui(.caption).weight(.semibold)).foregroundStyle(Trek.muted)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .background(Trek.foreground.opacity(0.04), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.75))
                .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            }
        }
        .buttonStyle(.plain)
        .allowsHitTesting(opens)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Sub-agent \(link.label). \(SubAgentCallRow.detail(link))")
        .accessibilityHint(opens ? (link.threadId != nil ? "Opens its thread" : "Shows what it's doing") : "")
        .accessibilityAddTraits(opens ? .isButton : [])
    }

    /// Its line under the label: the Mac's words, else its state.
    static func detail(_ link: SubAgentLink) -> String {
        let d = link.detail?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return d.isEmpty ? SubAgentsCard.label(link.state) : d
    }

    /// How long it has worked: ticking while it works, as it ended once done.
    static func time(_ link: SubAgentLink, now: Date) -> String? {
        if link.live, let since = link.since {
            return When.duration(max(0, Int(now.timeIntervalSince1970 - Double(since) / 1000)))
        }
        return link.tookSecs.map { When.duration($0) }
    }
}

/// One of the agent's own sub-agents (Claude Code's Task, Codex's agents), opened from its row:
/// what it was told, what it said and called, live while it runs (asked for again every second
/// and a half while it's on screen), its answer once it's done. Stop where its agent can stop it,
/// a message where it takes one, and where it doesn't, the Mac's words for why. What it asks for
/// waits on its parent's thread, under this sheet, and is answered there.
struct SubAgentView: View {
    let target: SubAgentTarget
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var watch: SubAgentWatch
    @State private var confirmingStop = false
    @State private var draft = ""
    @FocusState private var writing: Bool

    init(target: SubAgentTarget) {
        self.target = target
        _watch = State(initialValue: SubAgentWatch(target: target))
    }

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        header
                        if let log = watch.log {
                            content(log)
                        } else if let failure = watch.failure {
                            Label(failure, systemImage: "exclamationmark.triangle")
                                .font(.ui(.subheadline)).foregroundStyle(Trek.muted)
                        } else {
                            ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
                        }
                        Color.clear.frame(height: 1).id("end")
                    }
                    .padding(16)
                }
                .onChange(of: watch.log?.items.count) { withAnimation(.snappy) { proxy.scrollTo("end", anchor: .bottom) } }
            }
            .background(Trek.background)
            .safeAreaInset(edge: .bottom, spacing: 0) { steering }
            .navigationTitle("Sub-agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Done") { dismiss() }
                }
                if watch.log?.canStop == true {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Stop", systemImage: "stop.fill", role: .destructive) { confirmingStop = true }
                            .labelStyle(.titleAndIcon)
                            .disabled(watch.sending)
                            .accessibilityIdentifier("sub-agent-stop")
                    }
                }
            }
            .confirmationDialog("Stop this sub-agent?", isPresented: $confirmingStop, titleVisibility: .visible) {
                Button("Stop", role: .destructive) {
                    watch.steer(.stop, model: model) { why in if let why { model.show(why, error: true) } }
                }
            } message: {
                Text("\(watch.link?.agent.name ?? "The agent") stops it on the Mac. The thread it works for carries on.")
            }
        }
        .onAppear { watch.start(model) }
        .onDisappear { watch.stop() }
    }

    // MARK: Parts

    private var header: some View {
        let link = watch.link
        return TimelineView(.periodic(from: .now, by: 1)) { ctx in
            HStack(alignment: .top, spacing: 12) {
                ZStack(alignment: .bottomTrailing) {
                    AgentGlyph(link?.agent ?? AgentRef(key: "", name: ""), size: 32)
                    if let link {
                        BeaconDot(color: SubAgentsCard.color(link.state), pulses: link.state == .running, size: 10)
                            .padding(2)
                            .background(Trek.background, in: Circle())
                            .offset(x: 4, y: 4)
                    }
                }
                VStack(alignment: .leading, spacing: 3) {
                    Text(link?.label ?? "Sub-agent")
                        .font(.ui(.headline))
                        .foregroundStyle(Trek.foreground)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 5) {
                        if let link {
                            Text(SubAgentsCard.label(link.state)).foregroundStyle(SubAgentsCard.color(link.state))
                            Text("·")
                            Text(link.agent.name)
                            if let time = SubAgentCallRow.time(link, now: ctx.date) {
                                Text("·")
                                Text(time).monospacedDigit()
                            }
                        }
                    }
                    .font(.ui(.footnote))
                    .foregroundStyle(Trek.muted)
                }
                Spacer(minLength: 0)
            }
            .accessibilityElement(children: .combine)
        }
    }

    @ViewBuilder
    private func content(_ log: SubAgentLog) -> some View {
        if let prompt = log.prompt, !prompt.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("Told to").font(.ui(.caption).weight(.semibold)).foregroundStyle(Trek.muted).textCase(.uppercase)
                Text(prompt)
                    .font(.ui(.subheadline))
                    .foregroundStyle(Trek.foreground)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .background(Trek.surface, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
            }
        }
        if log.asking {
            asking
        }
        if log.more > 0 {
            Text("\(log.more) earlier step\(log.more == 1 ? "" : "s") aren't kept on the Mac.")
                .font(.ui(.footnote)).foregroundStyle(Trek.muted)
        }
        if log.gone {
            Label("The Mac no longer has its steps: its session ended, or Trek restarted since.", systemImage: "clock.arrow.circlepath")
                .font(.ui(.footnote)).foregroundStyle(Trek.muted)
        } else if log.items.isEmpty, log.link.live {
            Text("Nothing yet: its steps show here as it takes them.").font(.ui(.footnote)).foregroundStyle(Trek.muted)
        }
        VStack(alignment: .leading, spacing: 10) {
            ForEach(log.items) { item in
                switch item.body {
                case .assistant(let text, _):
                    MarkdownText(text: text, size: 15).equatable().textSelection(.enabled)
                case .user(let text, _, _):
                    // A message it was sent (from here or the Mac).
                    UserBubble(text: text, images: 0)
                case .tool:
                    ToolRow(item: item)
                default:
                    EmptyView()
                }
            }
        }
        if let answer = log.answer, !answer.isEmpty, !log.link.live {
            VStack(alignment: .leading, spacing: 6) {
                Text("Its answer").font(.ui(.caption).weight(.semibold)).foregroundStyle(Trek.muted).textCase(.uppercase)
                MarkdownText(text: answer, size: 15).equatable().textSelection(.enabled)
            }
            .padding(.top, 4)
        }
    }

    /// Its parent's thread waits on the user: the request is there, under this sheet.
    private var asking: some View {
        HStack(spacing: 10) {
            Image(systemName: "hand.raised.fill").foregroundStyle(Trek.approval)
            Text("A request waits on the thread it works for. Answer it there.")
                .font(.ui(.footnote)).foregroundStyle(Trek.foreground)
            Spacer(minLength: 6)
            Button("Go to thread") { dismiss() }
                .font(.ui(.footnote).weight(.semibold))
                .buttonStyle(.bordered)
                .accessibilityIdentifier("sub-agent-to-thread")
        }
        .padding(12)
        .background(Trek.approval.opacity(0.1), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    /// At the foot: a message for it, where it takes one; else why not, plainly.
    @ViewBuilder
    private var steering: some View {
        if let log = watch.log, log.link.live, !log.gone {
            Group {
                if log.canMessage {
                    HStack(spacing: 8) {
                        TextField("Message the sub-agent", text: $draft, axis: .vertical)
                            .lineLimit(1...4)
                            .focused($writing)
                            .font(.ui(.body))
                            .padding(.horizontal, 14)
                            .padding(.vertical, 9)
                            .background(Trek.surface, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
                            .accessibilityIdentifier("sub-agent-message")
                        Button {
                            send()
                        } label: {
                            Image(systemName: "arrow.up")
                                .font(.system(size: 15, weight: .bold))
                                .foregroundStyle(Trek.background)
                                .frame(width: 34, height: 34)
                                .background(Trek.foreground, in: Circle())
                        }
                        .buttonStyle(.plain)
                        .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || watch.sending)
                        .opacity(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? 0.4 : 1)
                        .accessibilityLabel("Send to the sub-agent")
                    }
                } else if let why = log.noMessage {
                    Label(why, systemImage: "bubble.left.and.exclamationmark.bubble.right")
                        .font(.ui(.footnote))
                        .foregroundStyle(Trek.muted)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityIdentifier("sub-agent-no-message")
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(.bar)
        }
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        draft = ""
        watch.steer(.message, text: text, model: model) { why in
            if let why {
                draft = text
                model.show(why, error: true)
            }
        }
    }
}
