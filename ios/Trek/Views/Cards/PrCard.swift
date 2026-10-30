import SwiftUI

/// A pull request the agent opened, watches or merges, as the Mac's card follows it: being made
/// (the step it's on), its checks ticking while they run, its commits once there's nothing to
/// wait for, and the outcome: conflicts (Resolve conflicts, then the files clearing), ready to
/// merge (Merge), merging, merged.
struct PrCard: View {
    var card: RemoteCard
    var act: ((CardAction) -> Void)?
    /// A button pressed here: busy until the Mac's next update of the card says how it went.
    @State private var pressed: CardAction?
    @State private var confirmingMerge = false

    var body: some View {
        CardFrame(card: card, header: header) {
            middle
        } footer: {
            outcome
        }
        .onChange(of: card) { pressed = nil }
        .confirmationDialog(mergeQuestion, isPresented: $confirmingMerge, titleVisibility: .visible) {
            Button("Merge") { press(.merge) }
        } message: {
            Text("It merges on GitHub and deletes \(card.head.isEmpty ? "the branch" : card.head).")
        }
    }

    private var mergeQuestion: String {
        "Merge \(card.number.map { "#\($0)" } ?? "the pull request") into \(card.base.isEmpty ? "main" : card.base)?"
    }

    private func press(_ a: CardAction) {
        pressed = a
        act?(a)
    }

    // MARK: Header

    private var header: CardHeader {
        let tile: (String, CardHue?) = switch card.phase {
        case .merged: ("git-merge", .purple)
        case .creating, .closed: ("git-pull-request", nil)
        case .failed: ("git-pull-request", .red)
        default: ("git-pull-request", .green)
        }
        let badge: CardBadge? = switch card.phase {
        case .merged: CardBadge(label: "Merged", hue: .purple, icon: "git-merge")
        case .closed: CardBadge(label: "Closed", hue: nil)
        case .creating, .failed: nil
        default: card.number == nil ? nil : CardBadge(label: "Open", hue: .green, icon: "git-pull-request")
        }
        var meta: [CardHeader.Meta] = []
        if !card.head.isEmpty { meta += [.pill(card.head), .arrow] }
        if !card.base.isEmpty { meta.append(.pill(card.base)) }
        if let n = card.commits, n > 0 { meta.append(.text("\(n) commit\(n == 1 ? "" : "s")")) }
        let (a, r) = (card.added ?? 0, card.removed ?? 0)
        if a > 0 || r > 0 { meta.append(.lines(added: a, removed: r)) }
        return CardHeader(tile: tile, name: card.title.isEmpty ? "Pull request" : card.title,
                          rest: card.number.map { "#\($0)" }, restMuted: true, badge: badge, meta: meta)
    }

    // MARK: Between

    @ViewBuilder
    private var middle: some View {
        if card.phase == .creating {
            CardDivider()
            HStack(spacing: 10) {
                RingSpinner(size: 14)
                Text(card.step.isEmpty ? "Opening the pull request" : card.step)
                    .font(.ui(size: 14, relativeTo: .subheadline))
                    .foregroundStyle(Trek.muted)
                    .contentTransition(.opacity)
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 13)
        } else if showsChecks {
            CardDivider()
            VStack(spacing: 0) {
                CardSectionLabel(head: "CHECKS (\(card.checks.count))", state: checksState)
                ForEach(card.checks, id: \.name) { c in
                    CardJobRow(state: c.state, label: c.name)
                }
            }
            .padding(.bottom, 6)
        } else if !card.history.isEmpty {
            CardDivider()
            VStack(spacing: 0) {
                CardSectionLabel(head: "COMMITS (\(card.commits ?? card.history.count))", state: nil)
                ForEach(card.history.prefix(3), id: \.oid) { c in commitRow(c) }
                let more = (card.commits ?? card.history.count) - min(3, card.history.count)
                if more > 0 {
                    Text("\(more) earlier")
                        .font(.ui(size: 12, relativeTo: .caption))
                        .foregroundStyle(Trek.muted)
                        .padding(.leading, 39)
                        .frame(maxWidth: .infinity, minHeight: 24, alignment: .leading)
                }
            }
            .padding(.bottom, 6)
        }
    }

    /// Checks while they're the news: some still running, or one failed; once all have passed
    /// they're said in a line under Ready to merge, and the commits show instead.
    private var showsChecks: Bool {
        !card.checks.isEmpty && (card.phase == .checking || card.phase == .open)
            && card.checks.contains { $0.state != .pass }
    }

    private var checksState: String {
        let failed = card.checks.filter { $0.state == .fail }.count
        if failed > 0 { return "\(failed) failed" }
        return "\(card.checks.filter { $0.state == .pass }.count) of \(card.checks.count) passed"
    }

