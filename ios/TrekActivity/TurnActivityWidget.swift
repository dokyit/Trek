import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

// The Lock Screen card and the Dynamic Island. Both are always on a dark ground, as Night's, so the
// palette here is Night's. iOS keeps the app from running for long once it's left, and there's no
// push to wake it, so a card that has gone stale says so rather than go on looking live.

private enum Ink {
    static let ember = Color(red: 1, green: 0.478, blue: 0.239)
    static let amber = Color(red: 1, green: 0.69, blue: 0.125)
    static let done = Color(red: 0.247, green: 0.812, blue: 0.557)
    static let quiet = Color.white.opacity(0.62)
    static let button = Color.white.opacity(0.16)
}

private typealias State = TurnActivityAttributes.ContentState

struct TurnActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: TurnActivityAttributes.self) { context in
            TurnCard(state: context.state, stale: context.isStale)
                .activityBackgroundTint(Color(red: 0.04, green: 0.04, blue: 0.045).opacity(0.88))
                .activitySystemActionForegroundColor(.white)
                .widgetURL(context.state.link)
        } dynamicIsland: { context in
            let state = context.state
            let stale = context.isStale
            return DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    TurnMark(state: state, size: 30)
                        .padding(.leading, 6)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    TurnClock(state: state, stale: stale, tight: true)
                        .font(.subheadline.weight(.medium))
                        .padding(.trailing, 6)
                }
                DynamicIslandExpandedRegion(.center) {
                    VStack(spacing: 1) {
                        Text(state.title).font(.subheadline.weight(.semibold)).lineLimit(1)
                        if let project = state.project {
                            Text(project).font(.caption).foregroundStyle(Ink.quiet).lineLimit(1)
                        }
                    }
                }
                DynamicIslandExpandedRegion(.bottom) {
                    // Short of room here: what else is going on waits for the Lock Screen's card.
                    TurnDetail(state: state, stale: stale, tellsOthers: false)
                        .padding(.horizontal, 6)
                        .padding(.top, 4)
                }
            } compactLeading: {
                TurnMark(state: state, size: 20)
            } compactTrailing: {
                CompactTrailing(state: state, stale: stale)
            } minimal: {
                TurnMark(state: state, size: 20)
            }
            .widgetURL(state.link)
            .keylineTint(state.kind == .asking ? Ink.amber : Ink.ember)
        }
    }
}

/// The Lock Screen's: the thread, what it's doing or asking, and what else is going on.
private struct TurnCard: View {
    var state: State
    var stale: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                TurnMark(state: state, size: 32)
                VStack(alignment: .leading, spacing: 1) {
                    Text(state.title)
                        .font(.headline)
                        .foregroundStyle(.white)
                        .lineLimit(1)
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(Ink.quiet)
                        .lineLimit(1)
                }
                Spacer(minLength: 6)
                TurnClock(state: state, stale: stale)
                    .font(.subheadline.weight(.medium))
            }
            TurnDetail(state: state, stale: stale)
        }
        .padding(16)
    }

    private var subtitle: String {
        [state.project, state.agent].compactMap { $0 }.joined(separator: " · ")
    }
}

