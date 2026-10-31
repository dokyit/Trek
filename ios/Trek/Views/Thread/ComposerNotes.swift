import SwiftUI

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
                }
            }
        }
        .padding(12)
        .background(Trek.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Trek.approval.opacity(0.35), lineWidth: 0.75))
        .accessibilityElement(children: .contain)
    }

    private func actions(compact: Bool) -> some View {
        HStack(spacing: 8) {
            if pause.resetsAt != nil {
                if pause.resume {
                    action("Cancel", strong: false) { model.threadAction(thread.id, .cancelResume) }
                } else {
                    action("Resume at reset", strong: true) { model.threadAction(thread.id, .resumeAtReset, done: "Resumes at the reset") }
                }
            } else {
                action("Try again", strong: true) { model.threadAction(thread.id, .tryAgain) }
            }
            if pause.resetsAt != nil, thread.snoozedUntil == nil {
                action(compact ? "Snooze" : "Snooze until reset", strong: false) {
                    model.threadAction(thread.id, .snoozeUntilReset, done: "Snoozed until the reset")
                }
            }
            action(compact ? "Switch…" : "Switch agent…", strong: false, action: switchAgent)
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
