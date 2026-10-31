import SwiftUI

/// The request waiting on the user, pinned above the composer. Nothing here ever answers by itself:
/// the agent waits, on the Mac and here, until someone decides.
struct PendingRequestCard: View {
    var item: TItem
    var agentName: String
    /// Its thread's qualified id: which machine's settings "Allow for this session" answers to.
    var threadID: String
    var answer: (AnswerResponse) -> Void

    var body: some View {
        switch item.body {
        case .approval(let a): ApprovalCard(request: a, agentName: agentName, threadID: threadID, answer: answer)
        case .question(let q): QuestionCard(request: q, agentName: agentName, answer: answer)
        case .plan(let p): PlanCard(request: p, agentName: agentName, answer: answer)
        default: EmptyView()
        }
    }
}

/// A request card as the Mac draws it: the icon in the card's colour (muted when it has none), the
/// title in semibold, on the panel colour with a rim of the tint; an approval also gets a wash of it.
private struct CardChrome<Content: View>: View {
    var tint: Color?
    var wash = false
    var symbol: String
    var title: String
    @ViewBuilder var content: Content

    private var shape: RoundedRectangle { RoundedRectangle(cornerRadius: 16, style: .continuous) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: symbol)
                    .font(.ui(size: 14, weight: .medium, relativeTo: .subheadline))
                    .foregroundStyle(tint ?? Trek.muted)
                Text(title).font(.ui(.subheadline).weight(.semibold))
                Spacer()
            }
            content
        }
        .padding(14)
        .background {
            shape.fill(Trek.surface)
            if wash, let tint { shape.fill(tint.opacity(0.07)) }
        }
        .overlay(shape.strokeBorder(tint.map { $0.opacity(wash ? 0.6 : 0.45) } ?? Trek.foreground.opacity(0.12), lineWidth: 1))
        .shadow(color: .black.opacity(0.1), radius: 16, y: 6)
    }
}

/// The Mac's primary button: ember, its label in the theme's ink for it.
private struct PrimaryLabel: View {
    var title: String
    /// What it says where the whole title doesn't fit (never cut off with an ellipsis).
    var short: String? = nil

    var body: some View {
        ViewThatFits(in: .horizontal) {
            Text(title).fontWeight(.semibold).lineLimit(1).fixedSize()
            Text(short ?? title).fontWeight(.semibold).lineLimit(1).minimumScaleFactor(0.8)
        }
        .foregroundStyle(Trek.onEmber)
        .frame(maxWidth: .infinity)
    }
}

struct ApprovalCard: View {
    var request: ApprovalRequest
    var agentName: String
    var threadID: String
    var answer: (AnswerResponse) -> Void
    @Environment(AppModel.self) private var model
    @State private var expanded = false
    @State private var confirming = false
    @State private var detailHeight: CGFloat = 124

    /// "Run command" → "run a command": the request's title as the end of a sentence.
    static func phrase(_ title: String) -> String {
        let t = title.lowercased()
        if t.hasPrefix("run") || t.hasPrefix("bash") || t.hasPrefix("exec") { return "run a command" }
        if t.hasPrefix("edit") || t.hasPrefix("write") || t.hasPrefix("apply") { return "change files" }
        if t.hasPrefix("fetch") || t.hasPrefix("web") { return "go online" }
        if t.isEmpty { return "do something" }
        return "use \(title)"
    }

    private var warning: String? { CommandRisk.warning(for: request.detail) }

    /// Roughly how many lines the detail wraps to in the card (~40 monospaced characters a line).
    private var estimatedLines: Int {
        request.detail.split(separator: "\n", omittingEmptySubsequences: false)
            .reduce(0) { $0 + max(1, Int((Double($1.count) / 40).rounded(.up))) }
    }

    private var isLong: Bool { estimatedLines > 7 }

