import SwiftUI

/// The composer's consult pill, as the Mac's: other models to ask before the agent acts, how they
/// work together (advice, a discussion, a design arena) and whether it then implements. The next
/// message goes with them (`send` with `consult`); the transcript shows the consult line under it.
struct ConsultSheet: View {
    var threadID: String
    @Binding var consult: ConsultRequest?
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var picked: [String: String] = [:]
    @State private var style: MessageParts.Style = .advise
    @State private var implement = true

    /// Picked model keys in the order they were listed: `agent/model` → effort.
    private var agents: [AgentOption] { model.link(for: threadID)?.agents ?? [] }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Picker("How", selection: $style) {
                        Text("Advise").tag(MessageParts.Style.advise)
                        Text("Discuss").tag(MessageParts.Style.discuss)
                        Text("Arena").tag(MessageParts.Style.arena)
                    }
                    .pickerStyle(.segmented)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
                    Toggle("Then implement", isOn: $implement)
                } footer: {
                    Text(styleHelp)
                }
                ForEach(agents) { agent in
                    Section {
                        ForEach(agent.models) { m in
                            row(agent, m)
                        }
                    } header: {
                        HStack(spacing: 6) {
                            AgentGlyph(agent.ref, size: 14)
                            Text(agent.name)
                        }
                    }
                }
            }
            .navigationTitle("Consult")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Clear") {
                        consult = nil
                        dismiss()
                    }
                    .disabled(consult == nil && picked.isEmpty)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        consult = picked.isEmpty ? nil : ConsultRequest(with: order(picked), style: style, implement: implement)
                        dismiss()
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .onAppear {
            guard let consult else { return }
            for key in consult.with {
                if let c = MessageParts.Consultant(key: key) { picked["\(c.agent)/\(c.model)"] = c.effort }
            }
            style = consult.style
            implement = consult.implement
        }
    }

    private var styleHelp: String {
        switch style {
        case .advise: "They review and suggest; the agent decides."
        case .discuss: "The agent and its consultants go back and forth until they agree."
        case .arena: "Each drafts a design; a judge on another model scores them blind; the agent builds on the best."
        }
    }

    private func row(_ agent: AgentOption, _ m: ModelOption) -> some View {
        let key = "\(agent.key)/\(m.id)"
        let on = picked[key] != nil
        let efforts = m.efforts ?? []
        return HStack(spacing: 10) {
            Button {
                if on { picked[key] = nil } else { picked[key] = efforts.contains("high") ? "high" : efforts.last ?? "high" }
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: on ? "checkmark.circle.fill" : "circle")
                        .foregroundStyle(on ? Trek.ember : Trek.muted)
                    Text(m.label).foregroundStyle(Trek.foreground)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if on, efforts.count > 1 {
                Menu {
                    ForEach(efforts, id: \.self) { e in
                        Button(Effort.label(e)) { picked[key] = e }
                    }
                } label: {
                    HStack(spacing: 3) {
                        Text(Effort.label(picked[key] ?? "high"))
                        Image(systemName: "chevron.up.chevron.down").font(.system(size: 9, weight: .semibold))
                    }
                    .font(.ui(.footnote))
                    .foregroundStyle(Trek.muted)
                }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    /// `agent/model/effort` keys, in the order the list shows them.
    private func order(_ picked: [String: String]) -> [String] {
        agents.flatMap { a in a.models.compactMap { m in picked["\(a.key)/\(m.id)"].map { "\(a.key)/\(m.id)/\($0)" } } }
    }
}

/// Over the composer while consultants are picked: who, and how; × drops them.
struct ConsultLine: View {
    var consult: ConsultRequest
    var threadID: String
    var clear: () -> Void
    var edit: () -> Void
    @Environment(AppModel.self) private var model

    var body: some View {
        let parts = MessageParts.Consult(consultants: consult.with.compactMap(MessageParts.Consultant.init(key:)),
                                         style: consult.style, implement: consult.implement, judge: nil)
        HStack(spacing: 8) {
            Button(action: edit) {
                HStack(spacing: 6) {
                    HStack(spacing: -4) {
                        ForEach(Array(parts.agents.prefix(4).enumerated()), id: \.offset) { _, key in
                            AgentGlyph(key: agents.first { $0.key == key }?.logo ?? key, size: 14)
                        }
                    }
                    Text(parts.line(name: name)).lineLimit(1).truncationMode(.tail)
                }
            }
            .buttonStyle(.plain)
            Spacer(minLength: 4)
            Button(action: clear) {
                Image(systemName: "xmark").font(.system(size: 11, weight: .semibold)).frame(width: 28, height: 28)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Don't consult")
        }
        .font(.ui(.caption).weight(.medium))
        .foregroundStyle(Trek.muted)
        .padding(.leading, 10)
        .padding(.trailing, 2)
        .background(Trek.surface, in: Capsule())
        .overlay(Capsule().strokeBorder(Trek.border, lineWidth: 0.5))
    }

    private var agents: [AgentOption] { model.link(for: threadID)?.agents ?? [] }

    private func name(_ c: MessageParts.Consultant) -> String {
        agents.first { $0.key == c.agent }?.models.first { ModelNaming.same(c.model, $0.id) }?.label ?? c.model
    }
}