/// What it's doing or asking, the buttons when it can be answered from here, and the small print.
private struct TurnDetail: View {
    var state: State
    var stale: Bool
    var tellsOthers = true

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(state.line)
                .font(.subheadline)
                .foregroundStyle(state.kind == .asking ? Ink.amber : .white.opacity(0.88))
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let warning = state.warning {
                Label("Looks destructive: \(warning). Open Trek to read it all.", systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(Color(red: 1, green: 0.353, blue: 0.373))
                    .lineLimit(2)
            } else if let request = state.requestID {
                HStack(spacing: 8) {
                    Button(intent: AnswerAskIntent(thread: state.threadID, request: request, allow: false)) {
                        Text("Deny")
                            .font(.subheadline.weight(.medium))
                            .foregroundStyle(.white)
                            .frame(maxWidth: .infinity, minHeight: 34)
                            .background(Ink.button, in: Capsule())
                    }
                    .buttonStyle(.plain)
                    Button(intent: AnswerAskIntent(thread: state.threadID, request: request, allow: true)) {
                        Text("Allow")
                            .font(.subheadline.weight(.semibold))
                            .foregroundStyle(Color(red: 0.086, green: 0.039, blue: 0.016))
                            .frame(maxWidth: .infinity, minHeight: 34)
                            .background(Ink.ember, in: Capsule())
                    }
                    .buttonStyle(.plain)
                }
            } else if state.kind == .asking {
                Text("Open Trek to answer.").font(.caption).foregroundStyle(Ink.quiet)
            }
            if let footnote {
                Text(footnote).font(.caption).foregroundStyle(Ink.quiet).lineLimit(1)
            }
        }
    }

    private var footnote: String? {
        if stale { return "Last heard \(state.heard.formatted(date: .omitted, time: .shortened)). Open Trek for the latest." }
        return !tellsOthers || state.others.isEmpty ? nil : state.others
    }
}

/// Trek's mountain while it works, a hand when it waits on the user, a tick when it's over.
private struct TurnMark: View {
    var state: State
    var size: CGFloat

    var body: some View {
        Group {
            switch state.kind {
            case .asking:
                Image(systemName: "hand.raised.fill")
                    .resizable()
                    .scaledToFit()
                    .padding(size * 0.08)
                    .foregroundStyle(Ink.amber)
            case .finished:
                Image(systemName: "checkmark.circle.fill")
                    .resizable()
                    .scaledToFit()
                    .foregroundStyle(Ink.done)
            case .working, .waiting:
                Image("TrekMark")
                    .resizable()
                    .scaledToFit()
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// How long the turn has run, ticking by itself; once stale, no longer claiming to know.
private struct TurnClock: View {
    var state: State
    var stale: Bool
    /// In the island's corner: room for a few characters.
    var tight = false

    var body: some View {
        if state.kind == .finished {
            Text("Done").foregroundStyle(Ink.done)
        } else if state.kind == .asking {
            // The hand and the amber line say it waits; in the corner, how many more do.
            if tight, state.waitingOnYou > 1 {
                Text("+\(state.waitingOnYou - 1)")
                    .monospacedDigit()
                    .foregroundStyle(Ink.amber)
                    .accessibilityLabel("\(state.waitingOnYou - 1) more waiting on you")
            }
        } else if stale {
            Image(systemName: "clock").foregroundStyle(Ink.quiet).accessibilityLabel("Last heard a while ago")
        } else if let since = state.since {
            Text(timerInterval: since...Date.distantFuture, countsDown: false)
                .monospacedDigit()
                .multilineTextAlignment(.trailing)
                .foregroundStyle(Ink.ember)
                .frame(maxWidth: 64, alignment: .trailing)
        } else {
            Text(state.kind == .waiting ? "Waiting" : "Working").foregroundStyle(Ink.ember)
        }
    }
}

/// Beside the camera: how long, or how many wait on the user.
private struct CompactTrailing: View {
    var state: State
    var stale: Bool

    var body: some View {
        if state.kind == .asking {
            Text("\(max(state.waitingOnYou, 1))")
                .font(.subheadline.weight(.semibold))
                .monospacedDigit()
                .foregroundStyle(Ink.amber)
                .accessibilityLabel("\(max(state.waitingOnYou, 1)) waiting on you")
        } else if state.kind == .finished {
            Image(systemName: "checkmark").foregroundStyle(Ink.done)
        } else if stale {
            Image(systemName: "clock").foregroundStyle(Ink.quiet)
        } else if let since = state.since {
            Text(timerInterval: since...Date.distantFuture, countsDown: false)
                .font(.caption.weight(.medium))
                .monospacedDigit()
                .multilineTextAlignment(.trailing)
                .foregroundStyle(Ink.ember)
                .frame(maxWidth: 44)
        } else {
            Image(systemName: "ellipsis").foregroundStyle(Ink.ember)
        }
    }
}
