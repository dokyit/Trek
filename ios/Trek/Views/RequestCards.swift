import SwiftUI

/// The request waiting on the user, pinned above the composer. Nothing here ever answers by itself:
/// the agent waits, on the Mac and here, until someone decides.
struct PendingRequestCard: View {
    var item: TItem
    var agentName: String
    var answer: (AnswerResponse) -> Void

    var body: some View {
        switch item.body {
        case .approval(let a): ApprovalCard(request: a, agentName: agentName, answer: answer)
        case .question(let q): QuestionCard(request: q, answer: answer)
        case .plan(let p): PlanCard(request: p, answer: answer)
        default: EmptyView()
        }
    }
}

private struct CardChrome<Content: View>: View {
    var tint: Color
    var symbol: String
    var title: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Image(systemName: symbol)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(tint)
                    .frame(width: 26, height: 26)
                    .background(tint.opacity(0.15), in: Circle())
                Text(title).font(.subheadline.weight(.semibold))
                Spacer()
            }
            content
        }
        .padding(14)
        .background(Trek.elevated.opacity(0.92), in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 22, style: .continuous).strokeBorder(tint.opacity(0.45), lineWidth: 1))
        .shadow(color: .black.opacity(0.12), radius: 18, y: 6)
    }
}

struct ApprovalCard: View {
    var request: ApprovalRequest
    var agentName: String
    var answer: (AnswerResponse) -> Void
    @State private var confirmSession = false

    /// "Run command" → "run a command": the request's title as the end of a sentence.
    static func phrase(_ title: String) -> String {
        let t = title.lowercased()
        if t.hasPrefix("run") || t.hasPrefix("bash") || t.hasPrefix("exec") { return "run a command" }
        if t.hasPrefix("edit") || t.hasPrefix("write") || t.hasPrefix("apply") { return "change files" }
        if t.hasPrefix("fetch") || t.hasPrefix("web") { return "go online" }
        if t.isEmpty { return "do something" }
        return "use \(title)"
    }

    var body: some View {
        CardChrome(tint: Trek.approval, symbol: "hand.raised.fill", title: "\(agentName) wants to \(Self.phrase(request.title))") {
            ScrollView(.horizontal, showsIndicators: false) {
                Text(request.detail)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(Trek.foreground)
                    .padding(10)
            }
            .background(Trek.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 10, style: .continuous))

            HStack(spacing: 10) {
                Button {
                    answer(.approval(.deny))
                } label: {
                    Text("Deny").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glass)
                .controlSize(.large)

                Button {
                    answer(.approval(.allow))
                } label: {
                    Text("Allow").fontWeight(.semibold).frame(maxWidth: .infinity)
                }
                .buttonStyle(.glassProminent)
                .tint(Trek.approval)
                .controlSize(.large)

                Menu {
                    Button("Allow for this session", systemImage: "checkmark.seal") { confirmSession = true }
                } label: {
                    Image(systemName: "ellipsis").frame(width: 22, height: 22)
                }
                .buttonStyle(.glass)
                .controlSize(.large)
            }
        }
        .confirmationDialog("Allow similar requests for the rest of this session?", isPresented: $confirmSession, titleVisibility: .visible) {
            Button("Allow for this session") { answer(.approval(.allowForSession)) }
        } message: {
            Text("The agent won't ask again for this kind of action until the session ends.")
        }
    }
}

struct QuestionCard: View {
    var request: QuestionRequest
    var answer: (AnswerResponse) -> Void
    @State private var picks: [Int: Set<String>] = [:]
    @State private var other: [Int: String] = [:]

    var body: some View {
        CardChrome(tint: Trek.question, symbol: "questionmark.bubble.fill", title: request.questions.first?.header ?? "Question") {
            ForEach(Array(request.questions.enumerated()), id: \.offset) { qi, q in
                VStack(alignment: .leading, spacing: 8) {
                    Text(q.question).font(.body.weight(.medium))
                    ForEach(q.options, id: \.label) { option in
                        let on = picks[qi]?.contains(option.label) == true
                        Button {
                            withAnimation(.snappy(duration: 0.15)) { toggle(qi, option.label, multi: q.multi) }
                        } label: {
                            HStack(alignment: .top, spacing: 10) {
                                Image(systemName: on ? (q.multi ? "checkmark.square.fill" : "largecircle.fill.circle") : (q.multi ? "square" : "circle"))
                                    .foregroundStyle(on ? Trek.question : Trek.muted)
                                    .font(.system(size: 17))
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(option.label).font(.subheadline.weight(.semibold)).foregroundStyle(Trek.foreground)
                                    if !option.description.isEmpty {
                                        Text(option.description).font(.caption).foregroundStyle(Trek.muted)
                                    }
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(10)
                            .background(on ? Trek.question.opacity(0.1) : Color.clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(on ? Trek.question.opacity(0.5) : Trek.border, lineWidth: 0.8))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    TextField(q.secret ? "Type a secret answer" : "Or type an answer", text: Binding(get: { other[qi] ?? "" }, set: { other[qi] = $0 }))
                        .font(.subheadline)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(Trek.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                }
            }
            Button {
                answer(.questions(answers))
            } label: {
                Text("Send answer").fontWeight(.semibold).frame(maxWidth: .infinity)
            }
            .buttonStyle(.glassProminent)
            .tint(Trek.question)
            .controlSize(.large)
            .disabled(answers.count < request.questions.count)
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
    var answer: (AnswerResponse) -> Void
    @State private var showAll = false
    @State private var feedback = ""
    @State private var revising = false

    var body: some View {
        CardChrome(tint: Trek.plan, symbol: "list.bullet.clipboard.fill", title: "Plan ready for review") {
            ScrollView {
                MarkdownText(text: request.markdown, size: 15)
            }
            .frame(maxHeight: showAll ? 360 : 150)
            .scrollDisabled(!showAll)
            .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black, location: showAll ? 1 : 0.72),
                                         .init(color: showAll ? .black : .clear, location: 1)], startPoint: .top, endPoint: .bottom))
            Button(showAll ? "Show less" : "Show the whole plan") { withAnimation(.snappy) { showAll.toggle() } }
                .font(.footnote.weight(.medium))
                .foregroundStyle(Trek.plan)

            if revising {
                TextField("What should change?", text: $feedback, axis: .vertical)
                    .lineLimit(1...4)
                    .font(.subheadline)
                    .padding(10)
                    .background(Trek.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
            HStack(spacing: 10) {
                Button {
                    if revising { answer(.plan(approve: false, feedback: feedback)) } else { withAnimation(.snappy) { revising = true } }
                } label: {
                    Text(revising ? "Send back" : "Keep planning").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glass)
                .controlSize(.large)
                Button {
                    answer(.plan(approve: true, feedback: nil))
                } label: {
                    Text("Approve plan").fontWeight(.semibold).frame(maxWidth: .infinity)
                }
                .buttonStyle(.glassProminent)
                .tint(Trek.plan)
                .controlSize(.large)
            }
        }
    }
}
