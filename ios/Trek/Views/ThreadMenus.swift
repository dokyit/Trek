import SwiftUI

/// The Mac's snooze menu for a thread: its presets, a time of one's own, and Wake now for one
/// that's snoozed.
struct SnoozeMenu: View {
    var thread: ThreadSummary
    /// "Pick a time…": the caller shows `SnoozePicker`.
    var pickTime: () -> Void
    @Environment(AppModel.self) private var model

    var body: some View {
        Menu {
            ForEach(SnoozePreset.allCases) { preset in
                Button(preset.label) { model.snooze(thread.id, preset) }
            }
            Button("Pick a time…", systemImage: "calendar") { pickTime() }
            if thread.section == .snoozed {
                Divider()
                Button("Wake now", systemImage: "sun.max") { model.wake(thread.id) }
            }
        } label: {
            Label(thread.section == .snoozed ? "Snoozed" : "Snooze", systemImage: "moon.zzz")
        }
    }
}

/// Snooze until a time of the user's choosing.
struct SnoozePicker: View {
    var thread: ThreadSummary
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var at = SnoozePicker.suggested

    /// Tomorrow at 9:00, the Mac's morning.
    static var suggested: Date {
        let calendar = Calendar.current
        let tomorrow = calendar.date(byAdding: .day, value: 1, to: .now) ?? .now
        return calendar.date(bySettingHour: 9, minute: 0, second: 0, of: tomorrow) ?? tomorrow
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 12) {
                Text(thread.title).font(.ui(.subheadline)).foregroundStyle(Trek.muted).lineLimit(2)
                DatePicker("Until", selection: $at, in: Date.now.addingTimeInterval(60)..., displayedComponents: [.date, .hourAndMinute])
                    .datePickerStyle(.graphical)
                Text("It comes back to the inbox then, or sooner if it needs you.")
                    .font(.ui(.footnote)).foregroundStyle(Trek.muted)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 20)
            .padding(.top, 8)
            .navigationTitle("Snooze")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Snooze") {
                        model.snooze(thread.id, until: at)
                        dismiss()
                    }
                    .fontWeight(.semibold)
                }
            }
        }
        .presentationDetents([.large])
    }
}

/// The Mac's words before a thread is deleted, waiting for the user.
struct DeleteAsk: Identifiable {
    var id: String
    var title: String
    var message: String
}

extension View {
    /// The Mac's Delete… confirmation: asks the Mac what would go (`ask`), then deletes once the
    /// user agrees; `deleted` runs after.
    func deleteConfirmation(_ ask: Binding<DeleteAsk?>, model: AppModel, deleted: @escaping (String) -> Void = { _ in }) -> some View {
        alert(ask.wrappedValue.map { "Delete “\($0.title)”?" } ?? "", isPresented: Binding(get: { ask.wrappedValue != nil }, set: { if !$0 { ask.wrappedValue = nil } }),
              presenting: ask.wrappedValue) { a in
            Button("Delete", role: .destructive) {
                model.deleteThread(a.id, confirmed: true, confirm: { _ in }, deleted: { deleted(a.id) })
            }
            Button("Cancel", role: .cancel) {}
        } message: { a in
            Text(a.message)
        }
    }
}

extension AppModel {
    /// Ask the Mac what deleting `thread` takes, for `deleteConfirmation`.
    func askToDelete(_ thread: ThreadSummary, into ask: Binding<DeleteAsk?>) {
        deleteThread(thread.id, confirm: { text in ask.wrappedValue = DeleteAsk(id: thread.id, title: thread.title, message: text) }, deleted: {})
    }
}

/// When a snoozed thread wakes, as its row says it: "until Tue 9:00".
enum SnoozeWords {
    static func until(_ ms: Int64?) -> String? {
        guard let ms else { return nil }
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        let sameDay = Calendar.current.isDateInToday(date)
        return "Until " + (sameDay ? date.formatted(date: .omitted, time: .shortened) : date.formatted(.dateTime.weekday(.abbreviated).hour().minute()))
    }
}

