import SwiftUI

/// A release the agent cut (`release.yml`, `script/release.sh`, `gh release create`): its version
/// and channel, the platforms it builds for and how long it has taken, its jobs building one by
/// one and then publishing, and how it ended: published (the downloads are up; its notes), or
/// what failed (Re-run, View log).
struct ReleaseCard: View {
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
        case .published: ("rocket", .green)
        case .failed, .closed: ("rocket", .red)
        default: ("rocket", .blue)
        }
        // "Release" strong, then the version.
        let parts = card.title.split(separator: " ", maxSplits: 1).map(String.init)
        let name = parts.first ?? "Release"
        let version = parts.count > 1 ? parts[1] : (card.version.isEmpty ? nil : "v\(card.version)")
        var meta: [CardHeader.Meta] = []
        if !card.head.isEmpty { meta.append(.pill(card.head)) }
        let platforms = card.platforms
        if !platforms.isEmpty { meta.append(.text(platforms)) }
        let took = card.tookMs(now: now)
        if took > 0 { meta.append(.text(RemoteCard.duration(took))) }
        // A long version ("v0.4.28-nightly.20261030") may break after its hyphen.
        let breakable = version?.replacingOccurrences(of: "-", with: "-\u{200B}")
        return CardHeader(tile: tile, name: name, rest: breakable,
                          badge: card.channel.isEmpty ? nil : CardBadge(label: card.channelLabel, hue: nil), meta: meta)
    }
}