    var body: some View {
        CardChrome(tint: Trek.approval, wash: true, symbol: "checkmark.shield", title: "\(agentName) wants to \(Self.phrase(request.title))") {
            detail

            if let warning {
                Label {
                    Text("Looks destructive: \(warning). Read it all before allowing.")
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill")
                }
                .font(.ui(.footnote).weight(.medium))
                .foregroundStyle(Trek.failed)
                .fixedSize(horizontal: false, vertical: true)
            }

            HStack(spacing: 10) {
                Button {
                    answer(.approval(.deny))
                } label: {
                    Text("Deny").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glass)
                .controlSize(.large)

                // A destructive-looking command gets no visual push towards Allow.
                if warning == nil {
                    Button {
                        answer(.approval(.allow))
                    } label: {
                        PrimaryLabel(title: "Allow")
                    }
                    .buttonStyle(.glassProminent)
                    .tint(Trek.ember)
                    .controlSize(.large)
                } else {
                    Button {
                        answer(.approval(.allow))
                    } label: {
                        Text("Allow").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.glass)
                    .controlSize(.large)
                }

                // Only where the Mac takes it (its Settings › Phone): it refuses otherwise.
                if model.link(for: threadID)?.macSettings?.sessionApprovals != false {
                    Menu {
                        Button("Allow for this session…", systemImage: "checkmark.seal") { allowForSession() }
                    } label: {
                        Image(systemName: "ellipsis").frame(width: 22, height: 22)
                    }
                    .buttonStyle(.glass)
                    .controlSize(.large)
                    .fixedSize()
                    .disabled(confirming)
                    .accessibilityLabel("More options")
                }
            }
        }
    }

    /// The command or file in full: wrapped, monospaced, selectable. A long one starts folded to
    /// about seven lines with "Show all", then scrolls inside the card.
    @ViewBuilder
    private var detail: some View {
        let text = Text(request.detail)
            .font(.code(.footnote))
            .foregroundStyle(Trek.foreground)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
            .padding(10)
        VStack(alignment: .leading, spacing: 6) {
            Group {
                if !isLong {
                    text
                } else {
                    ScrollView {
                        text.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { detailHeight = $0 }
                    }
                    .scrollDisabled(!expanded)
                    .scrollIndicators(expanded ? .automatic : .hidden)
                    .frame(height: expanded ? min(detailHeight, 300) : 124)
                    .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black, location: expanded ? 1 : 0.7),
                                                 .init(color: .black.opacity(expanded ? 1 : 0.15), location: 1)],
                                         startPoint: .top, endPoint: .bottom))
                }
            }
            .background(Trek.foreground.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            if isLong {
                Button(expanded ? "Show less" : "Show all \(estimatedLines) lines") {
                    withAnimation(.snappy) { expanded.toggle() }
                }
                .font(.ui(.footnote).weight(.medium))
                .foregroundStyle(Trek.approval)
            }
        }
    }

    /// "Allow for session" stops the agent asking for this kind of action, so it needs the
    /// phone's owner: Face ID, Touch ID or the passcode before anything is sent.
    private func allowForSession() {
        confirming = true
        Task {
            let result = await DeviceOwner.confirm("Let \(agentName) \(Self.phrase(request.title)) without asking again this session.")
            confirming = false
            switch result {
            case .success: answer(.approval(.allowForSession))
            case .failure(let e): if let message = e.message { model.show(message, error: true) }
            }
        }
    }
}

/// Scrolls the requests over the composer to a field being typed in, once the keyboard is up.
final class RequestScroller {
    var proxy: ScrollViewProxy?

    func reveal(_ id: String) {
        // Now, and again once the keyboard has risen (the requests' room shrinks as it does).
        for delay in [0.05, 0.4] {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
                withAnimation(.snappy) { self?.proxy?.scrollTo(id, anchor: .bottom) }
            }
        }
    }
}

extension EnvironmentValues {
    @Entry var requestScroller: RequestScroller? = nil
}

struct QuestionCard: View {
    var request: QuestionRequest
    var agentName: String
    var answer: (AnswerResponse) -> Void
    @State private var picks: [Int: Set<String>] = [:]
    @State private var other: [Int: String] = [:]
    /// The question whose answer is being typed.
    @FocusState private var typing: Int?
    @Environment(\.requestScroller) private var scroller

    private func fieldID(_ qi: Int) -> String { "\(request.requestId)-answer-\(qi)" }

