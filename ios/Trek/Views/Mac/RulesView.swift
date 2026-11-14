import SwiftUI

/// The Mac's Settings › Permissions rules, as its page has them: the list, a tester that says which
/// rule decides a call at each level, and adding and removing a rule. Writing them is the Mac's to
/// allow (its Settings › Phone): until it does, the list and the tester are here and the rest says
/// where to turn it on.
struct RulesView: View {
    var link: HostLink?
    @Environment(AppModel.self) private var model
    @State private var list: RulesList?
    @State private var kind: RuleKind = .command
    @State private var action: RuleVerdict = .deny
    @State private var pattern = ""
    @State private var testKind: RuleKind = .command
    @State private var tried = ""
    @State private var test: RuleTest?
    @State private var busy = false

    private var editable: Bool { list?.editable ?? false }

    var body: some View {
        Form {
            Section {
                if let list {
                    if list.rules.isEmpty {
                        Text("No rules yet. The thread's level decides what asks.")
                            .foregroundStyle(Trek.muted)
                    }
                    ForEach(Array(list.rules.enumerated()), id: \.offset) { i, rule in
                        RuleRow(rule: rule)
                            .swipeActions {
                                if editable {
                                    Button("Remove", role: .destructive) { change(.remove(index: i, rule)) }
                                }
                            }
                    }
                } else {
                    HStack(spacing: 10) {
                        ProgressView().controlSize(.small)
                        Text("Asking your Mac…").foregroundStyle(Trek.muted)
                    }
                }
            } header: {
                Text("Rules")
            } footer: {
                Text("A deny holds at every level. An ask puts a card up even at Full access. An allow lets it through without asking. Deny is tried first, then ask, then allow.")
                    .font(.ui(.footnote))
            }

            if editable {
                Section {
                    Picker("Looks at", selection: $kind) {
                        ForEach(RuleKind.allCases) { Text($0.label).tag($0) }
                    }
                    Picker("Then", selection: $action) {
                        ForEach(RuleVerdict.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    TextField(kind.placeholder, text: $pattern)
                        .font(.code(.subheadline))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .onSubmit(add)
                    Button("Add rule", action: add)
                        .disabled(pattern.trimmingCharacters(in: .whitespaces).isEmpty || busy)
                } header: {
                    Text("New rule")
                } footer: {
                    Text(action == .allow ? "An allow lasts until it's removed. Face ID first." : "It applies from the next request, in sessions already running.")
                        .font(.ui(.footnote))
                }
            } else if list != nil {
                Section {
                    Label("Adding and removing rules from this iPhone is off on your Mac. Turn it on there in Settings › Phone, or change them on the Mac.",
                          systemImage: "lock")
                        .font(.ui(.footnote))
                        .foregroundStyle(Trek.muted)
                }
            }

            Section {
                Picker("Try", selection: $testKind) {
                    ForEach(RuleKind.allCases) { Text($0.label).tag($0) }
                }
                TextField(testKind.testPlaceholder, text: $tried)
                    .font(.code(.subheadline))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                if let test, test.text == tried.trimmingCharacters(in: .whitespaces) || test.text == tried {
                    Text(test.said)
                        .font(.ui(.footnote))
                        .foregroundStyle(Trek.foreground)
                        .fixedSize(horizontal: false, vertical: true)
                    ForEach(test.levels, id: \.level) { l in
                        LabeledContent(l.level.label) {
                            Text(l.outcome.label).fontWeight(.medium).foregroundStyle(Self.tint(l.outcome))
                        }
                    }
                }
            } header: {
                Text("Try a call")
            } footer: {
                Text("What happens to it at each level, with your rules, as the Mac's tester says.")
                    .font(.ui(.footnote))
            }
        }
        .scrollContentBackground(.hidden)
        .background(Trek.grouped)
        .navigationTitle("Permission rules")
        .navigationSubtitle(link?.name ?? "")
        .task { model.loadRules(on: link) { list = $0 } }
        .task(id: "\(testKind.rawValue) \(tried)") {
            let text = tried.trimmingCharacters(in: .whitespaces)
            guard !text.isEmpty else { test = nil; return }
            // As it's typed: once it settles a moment.
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            model.testRule(testKind, text, on: link) { test = $0 }
        }
    }

    static func tint(_ outcome: RuleOutcome) -> Color {
        switch outcome {
        case .refused: Trek.failed
        case .asks: Trek.approval
        case .runs: Trek.done
        case .unknown: Trek.muted
        }
    }

    private func add() {
        let p = pattern.trimmingCharacters(in: .whitespaces)
        guard !p.isEmpty, !busy else { return }
        let rule = Rule(kind: kind, pattern: p, action: action)
        guard action == .allow else { return change(.add(rule)) }
        Task {
            switch await DeviceOwner.confirm("Allow \(rule.kind.label.lowercased()) “\(p)” without asking, until the rule is removed") {
            case .success: change(.add(rule))
            case .failure(let e): if let m = e.message { model.show(m, error: true) }
            }
        }
    }

    private func change(_ c: RuleChange) {
        busy = true
        model.changeRule(c, on: link) { new in
            busy = false
            guard let new else { return }
            withAnimation(.snappy) { list = new }
            if case .add = c { pattern = "" }
        }
    }
}

/// A rule as the Mac's list shows it: its action in its colour, what it looks at, the pattern.
struct RuleRow: View {
    var rule: Rule

    private var tint: Color {
        switch rule.action {
        case .deny: Trek.failed
        case .ask: Trek.approval
        case .allow: Trek.done
        }
    }

    var body: some View {
        HStack(spacing: 10) {
            Text(rule.action.label)
                .font(.ui(.caption).weight(.semibold))
                .foregroundStyle(tint)
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: Trek.corner(6), style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text(rule.pattern).font(.code(.subheadline)).foregroundStyle(Trek.foreground).lineLimit(2)
                Text(rule.kind.label).font(.ui(.caption)).foregroundStyle(Trek.muted)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(rule.shown)
    }
}

/// An approval card's "Always allow…": the rule it would write, ready to shorten, then "Add rule
/// and allow" (after Face ID: it lasts until it's removed). The Mac checks the rule would let this
/// card's call through, and says why not when it wouldn't.
struct AlwaysAllowSheet: View {
    var request: ApprovalRequest
    var draft: RuleDraft
    var agentName: String
    var threadID: String
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var pattern = ""
    @State private var busy = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text(request.detail)
                        .font(.code(.footnote))
                        .foregroundStyle(Trek.muted)
                        .lineLimit(4)
                } header: {
                    Text("\(agentName) wants to \(ApprovalCard.phrase(request.title))")
                }
                Section {
                    TextField(draft.kind.placeholder, text: $pattern, axis: .vertical)
                        .font(.code(.subheadline))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } header: {
                    Text("Allow \(draft.kind.label.lowercased())")
                } footer: {
                    Text("It runs without asking from now on. The Mac's Settings › Permissions lists it, and it can be removed there or from this iPhone's Trek settings.")
                        .font(.ui(.footnote))
                }
            }
            .scrollContentBackground(.hidden)
            .background(Trek.grouped)
            .navigationTitle("Always allow")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add rule and allow", action: add)
                        .disabled(pattern.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || busy)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .onAppear { if pattern.isEmpty { pattern = draft.pattern } }
    }

    private func add() {
        let p = pattern.trimmingCharacters(in: .whitespacesAndNewlines)
        busy = true
        Task {
            switch await DeviceOwner.confirm("Let \(agentName) \(ApprovalCard.phrase(request.title)) without asking, until the rule is removed") {
            case .success:
                model.alwaysAllow(threadID, requestId: request.requestId, pattern: p) { ok in
                    busy = false
                    if ok { dismiss() }
                }
            case .failure(let e):
                busy = false
                if let m = e.message { model.show(m, error: true) }
            }
        }
    }
}
