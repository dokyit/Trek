import SwiftUI

/// Over the composer, as the Mac's: this thread's turn was cut off when Trek closed, how its agent
/// comes back, and Resume. Words and buttons as the Mac's own bar has them (`interrupted_bar`).
struct CutShortBar: View {
    var thread: ThreadSummary
    var cut: CutShort
    @Environment(AppModel.self) private var model

    private var tint: Color {
        switch cut.state {
        case .failed: Trek.failed
        case .resuming: Trek.muted
        case .offered: Trek.approval
        }
    }

    var title: String {
        switch cut.state {
        case .resuming: "Resuming…"
        case .failed: "Couldn't resume"
        case .offered: cut.waiting ? "Waiting for you when Trek closed" : "Interrupted"
        }
    }

    var detail: String {
        switch cut.state {
        case .resuming: cut.how
        case .failed: cut.why ?? cut.how
        case .offered: cut.waiting ? (cut.asked ?? "Its question went with Trek. Ask again to bring it back.") : cut.how
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                if cut.state == .resuming {
                    RingSpinner(size: 13)
                } else {
                    Image(systemName: cut.state == .failed ? "exclamationmark.triangle.fill" : "bolt.horizontal.circle")
                        .foregroundStyle(tint)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.ui(.subheadline).weight(.semibold)).foregroundStyle(Trek.foreground)
                    Text(detail).font(.ui(.footnote)).foregroundStyle(Trek.muted).lineLimit(3)
                }
                Spacer(minLength: 0)
            }
            .accessibilityElement(children: .combine)
            if cut.state != .resuming {
                HStack(spacing: 8) {
                    action(cut.state == .failed ? "Retry" : cut.waiting ? "Ask again" : "Resume", strong: true) {
                        model.resumeCutShort(thread.id)
                    }
                    action("Dismiss", strong: false) { model.dismissCutShort(thread.id) }
                    Spacer(minLength: 0)
                }
            }
        }
        .padding(12)
        .background(Trek.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(tint.opacity(0.35), lineWidth: 0.75))
    }

    private func action(_ title: String, strong: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.ui(.footnote).weight(strong ? .semibold : .medium))
                .foregroundStyle(strong ? CardInk.onButton : Trek.foreground)
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .background(strong ? CardInk.button : Trek.foreground.opacity(0.06), in: Capsule())
                .fixedSize()
        }
        .buttonStyle(CardPress())
    }
}

/// Over the composer while the thread is paused at its agent's usage limit, as the Mac's limit
/// bar (`composer.rs`): what happened, when the limit resets, and what to do about it — resume
/// by itself at the reset (or not), try again now, snooze the thread until then, or switch to
/// another agent or model.
struct LimitBar: View {
    var thread: ThreadSummary
    var pause: LimitPause
    /// Opens the model picker (Switch agent…).
    var switchAgent: () -> Void
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: pause.wrapped ? "pause.circle.fill" : "clock.badge.exclamationmark.fill")
                    .foregroundStyle(Trek.approval)
                VStack(alignment: .leading, spacing: 2) {
                    Text(pause.wrapped ? "Wrapped up before the limit" : "Usage limit reached")
                        .font(.ui(.subheadline).weight(.semibold))
                        .foregroundStyle(Trek.foreground)
                    TimelineView(.periodic(from: .now, by: 30)) { ctx in
                        Text(Self.detail(pause, now: ctx.date))
                            .font(.ui(.footnote))
                            .foregroundStyle(Trek.muted)
                    }
                }
                Spacer(minLength: 0)
            }
            if model.serves(.limitActions, on: thread.id) {
                // In the card's width: the long words where they fit, the Mac's short ones where not.
                ViewThatFits(in: .horizontal) {
                    actions(compact: false)
                    actions(compact: true)
                    // With another account offered too: that on a row of its own.
                    if let other {
                        VStack(alignment: .leading, spacing: 8) {
                            action("Continue on \(other.label)", strong: true) { model.continueOnAccount(thread.id, other) }
                            actions(compact: true, offering: false)
                        }
                    }
                }
            }
        }
        .padding(12)
        .background(Trek.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Trek.approval.opacity(0.35), lineWidth: 0.75))
        .accessibilityElement(children: .contain)
    }

    /// Another account of the agent with room, where the Mac offers it: the quickest way on, and
    /// the bar says less of the rest then (as the Mac's).
    private var other: OtherAccount? {
        model.serves(.continueOnAccount, on: thread.id) ? pause.otherAccount : nil
    }

    private func actions(compact: Bool, offering: Bool = true) -> some View {
        HStack(spacing: 8) {
            if offering, let other {
                action("Continue on \(other.label)", strong: true) { model.continueOnAccount(thread.id, other) }
                    .accessibilityHint("Goes on now in a new session there, from a recap of this thread")
            }
            if pause.resetsAt != nil {
                if pause.resume {
                    action("Cancel", strong: false) { model.threadAction(thread.id, .cancelResume) }
                } else {
                    action("Resume at reset", strong: other == nil) { model.threadAction(thread.id, .resumeAtReset, done: "Resumes at the reset") }
                }
            } else {
                action("Try again", strong: true) { model.threadAction(thread.id, .tryAgain) }
            }
            if pause.resetsAt != nil, thread.snoozedUntil == nil, other == nil {
                action(compact ? "Snooze" : "Snooze until reset", strong: false) {
                    model.threadAction(thread.id, .snoozeUntilReset, done: "Snoozed until the reset")
                }
            }
            action(compact || other != nil ? "Switch…" : "Switch agent…", strong: false, action: switchAgent)
            Spacer(minLength: 0)
        }
    }

    private func action(_ title: String, strong: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.ui(.footnote).weight(strong ? .semibold : .medium))
                .foregroundStyle(strong ? CardInk.onButton : Trek.foreground)
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .background(strong ? CardInk.button : Trek.foreground.opacity(0.06), in: Capsule())
                .fixedSize()
        }
        .buttonStyle(CardPress())
    }

    /// "Resumes at 4:30 PM (in 2h 14m)", "Resets 4:30 PM (in 2h 14m)", "Reset time unknown".
    static func detail(_ pause: LimitPause, now: Date) -> String {
        guard let at = pause.resetsAt else { return "Reset time unknown" }
        let left = max(0, Int(Double(at) / 1000 - now.timeIntervalSince1970))
        let clock = When.clock(at)
        let when = left > 0 ? "\(clock) (in \(When.duration(left)))" : clock
        return pause.resume ? "Resumes at \(when)" : "Resets \(when)"
    }
}

/// Under the chips, as the Mac's composer status strip says it: another thread edits this folder.
struct FolderSharedLine: View {
    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Trek.approval)
            Text("Another thread is also editing this folder")
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 0)
        }
        .font(.ui(.caption))
        .foregroundStyle(Trek.muted)
        .padding(.horizontal, 4)
        .accessibilityElement(children: .combine)
    }
}
