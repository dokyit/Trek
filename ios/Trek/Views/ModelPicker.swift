import SwiftUI

/// Names a thread's model as the Mac's composer does (`composer::model_name`, `default_model`).
enum ModelNaming {
    /// True when `id` is `candidate` or a dated snapshot of it (`claude-haiku-4-5-20251001`).
    static func same(_ id: String, _ candidate: String) -> Bool {
        if id == candidate { return true }
        guard id.hasPrefix(candidate + "-") else { return false }
        let date = id.dropFirst(candidate.count + 1)
        return date.count == 8 && date.allSatisfy(\.isNumber)
    }

    /// The agent's model when the thread hasn't picked one: Opus 5.5 for Claude, else the first.
    static func defaultModel(_ agent: AgentOption?) -> ModelOption? {
        guard let agent else { return nil }
        return agent.models.first { $0.id == "claude-opus-5-5" }
            ?? agent.models.first { $0.id == agent.defaultModel }
            ?? agent.models.first
    }

    /// The model a thread runs: its own pick, else its agent's default.
    static func current(_ modelID: String?, agent: AgentOption?) -> ModelOption? {
        if let id = modelID {
            return agent?.models.first { same(id, $0.id) } ?? ModelOption(id: id, label: id)
        }
        return defaultModel(agent)
    }
}

/// The model chip's label, as on the Mac: the agent's logo, the model, its effort in muted text.
struct ModelChipLabel: View {
    var agentKey: String
    var model: String
    var effort: String?
    var logo: CGFloat = 15

    var body: some View {
        HStack(spacing: 5) {
            AgentGlyph(key: agentKey, size: logo)
            Text(model).lineLimit(1)
            if let effort {
                Text(Effort.label(effort)).foregroundStyle(Trek.muted).lineLimit(1)
            }
            Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold)).foregroundStyle(Trek.muted)
        }
    }
}

/// The model picker, in the spirit of the Mac's model popover: a rail of providers (their logos)
/// down the side, the chosen provider's models beside it with the current one checked, and the
/// effort underneath, Faster to Smarter. Picking a model from another provider switches the
/// thread's agent and model in one go.
struct ModelPickerSheet: View {
    var agents: [AgentOption]
    var agentKey: String
    var modelID: String?
    var effort: String?
    /// The efforts `model` of `agent` takes, lowest first.
    var efforts: (_ agent: String, _ model: String?) -> [String]
    var pickModel: (_ agent: String, _ model: String) -> Void
    var pickEffort: (String) -> Void

    @State private var rail: String?
    @Environment(\.dismiss) private var dismiss

    private var agent: AgentOption? { agents.first { $0.key == agentKey } }
    private var current: ModelOption? { ModelNaming.current(modelID, agent: agent) }
    private var shown: AgentOption? { agents.first { $0.key == (rail ?? agentKey) } ?? agent }
    private var currentEfforts: [String] { efforts(agentKey, current?.id) }

