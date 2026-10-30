import SwiftUI

/// A GitHub Actions run the agent watches (`gh run watch`): the workflow and what it's for, its
/// jobs as they queue, run and finish (each with how long it took), the time it has taken so far,
/// and how it ended: all passed (Open run), or what failed (Re-run, View log).
struct RunCard: View {
    var card: RemoteCard
    var act: ((CardAction) -> Void)?

    var body: some View {
        CardClock(live: !card.phase.terminal) { now in
            CardFrame(card: card, header: header(now: now)) {
                JobsSection(card: card, now: now)
            } footer: {
                PipelineOutcome(card: card, act: act, now: now)
            }
        }
    }

    private func header(now: Int64) -> CardHeader {
        let tile: (String, CardHue?) = switch card.phase {
        case .passed: ("check", .green)
        case .failed, .closed: ("x", .red)
        default: ("workflow", .blue)
        }
        var meta: [CardHeader.Meta] = []
        if !card.head.isEmpty { meta.append(.pill(card.head)) }
        let jobs = RemoteCard.jobs(card.stages.count)
        if !jobs.isEmpty { meta.append(.text(jobs)) }
        let took = card.tookMs(now: now)
        if took > 0 { meta.append(.text(RemoteCard.duration(took))) }
        let event = card.eventLabel
        return CardHeader(tile: tile, name: card.runName, rest: card.subject.isEmpty ? nil : card.subject,
                          badge: event.isEmpty ? nil : CardBadge(label: event, hue: nil), meta: meta)
    }
}

/// A run's or release's jobs: "JOBS (3) · 2 of 3 passed", then each with its mark and time.
struct JobsSection: View {
    var card: RemoteCard
    var now: Int64

    var body: some View {
        if !card.stages.isEmpty {
            CardDivider()
            VStack(spacing: 0) {
                CardSectionLabel(head: "JOBS (\(card.stages.count))", state: card.phase.terminal ? nil : card.pipelineState)
                ForEach(Array(card.stages.enumerated()), id: \.offset) { _, s in
                    CardJobRow(state: s.state, label: s.label, time: s.timeLabel(now: now))
                }
            }
            .padding(.bottom, 6)
        } else if !card.phase.terminal {
            CardDivider()
            HStack(spacing: 10) {
                RingSpinner(size: 14)
                Text(card.step.isEmpty ? "Waiting for the run to start" : card.step)
                    .font(.ui(size: 14, relativeTo: .subheadline))
                    .foregroundStyle(Trek.muted)
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 13)
        }
    }
}

/// How a run or release ended: passed or published (a quiet link to it), or failed (Re-run when
/// the Mac takes it from here, and View log). Nothing while it's still going: the jobs say that.
struct PipelineOutcome: View {
    var card: RemoteCard
    var act: ((CardAction) -> Void)?
    var now: Int64
    @Environment(\.openURL) private var openURL
    @State private var rerunPressed = false

    var body: some View {
        Group {
            switch card.phase {
            case .passed, .published:
                CardDivider()
                CardOutcome(tile: ("check", .green), title: doneTitle, detail: doneDetail, inline: link)
            case .failed, .closed:
                CardDivider()
                CardOutcome(tile: ("triangle-alert", .red), title: card.failureTitle, detail: card.failureDetail, inline: nil) {
                    EmptyView()
                } actions: {
                    CardActions {
                        if rerunPressed {
                            CardButton(label: "Re-running", style: .busy)
                        } else if card.actions.contains(.rerun), act != nil {
                            CardButton(label: "Re-run", style: .light) {
                                rerunPressed = true
                                act?(.rerun)
                            }
                        }
                        if let url = card.url.flatMap(URL.init(string:)) {
                            CardButton(label: "View log", style: .dark) { openURL(url) }
                        }
                    }
                }
            default:
                EmptyView()
            }
        }
        .onChange(of: card.phase) { rerunPressed = false }
    }

    private var doneTitle: String {
        if card.kind == .run { return card.stages.isEmpty ? "Passed" : "All checks passed" }
        return card.channel.isEmpty || card.channel == "stable" ? "Published" : "Published to \(card.channel)"
    }

    private var doneDetail: String {
        if card.kind == .run {
            let took = RemoteCard.duration(card.tookMs(now: now))
            return card.stages.isEmpty ? took : "\(RemoteCard.andList(card.stages.map(\.label))) · \(took)"
        }
        return "\(card.platformsForSub) downloads are up · \(RemoteCard.ago(max(card.phaseAtMs, card.createdMs), now: now))"
    }

    /// What's left to do with it: look at it on GitHub.
    private var link: CardButton? {
        if card.kind == .release, let notes = card.notesUrl.flatMap(URL.init(string:)) {
            return CardButton(label: "Notes", style: .link(.green)) { openURL(notes) }
        }
        guard let url = card.url.flatMap(URL.init(string:)) else { return nil }
        return CardButton(label: card.kind == .release ? "Run" : "Open run", style: .link(.green)) { openURL(url) }
    }
}
