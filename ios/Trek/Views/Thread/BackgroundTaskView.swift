import SwiftUI

/// A background task in the list: what it is, the last line it printed, how long it has run.
struct BackgroundTaskRow: View {
    var task: BackgroundTask
    var now: Date

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: task.kind.symbol)
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(Trek.muted)
                .frame(width: 22)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 3) {
                Text(task.title).font(.code(.subheadline)).lineLimit(2)
                if let line = task.line {
                    Text(line).font(.code(.caption)).foregroundStyle(Trek.muted).lineLimit(1)
                }
            }
            Spacer(minLength: 6)
            if task.stopping == true {
                Text("Stopping…").font(.ui(.caption)).foregroundStyle(Trek.muted)
            } else if let since = task.since {
                Text(When.duration(max(0, Int(now.timeIntervalSince1970 - Double(since) / 1000))))
                    .font(.ui(.caption).monospacedDigit())
                    .foregroundStyle(Trek.muted)
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}

/// One background task: the end of its output, read every few seconds while it's open (as the
/// Mac's popover does), and Stop where its agent can stop it.
struct BackgroundTaskView: View {
    var threadID: String
    var taskID: String
    @Environment(AppModel.self) private var model
    @State private var output: String?
    @State private var read = false
    @State private var confirmingStop = false

    private var thread: ThreadSummary? { model.thread(threadID) }
    private var task: BackgroundTask? { thread?.backgroundTasks?.first { $0.id == taskID } }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if let task {
                        TimelineView(.periodic(from: .now, by: 1)) { ctx in
                            HStack(spacing: 8) {
                                Image(systemName: task.kind.symbol)
                                Text(task.kind.label)
                                if let since = task.since {
                                    Text("·")
                                    Text(When.duration(max(0, Int(ctx.date.timeIntervalSince1970 - Double(since) / 1000)))).monospacedDigit()
                                }
                            }
                            .font(.ui(.footnote))
                            .foregroundStyle(Trek.muted)
                        }
                        Text(task.title)
                            .font(.code(.callout).weight(.medium))
                            .textSelection(.enabled)
                    }
                    if let output, !output.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        Text(output)
                            .font(.code(.caption))
                            .lineSpacing(2)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(12)
                            .background(Trek.foreground.opacity(0.045), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    }
                    Text(note).font(.ui(.footnote)).foregroundStyle(Trek.muted)
                    Color.clear.frame(height: 1).id("end")
                }
                .padding(16)
            }
            .onChange(of: output) { proxy.scrollTo("end", anchor: .bottom) }
        }
        .background(Trek.background)
        .navigationTitle(task?.kind.label ?? "Background")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let task, task.stoppable == true {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(task.stopping == true ? "Stopping…" : "Stop", systemImage: "stop.fill", role: .destructive) { confirmingStop = true }
                        .labelStyle(.titleAndIcon)
                        .disabled(task.stopping == true)
                }
            }
        }
        .confirmationDialog("Stop this task?", isPresented: $confirmingStop, titleVisibility: .visible) {
            Button("Stop", role: .destructive) { model.stopBackground(threadID, task: taskID) }
        } message: {
            Text("\(thread?.agent.name ?? "The agent") stops it on the Mac.")
        }
        .task {
            // The end of its output, every couple of seconds while it's on screen.
            while !Task.isCancelled, task != nil {
                if let out = await model.backgroundOutput(threadID, task: taskID) {
                    output = out.output
                    read = true
                }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    /// As the Mac's popover says it.
    private var note: String {
        guard let task else { return "It isn't running any more." }
        if task.readable != true { return "\(thread?.agent.name ?? "The agent") doesn't share this task's output while it runs." }
        if let output, !output.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return "The end of its output, read every few seconds." }
        return read ? "Nothing printed yet." : "Reading its output…"
    }
}
