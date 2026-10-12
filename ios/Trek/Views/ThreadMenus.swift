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