/// The thread's ⋯ menu, a view of its own that's equal while what it shows is. A working thread's
/// row changes every few seconds (its activity, clocks, line counts), and a menu rebuilt while
/// it's open lays itself out again under the finger: rows shift, the glass of the button it grew
/// from morphs again. So it shows nothing that ticks (the line counts are on the Git button),
/// and it fits under the button whole, Delete… included (the branch is under the title).
struct ThreadMenu: View, Equatable {
    /// What the menu shows, and the only thing it's compared on.
    struct Shown: Equatable {
        var title: String
        var agent: AgentRef
        /// "Claude Code · Opus 5.5".
        var name: String
        /// The Mac shows a plan for its agent: Usage behind the agent's row, and the plan's
        /// usage under it when there's a summary.
        var plan: Bool
        var planSummary: String?
        var subAgent: Bool
        var pinned: Bool
        var section: ThreadSection
        var working: Bool
        /// What its Mac serves.
        var snooze: Bool
        var regenerateTitle: Bool
        var markUnread: Bool
        var delete: Bool
    }

    var shown: Shown?
    /// For the snooze menu; not compared (`shown` says what's on screen).
    var thread: ThreadSummary?
    var usage: () -> Void
    var pickSnooze: () -> Void
    var rename: () -> Void
    var archive: () -> Void
    var delete: () -> Void
    @Environment(AppModel.self) private var model

    nonisolated static func == (a: ThreadMenu, b: ThreadMenu) -> Bool {
        MainActor.assumeIsolated { a.shown == b.shown }
    }

    var body: some View {
        Menu {
            if let s = shown, let t = thread {
                Section {
                    // A menu reads its title, subtitle and icon straight from these.
                    if s.plan {
                        Button(action: usage) {
                            AgentMenuIcon(agent: s.agent)
                            Text(s.name)
                            if let summary = s.planSummary { Text(summary) }
                        }
                    } else {
                        Label { Text(s.name) } icon: { AgentMenuIcon(agent: s.agent) }
                    }
                }
                // A sub-agent isn't in the list: what moves rows around doesn't apply to it.
                if !s.subAgent {
                    // The everyday three side by side, so the whole menu fits under the button
                    // without scrolling to Archive.
                    ControlGroup {
                        if s.pinned {
                            Button("Unpin", systemImage: "pin.slash") { model.threadAction(t.id, .unpin) }
                        } else {
                            Button("Pin", systemImage: "pin") { model.threadAction(t.id, .pin, done: "Pinned") }
                        }
                        if s.section == .settled {
                            Button("Back to the inbox", systemImage: "tray.and.arrow.down") { model.threadAction(t.id, .unsettle) }
                        } else {
                            Button("Settle", systemImage: "checkmark.circle") { model.threadAction(t.id, .settle, done: "Settled") }
                        }
                        Button("Copy title", systemImage: "doc.on.doc") { UIPasteboard.general.string = s.title }
                    }
                    Section {
                        if s.snooze {
                            SnoozeMenu(thread: t, pickTime: pickSnooze)
                        }
                        Button("Rename…", systemImage: "pencil", action: rename)
                        if s.regenerateTitle {
                            Button("Regenerate title", systemImage: "text.badge.star") { model.regenerateTitle(t.id) }
                        }
                        if s.markUnread {
                            Button("Mark unread", systemImage: "circlebadge") { model.markUnread(t.id) }
                        }
                    }
                }
            }
            if shown?.subAgent != false {
                Button("Copy title", systemImage: "doc.on.doc") { UIPasteboard.general.string = shown?.title }
            }
            if let s = shown, let t = thread {
                if s.working {
                    Button(s.subAgent ? "Stop sub-agent" : "Stop", systemImage: "stop.fill", role: .destructive) { model.interrupt(t.id) }
                }
                if !s.subAgent {
                    Button("Archive", systemImage: "archivebox", role: .destructive, action: archive)
                    if s.delete {
                        Button("Delete…", systemImage: "trash", role: .destructive, action: delete)
                    }
                }
            }
        } label: {
            Image(systemName: "ellipsis")
        }
    }
}

extension ThreadMenu.Shown {
    /// What `ThreadMenu` shows for `t`, as `model`'s Mac for it serves.
    @MainActor init(_ t: ThreadSummary, model: AppModel) {
        let plan = model.link(for: t.id)?.usage?.providers.first { $0.agent.key == t.agent.key }
        self.init(title: t.title, agent: t.agent, name: t.agent.name + (t.modelLabel.map { " · \($0)" } ?? ""),
                  plan: plan != nil, planSummary: plan?.summary, subAgent: t.parentId != nil,
                  pinned: t.pinned, section: t.section, working: t.runState == .working,
                  snooze: model.serves(.snooze, on: t.id), regenerateTitle: model.serves(.regenerateTitle, on: t.id),
                  markUnread: model.serves(.markUnread, on: t.id), delete: model.serves(.delete, on: t.id))
    }
}
