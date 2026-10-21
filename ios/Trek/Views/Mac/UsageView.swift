import SwiftUI

/// Plan usage as the Mac's Usage card shows it: the providers it shows (picked on the Mac, three
/// at most), each limit's window, how much of it is left and when it resets, and the resets
/// Codex granted, which can be spent here.
struct UsageView: View {
    @Environment(AppModel.self) private var model

    /// The machine shown (the picked one, else the first connected).
    private var link: HostLink? { model.selected }
    private var providers: [ProviderUsage] { link?.usage?.providers ?? [] }
    private var asking: Bool { link?.usageLoading == true || link?.usage?.loading == true }

    var body: some View {
        ScrollView {
            GlassEffectContainer(spacing: 12) {
                VStack(spacing: 12) {
                    ForEach(providers) { ProviderCard(provider: $0, link: link) }
                    if let picked = link?.usage?.picked, !providers.isEmpty {
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
                if link?.usage == nil {
                    ProgressView("Asking your agents…").foregroundStyle(Trek.muted)
                } else if !asking {
                    ContentUnavailableView("No plan usage", systemImage: "gauge.with.dots.needle.0percent",
                                           description: Text(link?.usage?.picked == true ? "No providers picked. Choose some on your Mac's Usage card."
                                                                                       : "None of the agents on your Mac reported a plan."))
                }
            }
        }
        .scrollEdgeEffectStyle(.hard, for: .bottom)
        .background(Trek.background)
        .navigationTitle("Usage")
        .navigationSubtitle(asking && !providers.isEmpty ? "Asking your agents…" : "")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) { MachinePicker() }
            if asking {
                ToolbarItem(placement: .topBarTrailing) { ProgressView().controlSize(.small) }
            }
        }
        .refreshable { await reload() }
        .task(id: link?.id) { model.loadUsage(on: link) }
    }

    private func reload() async {
        model.loadUsage(on: link)
        for _ in 0..<40 where link?.usageLoading == true {
            try? await Task.sleep(for: .milliseconds(100))
        }
    }
}

/// Usage on its own, over the Threads list or a thread, with Done.
struct UsageSheet: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            UsageView()
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Done", systemImage: "xmark") { dismiss() }
                    }
                }
        }
        .presentationDetents([.medium, .large])
    }
}

/// Usage's toolbar button: a gauge whose needle and tint follow the limit closest to running out
/// (amber from 80% used, red from 95%, as the Mac's Usage button).
struct UsageButton: View {
    @Environment(AppModel.self) private var model
    var action: () -> Void

    private var usage: Usage? { model.selected?.usage }
    private var peak: Double { usage?.peak ?? 0 }

    private var symbol: String {
        switch peak {
        case 95...: "gauge.with.dots.needle.100percent"
        case 60..<95: "gauge.with.dots.needle.67percent"
        case 40..<60: "gauge.with.dots.needle.50percent"
        case 15..<40: "gauge.with.dots.needle.33percent"
        default: "gauge.with.dots.needle.0percent"
        }
    }

    var body: some View {
        Button(action: action) {
            Label("Usage", systemImage: symbol)
                .foregroundStyle(peak >= 95 ? Trek.failed : peak >= 80 ? Trek.approval : Trek.foreground)
        }
        .accessibilityValue(usage.map { _ in "\(Int(max(0, 100 - peak).rounded())) percent left on the tightest limit" } ?? "")
    }
}

/// One agent's plan: its logo and name, the plan, then each limit as a bar, and its resets.
private struct ProviderCard: View {
    var provider: ProviderUsage
    var link: HostLink?
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
            if let resets = provider.resets, !resets.isEmpty, link?.serves(.useReset) == true {
                ResetCredits(resets: resets, link: link)
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
    var link: HostLink?
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
                model.useReset(r, on: link) { _ in using.remove(r.id) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Your rate limits reset right away. The credit is spent and can't be returned.")
        }
    }
}