    var body: some View {
        CardChrome(tint: nil, symbol: "bubble.left", title: "\(agentName) has a question") {
            ForEach(Array(request.questions.enumerated()), id: \.offset) { qi, q in
                VStack(alignment: .leading, spacing: 8) {
                    Text(q.question).font(.ui(.body).weight(.medium))
                    ForEach(q.options, id: \.label) { option in
                        let on = picks[qi]?.contains(option.label) == true
                        Button {
                            withAnimation(.snappy(duration: 0.15)) { toggle(qi, option.label, multi: q.multi) }
                        } label: {
                            HStack(alignment: .top, spacing: 10) {
                                Image(systemName: on ? (q.multi ? "checkmark.square.fill" : "largecircle.fill.circle") : (q.multi ? "square" : "circle"))
                                    .foregroundStyle(on ? Trek.ember : Trek.muted)
                                    .font(.system(size: 17))
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(option.label).font(.ui(.subheadline).weight(.semibold)).foregroundStyle(Trek.foreground)
                                    if !option.description.isEmpty {
                                        Text(option.description).font(.ui(.caption)).foregroundStyle(Trek.muted)
                                    }
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(10)
                            .background(on ? Trek.ember.opacity(0.08) : Color.clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(on ? Trek.ember.opacity(0.7) : Trek.foreground.opacity(0.1), lineWidth: 1))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    AnswerField(secret: q.secret, text: Binding(get: { other[qi] ?? "" }, set: { other[qi] = $0 }))
                        .font(.ui(.subheadline))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(Trek.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .focused($typing, equals: qi)
                        .id(fieldID(qi))
                }
            }
            Button {
                answer(.questions(answers))
            } label: {
                PrimaryLabel(title: "Answer")
            }
            .buttonStyle(.glassProminent)
            .tint(Trek.ember)
            .controlSize(.large)
            .disabled(answers.count < request.questions.count)
        }
        .onChange(of: typing) {
            if let qi = typing { scroller?.reveal(fieldID(qi)) }
        }
    }

    private func toggle(_ qi: Int, _ label: String, multi: Bool) {
        var set = picks[qi] ?? []
        if set.contains(label) { set.remove(label) } else {
            if !multi { set = [] }
            set.insert(label)
        }
        picks[qi] = set
        if !multi { other[qi] = "" }
    }

    private var answers: [QuestionAnswer] {
        request.questions.enumerated().compactMap { qi, q in
            let typed = (other[qi] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let chosen = q.options.map(\.label).filter { picks[qi]?.contains($0) == true }
            let value = typed.isEmpty ? chosen.joined(separator: ", ") : typed
            return value.isEmpty ? nil : QuestionAnswer(question: q.question, answer: value)
        }
    }
}

struct PlanCard: View {
    var request: PlanRequest
    var agentName: String
    var answer: (AnswerResponse) -> Void
    @State private var showAll = false
    @State private var feedback = ""
    @State private var revising = false
    @FocusState private var writing: Bool
    @Environment(\.requestScroller) private var scroller

    var body: some View {
        CardChrome(tint: Trek.plan, symbol: "checklist", title: "\(agentName)'s plan") {
            ScrollView {
                MarkdownText(text: request.markdown, size: 15)
            }
            .frame(maxHeight: showAll ? 360 : 150)
            .scrollDisabled(!showAll)
            .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black, location: showAll ? 1 : 0.72),
                                         .init(color: showAll ? .black : .clear, location: 1)], startPoint: .top, endPoint: .bottom))
            Button(showAll ? "Show less" : "Show the whole plan") { withAnimation(.snappy) { showAll.toggle() } }
                .font(.ui(.footnote).weight(.medium))
                .foregroundStyle(Trek.plan)

            if revising {
                TextField("What should change?", text: $feedback, axis: .vertical)
                    .lineLimit(1...4)
                    .font(.ui(.subheadline))
                    .padding(10)
                    .background(Trek.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .focused($writing)
                    .id("\(request.requestId)-feedback")
            } else {
                Text("Nothing has been changed yet.").font(.ui(.footnote)).foregroundStyle(Trek.muted)
            }
            HStack(spacing: 10) {
                Button {
                    if revising { answer(.plan(approve: false, feedback: feedback)) } else { withAnimation(.snappy) { revising = true } }
                } label: {
                    Text(revising ? "Send back" : "Keep planning").lineLimit(1)
                }
                .buttonStyle(.glass)
                .controlSize(.large)
                .fixedSize()
                Button {
                    answer(.plan(approve: true, feedback: nil))
                } label: {
                    PrimaryLabel(title: "Approve and start", short: "Approve")
                }
                .buttonStyle(.glassProminent)
                .tint(Trek.ember)
                .controlSize(.large)
            }
        }
        .onChange(of: writing) {
            if writing { scroller?.reveal("\(request.requestId)-feedback") }
        }
    }
}