    private func commitRow(_ c: CardCommit) -> some View {
        HStack(spacing: 13) {
            Circle().fill(card.phase == .merged ? CardHue.purple.mark : CardHue.blue.mark).frame(width: 8, height: 8)
            Text(c.title)
                .font(.ui(size: 14, relativeTo: .subheadline))
                .foregroundStyle(Trek.foreground)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            CardClock(live: false) { now in
                HStack(spacing: 4) {
                    Text(c.oid.prefix(6)).font(.code(size: 12, relativeTo: .caption))
                    if let at = c.committedMs, at > 0 {
                        Text("· " + RemoteCard.ago(at, now: now).replacingOccurrences(of: " ago", with: "").replacingOccurrences(of: "just now", with: "now"))
                            .font(.ui(size: 12, relativeTo: .caption))
                    }
                }
                .foregroundStyle(Trek.muted)
            }
        }
        .padding(.leading, 18)
        .padding(.trailing, 14)
        .frame(minHeight: 32)
        .accessibilityElement(children: .combine)
    }

    // MARK: Outcome

    @ViewBuilder
    private var outcome: some View {
        let base = card.base.isEmpty ? "main" : card.base
        switch card.phase {
        case .conflicted:
            CardDivider()
            CardOutcome(tile: ("triangle-alert", .amber), title: card.conflictsTitle, detail: "The same lines changed on \(base)",
                        inline: nil) {
                CardFlow(spacing: 6, lineSpacing: 6) {
                    ForEach(card.resolved.prefix(4), id: \.self) { ConflictChip(path: $0, resolved: true) }
                    ForEach(card.conflicts.prefix(4), id: \.self) { ConflictChip(path: $0, resolved: false) }
                    let more = card.resolved.count + card.conflicts.count - min(4, card.resolved.count) - min(4, card.conflicts.count)
                    if more > 0 {
                        Text("+\(more) more").font(.ui(size: 12, relativeTo: .caption)).foregroundStyle(Trek.muted).frame(minHeight: 26)
                    }
                }
                .padding(.leading, 39)
            } actions: {
                if card.resolving || pressed == .resolve {
                    CardActions { CardButton(label: "Resolving", style: .busy) }
                } else if card.actions.contains(.resolve), act != nil {
                    CardActions { CardButton(label: "Resolve conflicts", style: .dark) { press(.resolve) } }
                }
            }
        case .ready, .merging:
            CardDivider()
            CardOutcome(tile: ("check", .green), title: "Ready to merge", detail: card.readyDetail, inline: mergeButton)
        case .merged:
            CardDivider()
            CardClock(live: false) { now in
                let when = RemoteCard.ago(max(card.phaseAtMs, card.createdMs), now: now)
                let n = card.commits ?? 0
                let detail = card.branchDeleted && !card.head.isEmpty
                    ? "\(card.head) was deleted · \(when)"
                    : n > 0 ? "\(n) commit\(n == 1 ? "" : "s") landed · \(when)" : when
                CardOutcome(tile: ("git-merge", .purple), title: "Merged into \(base)", detail: detail)
            }
        case .closed:
            CardDivider()
            CardClock(live: false) { now in
                CardOutcome(tile: ("circle-x", nil), title: "Closed unmerged",
                            detail: RemoteCard.ago(max(card.phaseAtMs, card.createdMs), now: now))
            }
        case .failed:
            CardDivider()
            CardOutcome(tile: ("triangle-alert", .red), title: "Couldn't open the pull request", detail: card.step)
        case .open where card.step.hasPrefix("Merge failed") || card.step.contains("failing"):
            // A merge that failed, or checks that are red: what went wrong, and where to look.
            let parts = card.step.split(separator: ": ", maxSplits: 1).map(String.init)
            CardDivider()
            CardOutcome(tile: ("triangle-alert", .red), title: parts.first ?? card.step, detail: parts.count > 1 ? parts[1] : "",
                        inline: card.url.flatMap(URL.init(string:)).map { url in
                            CardButton(label: "View", style: .link(.red)) { UIApplication.shared.open(url) }
                        })
        default:
            EmptyView()
        }
    }

    /// Merge, or Merging once pressed (here or on the Mac).
    private var mergeButton: CardButton? {
        if card.phase == .merging || card.merging || pressed == .merge { return CardButton(label: "Merging", style: .busy) }
        guard card.actions.contains(.merge), act != nil else { return nil }
        return CardButton(label: "Merge", style: .dark, icon: "git-merge") { confirmingMerge = true }
    }
}

/// A conflicting file: its language's tag and name, with a green tick once resolved.
struct ConflictChip: View {
    var path: String
    var resolved: Bool

    var body: some View {
        let name = (path as NSString).lastPathComponent
        let ext = (name as NSString).pathExtension.uppercased()
        HStack(spacing: 6) {
            if !ext.isEmpty, ext.count <= 4 {
                Text(ext).font(.code(size: 10, weight: .bold, relativeTo: .caption2)).foregroundStyle(CardHue.blue.text.opacity(0.8))
            }
            Text(name).font(.ui(size: 13, relativeTo: .footnote)).foregroundStyle(Trek.foreground.opacity(0.72)).lineLimit(1)
            if resolved {
                CardIcon(name: "check", size: 11).foregroundStyle(CardHue.green.text)
                    .transition(.scale.combined(with: .opacity))
            }
        }
        .padding(.horizontal, 10)
        .frame(minHeight: 26)
        .background(Trek.background, in: Capsule())
        .overlay(Capsule().strokeBorder(CardInk.hairline.opacity(0.8), lineWidth: 1))
        .accessibilityElement(children: .combine)
        .accessibilityValue(resolved ? "resolved" : "conflicting")
    }
}
