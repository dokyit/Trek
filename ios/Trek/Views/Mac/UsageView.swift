import SwiftUI

/// Plan usage for every agent on the Mac that reports one, as the Mac's Usage popover shows it:
/// each limit's window, how much of it is used, and when it resets.
struct UsageView: View {
    @Environment(AppModel.self) private var model

    private var providers: [ProviderUsage] { model.usage?.providers ?? [] }
    private var asking: Bool { model.usageLoading || model.usage?.loading == true }

    var body: some View {
        ScrollView {
            GlassEffectContainer(spacing: 12) {
                VStack(spacing: 12) {
                    ForEach(providers) { ProviderCard(provider: $0) }
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
                                           description: Text("None of the agents on your Mac reported a plan."))
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

/// One agent's plan: its logo and name, the plan, then each limit as a bar.
private struct ProviderCard: View {
    var provider: ProviderUsage

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                AgentGlyph(key: provider.agent.key, size: 26)
                VStack(alignment: .leading, spacing: 1) {
                    Text(provider.agent.name).font(.headline)
                    if let plan = provider.plan {
                        Text(plan).font(.subheadline).foregroundStyle(Trek.muted)
                    }
                }
                Spacer()
                if let worst = provider.limits.map(\.percent).max() {
                    Text("\(Int(worst.rounded()))%")
                        .font(.title3.weight(.semibold).monospacedDigit())
                        .foregroundStyle(worst >= 90 ? Trek.failed : worst >= 70 ? Trek.approval : Trek.foreground)
                        .accessibilityLabel("Most used limit: \(Int(worst.rounded())) percent")
                }
            }
            if let error = provider.error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.footnote)
                    .foregroundStyle(Trek.approval)
            }
            if let note = provider.note {
                Text(note).font(.subheadline).foregroundStyle(Trek.muted)
            }
            if provider.limits.isEmpty, provider.error == nil, provider.note == nil {
                Text("No usage limits on this plan.").font(.subheadline).foregroundStyle(Trek.muted)
            }
            ForEach(provider.limits) { LimitRow(limit: $0) }
        }
        .glassCard(radius: 24, padding: 18)
    }
}

/// "5-hour limit · 42%", its bar, "Resets in 2h 10m".
private struct LimitRow: View {
    var limit: UsageLimit

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(limit.label).font(.subheadline.weight(.medium))
                if let window = limit.window {
                    Text(window)
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(Trek.muted)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Trek.foreground.opacity(0.07), in: Capsule())
                }
                Spacer()
                Text("\(Int(limit.percent.rounded()))% used")
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(Trek.muted)
            }
            UsageBar(percent: limit.percent, height: 6)
            if let at = limit.resetsAt {
                Text("Resets \(Resets.until(at))").font(.caption).foregroundStyle(Trek.muted)
            }
        }
        .accessibilityElement(children: .combine)
    }
}
