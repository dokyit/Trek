import SwiftUI

/// A small, muted icon button with a full-size tap target, for the rows under messages and turns.
struct RowIconButton: View {
    var systemImage: String
    var label: String
    var disabled = false
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            RowIcon(systemImage: systemImage)
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.35 : 1)
        .accessibilityLabel(label)
    }
}

private struct RowIcon: View {
    var systemImage: String
    var width: CGFloat = 36
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        let big = typeSize.isAccessibilitySize
        Image(systemName: systemImage)
            .font(.system(size: big ? 19 : 13, weight: .medium))
            .foregroundStyle(Trek.muted)
            .frame(width: big ? width * 1.25 : width, height: big ? 44 : 32)
            .contentShape(Rectangle())
    }
}

/// Under each turn, as on the Mac: copy the response, when it finished and how long it took, then
/// undo, retry (or retry with another model) and fork. The latest turn's are out; earlier turns
/// keep theirs in a menu, so the transcript stays quiet.
struct TurnActions: View {
    /// The turn's `turn_end` item.
    var end: String
    /// How long it worked (nil for a turn that stopped with an error, a limit or an interruption).
    var secs: Int?
    var at: Int64?
    var latest: Bool
    /// A turn is running: undo and retry wait for it.
    var busy: Bool
    /// Other models to retry with.
    var models: [ModelOption]
    /// The turn ran the project's verification CLI: the Mac's "Verified" badge.
    var verified: Verification? = nil
    var act: (RowAction) -> Void
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        if typeSize.isAccessibilitySize {
            // When it finished on a line of its own: beside the buttons, it pushed them off the side.
            VStack(alignment: .leading, spacing: 0) {
                finished.padding(.leading, 8)
                HStack(spacing: 0) {
                    copy
                    Spacer(minLength: 0)
                    actions
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            HStack(spacing: 0) {
                copy
                finished
                Rectangle().fill(Trek.border).frame(height: 0.5).padding(.horizontal, 10)
                actions
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var copy: some View {
        RowIconButton(systemImage: "doc.on.doc", label: "Copy response") { act(.copyResponse(end: end)) }
    }

    @ViewBuilder
    private var finished: some View {
        HStack(spacing: 0) {
            if at != nil || secs != nil {
                Text([at.map { When.clock($0) }, secs.map { When.duration($0) }].compactMap { $0 }.joined(separator: " · "))
                    .font(.ui(.caption).monospacedDigit())
                    .foregroundStyle(Trek.muted.opacity(0.85))
                    // On a line of its own at the accessibility sizes, where an older turn's date can need two.
                    .fixedSize(horizontal: !typeSize.isAccessibilitySize, vertical: true)
                    .accessibilityLabel("Finished\(at.map { " at \(When.clock($0))" } ?? "")\(secs.map { ", worked for \(When.duration($0))" } ?? "")")
            }
            if let verified {
                VerifiedBadge(verification: verified, compact: latest)
                    .padding(.leading, 8)
            }
        }
    }

    @ViewBuilder
    private var actions: some View {
            if latest {
                RowIconButton(systemImage: "arrow.uturn.backward", label: "Undo this turn", disabled: busy) { act(.undo(end: end)) }
                RowIconButton(systemImage: "arrow.clockwise", label: "Retry", disabled: busy) { act(.retry(end: end, model: nil)) }
                if !models.isEmpty {
                    Menu {
                        retryWith
                    } label: {
                        RowIcon(systemImage: "chevron.down", width: 24)
                    }
                    .disabled(busy)
                    .opacity(busy ? 0.35 : 1)
                    .accessibilityLabel("Retry with another model")
                }
                RowIconButton(systemImage: "arrow.triangle.branch", label: "Fork from here") { act(.fork(item: end)) }
            } else {
                Menu {
                    Button("Undo this turn", systemImage: "arrow.uturn.backward") { act(.undo(end: end)) }.disabled(busy)
                    Button("Retry", systemImage: "arrow.clockwise") { act(.retry(end: end, model: nil)) }.disabled(busy)
                    if !models.isEmpty {
                        Menu("Retry with", systemImage: "cpu") { retryWith }.disabled(busy)
                    }
                    Button("Fork from here", systemImage: "arrow.triangle.branch") { act(.fork(item: end)) }
                } label: {
                    RowIcon(systemImage: "ellipsis")
                }
                .accessibilityLabel("Turn actions")
            }
    }

    @ViewBuilder
    private var retryWith: some View {
        Section("Retry with") {
            ForEach(models) { m in
                Button(m.label) { act(.retry(end: end, model: m)) }
            }
        }
    }
}

/// Under each of the user's messages, as on the Mac: when it was sent, edit (rewind, then change
/// it in the composer), rewind, fork, and copy. The latest message's are out; earlier ones keep
/// theirs in a menu beside copy.
struct MessageActions: View {
    /// The message's `user` item.
    var item: String
    var text: String
    var at: Int64?
    var latest: Bool
    /// A turn is running: edit and rewind wait for it.
    var busy: Bool
    /// Not a prompt (a command Trek answered, a typed answer): there's nothing to go back to.
    var aside = false
    var act: (RowAction) -> Void

    var body: some View {
        BesideOrOver(overAlignment: .trailing, spacing: 0) {
            if let at {
                Text(When.clock(at))
                    .font(.ui(.caption).monospacedDigit())
                    .foregroundStyle(Trek.muted.opacity(0.85))
                    .padding(.trailing, 4)
                    .accessibilityLabel("Sent at \(When.clock(at))")
            }
            buttons
        }
        .padding(.trailing, -8)
    }

    private var buttons: some View {
        HStack(spacing: 0) {
            if aside {
                EmptyView()
            } else if latest {
                RowIconButton(systemImage: "pencil", label: "Edit", disabled: busy) { act(.rewind(user: item, edit: true)) }
                RowIconButton(systemImage: "arrow.uturn.backward", label: "Rewind to here", disabled: busy) { act(.rewind(user: item, edit: false)) }
                RowIconButton(systemImage: "arrow.triangle.branch", label: "Fork from here") { act(.fork(item: item)) }
            } else {
                Menu {
                    Button("Edit", systemImage: "pencil") { act(.rewind(user: item, edit: true)) }.disabled(busy)
                    Button("Rewind to here", systemImage: "arrow.uturn.backward") { act(.rewind(user: item, edit: false)) }.disabled(busy)
                    Button("Fork from here", systemImage: "arrow.triangle.branch") { act(.fork(item: item)) }
                } label: {
                    RowIcon(systemImage: "ellipsis")
                }
                .accessibilityLabel("Message actions")
            }
            RowIconButton(systemImage: "doc.on.doc", label: "Copy message") { act(.copy(text)) }
        }
    }
}