    /// Tall enough for every provider on the rail and the longest model list, up to most of the
    /// screen (it scrolls, or drags up to full height, past that).
    private var fitHeight: CGFloat {
        let rail = CGFloat(agents.count) * 56 + 8
        let list = CGFloat(agents.map(\.models.count).max() ?? 0) * 64 + 40
        let effort: CGFloat = currentEfforts.isEmpty ? 16 : 128
        return min(96 + max(rail, list) + effort, 640)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            HStack(alignment: .top, spacing: 0) {
                railColumn
                Rectangle().fill(Trek.border).frame(width: 0.5).padding(.vertical, 4)
                modelList
            }
            .frame(maxHeight: .infinity)
            if !currentEfforts.isEmpty {
                effortSection
            }
        }
        .presentationDetents([.height(fitHeight), .large])
        .presentationDragIndicator(.visible)
        .sensoryFeedback(.selection, trigger: modelID)
        .sensoryFeedback(.selection, trigger: effort)
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text("Model").font(.title3.weight(.semibold))
                HStack(spacing: 5) {
                    AgentGlyph(key: agentKey, size: 13)
                    Text([agent?.name, current?.label, effort.map(Effort.label)].compactMap { $0 }.joined(separator: " · "))
                        .lineLimit(1)
                }
                .font(.footnote)
                .foregroundStyle(Trek.muted)
            }
            Spacer()
            Button {
                dismiss()
            } label: {
                Image(systemName: "checkmark")
                    .font(.system(size: 15, weight: .semibold))
                    .frame(width: 40, height: 40)
            }
            .buttonStyle(.glass)
            .buttonBorderShape(.circle)
            .accessibilityLabel("Done")
        }
        .padding(.horizontal, 20)
        .padding(.top, 22)
        .padding(.bottom, 12)
    }

    private var railColumn: some View {
        ScrollView(showsIndicators: false) {
            VStack(spacing: 6) {
                ForEach(agents) { a in
                    let browsing = a.key == (rail ?? agentKey)
                    Button {
                        withAnimation(.snappy(duration: 0.2)) { rail = a.key }
                    } label: {
                        AgentGlyph(key: a.key, size: 26)
                            .frame(width: 50, height: 50)
                            .background {
                                RoundedRectangle(cornerRadius: 14, style: .continuous)
                                    .fill(browsing ? Trek.foreground.opacity(0.09) : .clear)
                            }
                            .overlay {
                                if browsing {
                                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                                        .strokeBorder(Trek.foreground.opacity(0.12), lineWidth: 0.75)
                                }
                            }
                            .overlay(alignment: .bottomTrailing) {
                                // The thread's own agent, wherever the rail is.
                                if a.key == agentKey {
                                    Image(systemName: "checkmark.circle.fill")
                                        .font(.system(size: 13, weight: .semibold))
                                        .symbolRenderingMode(.palette)
                                        .foregroundStyle(Trek.background, Trek.foreground)
                                        .offset(x: 2, y: 2)
                                }
                            }
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(a.name)
                    .accessibilityAddTraits(browsing ? .isSelected : [])
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 4)
        }
        .frame(width: 74)
    }

    private var modelList: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                if let shown {
                    Text(shown.name)
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(Trek.muted)
                        .padding(.horizontal, 12)
                        .padding(.bottom, 6)
                    if shown.models.isEmpty {
                        Text("No models on offer.").font(.subheadline).foregroundStyle(Trek.muted).padding(12)
                    }
                    ForEach(shown.models) { m in
                        modelRow(m, of: shown)
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
        }
        .id(shown?.key)
        .transition(.opacity)
    }

    private func modelRow(_ m: ModelOption, of a: AgentOption) -> some View {
        let selected = a.key == agentKey && current.map { ModelNaming.same($0.id, m.id) } == true
        let isDefault = ModelNaming.defaultModel(a)?.id == m.id
        return Button {
            pickModel(a.key, m.id)
        } label: {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(m.label).font(.body.weight(selected ? .semibold : .regular)).foregroundStyle(Trek.foreground)
                        if isDefault {
                            Text("Default")
                                .font(.caption2.weight(.semibold))
                                .foregroundStyle(Trek.muted)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(Trek.foreground.opacity(0.06), in: Capsule())
                        }
                    }
                    Text(m.id).font(.system(.caption, design: .monospaced)).foregroundStyle(Trek.muted).lineLimit(1)
                }
                Spacer(minLength: 6)
                if selected {
                    Image(systemName: "checkmark").font(.system(size: 15, weight: .semibold)).foregroundStyle(Trek.foreground)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background {
                if selected {
                    RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Trek.foreground.opacity(0.06))
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var effortSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Effort").font(.subheadline.weight(.semibold))
                Spacer()
                HStack(spacing: 4) {
                    Text("Faster")
                    Image(systemName: "arrow.left.and.right").font(.system(size: 9, weight: .semibold))
                    Text("Smarter")
                }
                .font(.caption)
                .foregroundStyle(Trek.muted)
            }
            EffortPicker(efforts: currentEfforts, selection: effort, pick: pickEffort)
        }
        .padding(.horizontal, 20)
        .padding(.top, 14)
        .padding(.bottom, 18)
        .overlay(alignment: .top) { Rectangle().fill(Trek.border).frame(height: 0.5) }
    }
}

/// Efforts side by side, lowest first, each with a little meter that fills as effort rises; the
/// current one lifted on a glass-like plate that glides to the next.
struct EffortPicker: View {
    var efforts: [String]
    var selection: String?
    var pick: (String) -> Void
    @Namespace private var plate

    var body: some View {
        HStack(spacing: 2) {
            ForEach(Array(efforts.enumerated()), id: \.element) { i, e in
                let on = e == selection
                Button {
                    withAnimation(.snappy(duration: 0.25)) { pick(e) }
                } label: {
                    VStack(spacing: 5) {
                        Meter(level: i + 1, of: efforts.count, on: on)
                        Text(Effort.label(e))
                            .font(.footnote.weight(on ? .semibold : .medium))
                            .foregroundStyle(on ? Trek.foreground : Trek.muted)
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 9)
                    .background {
                        if on {
                            RoundedRectangle(cornerRadius: 13, style: .continuous)
                                .fill(Trek.elevated)
                                .shadow(color: .black.opacity(0.12), radius: 6, y: 2)
                                .overlay(RoundedRectangle(cornerRadius: 13, style: .continuous).strokeBorder(Trek.foreground.opacity(0.1), lineWidth: 0.75))
                                .matchedGeometryEffect(id: "plate", in: plate)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Effort \(Effort.label(e))")
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(3)
        .background(Trek.foreground.opacity(0.06), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    }

    private struct Meter: View {
        var level: Int
        var of: Int
        var on: Bool

        var body: some View {
            HStack(alignment: .bottom, spacing: 2) {
                ForEach(0..<of, id: \.self) { i in
                    RoundedRectangle(cornerRadius: 1)
                        .fill(i < level ? (on ? Trek.foreground : Trek.muted.opacity(0.8)) : Trek.muted.opacity(0.22))
                        .frame(width: 3, height: 4 + CGFloat(i) * 2)
                }
            }
            .frame(height: 4 + CGFloat(max(of - 1, 0)) * 2)
            .accessibilityHidden(true)
        }
    }
}
