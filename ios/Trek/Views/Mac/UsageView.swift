import SwiftUI

/// Plan usage as the Mac's Usage card shows it: the providers it shows (picked on the Mac, three
/// at most), each limit's window, how much of it is left and when it resets, and the resets
/// Codex granted, which can be spent here.
struct UsageView: View {
    @Environment(AppModel.self) private var model

    private var providers: [ProviderUsage] { model.usage?.providers ?? [] }
    private var asking: Bool { model.usageLoading || model.usage?.loading == true }

    var body: some View {
        ScrollView {
            GlassEffectContainer(spacing: 12) {
                VStack(spacing: 12) {
                    ForEach(providers) { ProviderCard(provider: $0) }
                    if let picked = model.usage?.picked, !providers.isEmpty {
                        Text(picked ? "The providers you picked for the Usage card on your Mac. Change them there."
                                    : "The providers on your Mac's Usage card, picked by Trek. Choose others there.")
                            .font(.ui(.footnote))
                            .foregroundStyle(Trek.muted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 6)
                    }
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 24)
        }
        .overlay {
            if providers.isEmpty {
                if model.usage == nil {
                    ProgressView("Asking your agents…").foregroundStyle(Trek.muted)
                } else if !asking {
                    ContentUnavailableView("No plan usage", systemImage: "gauge.with.dots.needle.0percent",
                                           description: Text(model.usage?.picked == true ? "No providers picked. Choose some on your Mac's Usage card."
                                                                                       : "None of the agents on your Mac reported a plan."))
                }
            }
        }
        .scrollEdgeEffectStyle(.hard, for: .bottom)
        .background(Trek.background)
        .navigationTitle("Usage")
        .navigationSubtitle(asking && !providers.isEmpty ? "Asking your agents…" : "")
        .toolbar {
            if asking {
                ToolbarItem(placement: .topBarTrailing) { ProgressView().controlSize(.small) }
            }
        }
        .refreshable { await reload() }
        .task { model.loadUsage() }
    }

    private func reload() async {
        model.loadUsage()
        for _ in 0..<40 where model.usageLoading {
            try? await Task.sleep(for: .milliseconds(100))
        }
    }
}

/// One agent's plan: its logo and name, the plan, then each limit as a bar, and its resets.
private struct ProviderCard: View {
    var provider: ProviderUsage
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                AgentGlyph(provider.agent, size: 26)
                VStack(alignment: .leading, spacing: 1) {
                    Text(provider.agent.name).font(.ui(.headline))
                    if let plan = provider.plan {
                        Text(plan).font(.ui(.subheadline)).foregroundStyle(Trek.muted)
                    }
                }
                Spacer()
                if let worst = provider.limits.map(\.percent).max() {
                    let left = Int(max(0, 100 - worst).rounded())
                    Text("\(left)% left")
                        .font(.ui(.title3).weight(.semibold).monospacedDigit())
                        .foregroundStyle(worst >= 90 ? Trek.failed : worst >= 70 ? Trek.approval : Trek.foreground)
                        .accessibilityLabel("\(left) percent left on the tightest limit")
                }
            }
            if let error = provider.error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.ui(.footnote))
                    .foregroundStyle(Trek.approval)
            }
            if let note = provider.note {
                Text(note).font(.ui(.subheadline)).foregroundStyle(Trek.muted)
            }
            if provider.noLimits ?? (provider.limits.isEmpty && provider.error == nil && provider.note == nil) {
                Text("No usage limits on this plan.").font(.ui(.subheadline)).foregroundStyle(Trek.muted)
            }
            ForEach(provider.limits) { LimitRow(limit: $0) }
            if let today = provider.today, provider.limits.isEmpty {
                Text(today).font(.ui(.subheadline)).foregroundStyle(Trek.muted)
            }
            if let at = provider.asOf {
                let ago = When.short(at)
                Text(ago == "now" ? "As of just now" : "As of \(ago) ago").font(.ui(.caption)).foregroundStyle(Trek.muted)
            }
            if let resets = provider.resets, !resets.isEmpty, model.serves(.useReset) {
                ResetCredits(resets: resets)
            }
        }
        .card(padding: 18)
    }
}

/// "5-hour limit · 42%", its bar, "Resets in 2h 10m".
private struct LimitRow: View {
    var limit: UsageLimit

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(limit.label).font(.ui(.subheadline).weight(.medium))
                if let window = limit.window {
                    Text(window)
                        .font(.ui(.caption2).weight(.semibold))
                        .foregroundStyle(Trek.muted)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Trek.foreground.opacity(0.07), in: Capsule())
                }
                Spacer()
                Text("\(Int(max(0, 100 - limit.percent).rounded()))% left")
                    .font(.ui(.subheadline).monospacedDigit())
                    .foregroundStyle(Trek.muted)
            }
            UsageBar(percent: limit.percent, height: 6)
            if let at = limit.resetsAt {
                Text("Resets \(Resets.until(at))").font(.ui(.caption)).foregroundStyle(Trek.muted)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// "Usage limit resets · 2 available": Codex's granted resets, each with when it lapses and "Use
/// reset" behind the Mac's own warning.
private struct ResetCredits: View {
    var resets: [ResetCredit]
    @Environment(AppModel.self) private var model
    @State private var confirming: ResetCredit?
    @State private var using: Set<String> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Usage limit resets").font(.ui(.subheadline).weight(.medium))
                Spacer()
                Text("\(resets.count) available").font(.ui(.subheadline).weight(.medium)).foregroundStyle(Trek.done)
            }
            ForEach(resets) { r in
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(r.title).font(.ui(.subheadline)).lineLimit(2)
                        if let at = r.expiresAt {
                            Text("Expires \(Resets.until(at))").font(.ui(.caption)).foregroundStyle(Trek.muted)
                        }
                    }
                    Spacer(minLength: 6)
                    Button {
                        confirming = r
                    } label: {
                        if using.contains(r.id) { ProgressView().controlSize(.small) } else { Text("Use reset") }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(using.contains(r.id))
                }
            }
        }
        .padding(12)
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.75))
        .confirmationDialog(confirming.map { "Use “\($0.title)”?" } ?? "", isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
                            titleVisibility: .visible, presenting: confirming) { r in
            Button("Use reset", role: .destructive) {
                using.insert(r.id)
                model.useReset(r) { _ in using.remove(r.id) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Your rate limits reset right away. The credit is spent and can't be returned.")
        }
    }
}
