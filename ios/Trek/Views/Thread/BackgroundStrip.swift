import SwiftUI

/// Above the composer, as the Mac's strip (`background_strip.rs`): what the agent left running in
/// the background, whether a turn runs or not. One row, a row per task once opened: a spinner,
/// the command, "Running" and for how long, the last line it printed, and Stop. Tapping a row
/// opens the end of its output. It goes when the task does.
struct BackgroundStrip: View {
    var thread: ThreadSummary
    @Environment(AppModel.self) private var model
    @Environment(\.dynamicTypeSize) private var typeSize
    @State private var expanded = false
    @State private var open: Opened?
    @State private var listing = false
    @State private var confirmingStop: BackgroundTask?

    private struct Opened: Identifiable { var id: String }

    /// The Mac reads tasks' output and stops them for the phone; an older one only names them.
    private var served: Bool { model.serves(.background, on: thread.id) && !(thread.backgroundTasks ?? []).isEmpty }

    var body: some View {
        let tasks = thread.backgroundWork
        let shown = expanded ? tasks : Array(tasks.prefix(1))
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            VStack(spacing: 0) {
                ForEach(Array(shown.enumerated()), id: \.element.id) { n, task in
                    if n > 0 { Rectangle().fill(CardInk.chipBorder).frame(height: 0.5).padding(.leading, 38) }
                    row(task, more: n == 0 ? tasks.count - 1 : 0, now: ctx.date)
                }
            }
        }
        .background(CardInk.chipFill, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(CardInk.chipBorder, lineWidth: 1))
        .shadow(color: .black.opacity(0.05), radius: 1, y: 1)
        .animation(.snappy(duration: 0.25), value: expanded)
        .onChange(of: tasks.count) { _, n in if n < 2 { expanded = false } }
        .sheet(item: $open) { o in
            NavigationStack {
                BackgroundTaskView(threadID: thread.id, taskID: o.id)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button { open = nil } label: { Image(systemName: "checkmark") }
                                .accessibilityLabel("Done")
                        }
                    }
            }
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $listing) { AtWorkSheet(threadID: thread.id) }
        .confirmationDialog("Stop this task?", isPresented: Binding(get: { confirmingStop != nil }, set: { if !$0 { confirmingStop = nil } }),
                            titleVisibility: .visible, presenting: confirmingStop) { task in
            Button("Stop", role: .destructive) { model.stopBackground(thread.id, task: task.id) }
        } message: { task in
            Text("\(thread.agent.name) stops it on the Mac.")
        }
        .onAppear {
            if Launch.sheet == "background-strip" { expanded = true }
            if Launch.sheet == "background-output", served, let first = tasks.first { open = Opened(id: first.id) }
        }
    }

    private func row(_ task: BackgroundTask, more: Int, now: Date) -> some View {
        let stopping = task.stopping == true
        let stops = served && task.stoppable == true
        return HStack(spacing: 8) {
            Button {
                if served, !task.id.hasPrefix(ThreadSummary.titleOnly) { open = Opened(id: task.id) } else { listing = true }
            } label: {
                HStack(spacing: 10) {
                    RingSpinner(size: 13)
                        .frame(width: 16)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(BackgroundLine.oneLine(task.title))
                            .font(.code(.footnote))
                            .foregroundStyle(Trek.foreground.opacity(0.9))
                            .lineLimit(1)
                            .truncationMode(.tail)
                        HStack(spacing: 4) {
                            Text(stopping ? "Stopping…" : "Running").fixedSize()
                            if let since = task.since, !stopping {
                                Text(When.duration(max(0, Int(now.timeIntervalSince1970 - Double(since) / 1000)))).monospacedDigit()
                            }
                            // At the accessibility sizes there's no room for it: it's in the output.
                            if let line = task.line?.trimmingCharacters(in: .whitespaces), !line.isEmpty, !typeSize.isAccessibilitySize {
                                Text("·")
                                Text(line).font(.code(.caption2)).lineLimit(1).truncationMode(.tail).layoutPriority(-1)
                            }
                        }
                        .font(.ui(.caption))
                        .foregroundStyle(Trek.muted)
                        .lineLimit(1)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .combine)
            .accessibilityHint(served ? "Shows its output" : "Lists what runs in the background")
            if more > 0 {
                Button {
                    expanded.toggle()
                } label: {
                    HStack(spacing: 3) {
                        if !expanded { Text("+\(more)").monospacedDigit() }
                        Image(systemName: expanded ? "chevron.down" : "chevron.up").font(.system(size: 10, weight: .semibold))
                    }
                    .font(.ui(.caption).weight(.medium))
                    .foregroundStyle(Trek.muted)
                    .padding(.horizontal, 8)
                    .frame(height: 26)
                    .background(Trek.foreground.opacity(0.06), in: Capsule())
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(expanded ? "Show one" : "Show all \(more + 1)")
            }
            if stops {
                Button { confirmingStop = task } label: {
                    Image(systemName: "stop.fill")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(stopping ? Trek.muted.opacity(0.5) : Trek.foreground.opacity(0.75))
                        .frame(width: 28, height: 28)
                        .background(Trek.foreground.opacity(0.06), in: Circle())
                        .frame(width: 36, height: 36)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(stopping)
                .accessibilityLabel(stopping ? "Stopping" : "Stop \(BackgroundLine.oneLine(task.title))")
            }
        }
        .padding(.leading, 12)
        .padding(.trailing, stops ? 6 : 12)
        .frame(minHeight: 48)
    }
}
