import SwiftUI
import UIKit

// The pieces Settings is built from: a row's symbol on a quiet tile with its words, the card at
// the top that says what this iPhone is connected to, and the pages for storage and for one
// machine's notifications.

/// A row's symbol on a neutral tile, as iOS's own Settings draws them, but in Trek's quiet
/// palette: colour is kept for status, never for decoration.
struct SettingsTile: View {
    var symbol: String
    var tint: Color = Trek.foreground
    @ScaledMetric(relativeTo: .body) private var side: CGFloat = 30

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: side * 0.5, weight: .medium))
            .foregroundStyle(tint)
            .frame(width: side, height: side)
            .background(Trek.foreground.opacity(0.07), in: RoundedRectangle(cornerRadius: side * 0.27, style: .continuous))
            .accessibilityHidden(true)
    }
}

/// A row's label: its tile, what the row is, and a line under it saying more when that helps.
/// The separator under the row starts at the words, as in iOS's Settings.
struct SettingsLabel: View {
    var title: String
    var subtitle: String? = nil
    var symbol: String? = nil
    var tint: Color = Trek.foreground

    var body: some View {
        HStack(spacing: 12) {
            if let symbol { SettingsTile(symbol: symbol, tint: tint) }
            VStack(alignment: .leading, spacing: 2) {
                Text(title).foregroundStyle(Trek.foreground)
                if let subtitle {
                    Text(subtitle)
                        .font(.ui(.footnote))
                        .foregroundStyle(Trek.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
        }
        .accessibilityElement(children: .combine)
    }
}

/// A row with its value at the end, kept on the row's own line. `LabeledContent` drops the value
/// under a label that has a second line, where it reads as a stray line of text. At the largest
/// text sizes the value goes under the words instead, as iOS's own Settings does: beside them it
/// left the title too little room and broke it mid-word ("Connec-tion").
struct SettingsValueRow<Value: View, Label: View>: View {
    var value: Value
    var label: Label
    @Environment(\.dynamicTypeSize) private var typeSize
    /// Where a row's words start, after its tile.
    @ScaledMetric(relativeTo: .body) private var indent: CGFloat = 42

    init(@ViewBuilder value: () -> Value, @ViewBuilder label: () -> Label) {
        self.value = value()
        self.label = label()
    }

    var body: some View {
        Group {
            if typeSize >= .xxLarge {
                VStack(alignment: .leading, spacing: 4) {
                    label
                    value
                        .foregroundStyle(Trek.muted)
                        .padding(.leading, indent)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                HStack(spacing: 10) {
                    label.frame(maxWidth: .infinity, alignment: .leading)
                    value
                        .foregroundStyle(Trek.muted)
                        .multilineTextAlignment(.trailing)
                        .fixedSize(horizontal: true, vertical: false)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// The top of Settings: the app's icon, what this iPhone is connected to and how safely, and a
/// chip for each machine with the way it's reached.
struct SettingsHeader: View {
    /// The icon chosen in Appearance (iOS doesn't say when it changes, so Settings hands it in).
    var icon: AppIconChoice
    @Environment(AppModel.self) private var model

    private var glances: [MachineGlance] {
        model.links.map { MachineGlance(name: $0.name, connection: $0.connection, detail: $0.connectionDetail, encrypted: $0.transport.isEncrypted) }
    }
    private var demo: Bool { model.mode == .demo }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 14) {
                Image(icon.preview)
                    .resizable()
                    .interpolation(.high)
                    .frame(width: 58, height: 58)
                    .clipShape(RoundedRectangle(cornerRadius: 13.5, style: .continuous))
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 3) {
                    Text(SettingsGlance.headline(glances, demo: demo))
                        .font(.ui(.title3).weight(.semibold))
                        .foregroundStyle(Trek.foreground)
                        .fixedSize(horizontal: false, vertical: true)
                    security
                    Text(activity)
                        .font(.ui(.subheadline))
                        .foregroundStyle(Trek.muted)
                }
            }
            if !model.links.isEmpty {
                FlowChips(spacing: 8) {
                    ForEach(model.links) { MachineChip(link: $0) }
                }
            }
            if demo {
                Button {
                    if let link = model.links.first { model.leave(link) }
                } label: {
                    Text("Pair with a Mac").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glass)
                .controlSize(.large)
            }
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .contain)
    }

    /// "Encrypted and pinned" with a lock, or how many connections aren't, in amber.
    @ViewBuilder
    private var security: some View {
        let plain = SettingsGlance.unencrypted(glances)
        Group {
            if demo {
                Label("Sample threads, on this iPhone", systemImage: "play.circle.fill")
                    .foregroundStyle(Trek.plan)
            } else if plain == 0 {
                Label("Encrypted and pinned", systemImage: "lock.fill")
                    .foregroundStyle(Trek.done)
            } else {
                Label(glances.count == 1 ? "Unencrypted connection" : "\(plain) of \(glances.count) unencrypted",
                      systemImage: "lock.open.fill")
                    .foregroundStyle(Trek.approval)
            }
        }
        .font(.ui(.subheadline).weight(.medium))
        .labelStyle(SettingsLineLabelStyle())
    }

    /// "2 working · 1 waiting on you", or that all is quiet.
    private var activity: String {
        var parts: [String] = []
        if model.workingCount > 0 { parts.append("\(model.workingCount) working") }
        if model.needsYouCount > 0 { parts.append("\(model.needsYouCount) waiting on you") }
        return parts.isEmpty ? "Nothing at work right now" : parts.joined(separator: " · ")
    }
}

/// A label with its icon close to its words, for a line of small text.
struct SettingsLineLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 5) {
            configuration.icon.font(.ui(.caption).weight(.semibold))
            configuration.title
        }
    }
}

/// A machine in the header: its mark, its name without the owner's, the way it's reached, and a
/// dot in its connection's colour.
struct MachineChip: View {
    var link: HostLink
    @Environment(AppModel.self) private var model

    var body: some View {
        let glance = MachineGlance(name: link.name, connection: link.connection, detail: link.connectionDetail, encrypted: link.transport.isEncrypted)
        HStack(spacing: 6) {
            MachineMark(kind: link.kind, size: 12)
            Text(MachineTag.short(link.name))
                .font(.ui(.footnote).weight(.medium))
                .foregroundStyle(Trek.foreground)
                .lineLimit(1)
            Text(model.mode == .demo ? "Demo" : SettingsGlance.way(glance))
                .font(.ui(.footnote))
                .foregroundStyle(Trek.muted)
                .lineLimit(1)
            Circle().fill(MachineStatus.color(link, demo: model.mode == .demo)).frame(width: 6, height: 6)
        }
        .padding(.leading, 9)
        .padding(.trailing, 11)
        .padding(.vertical, 6)
        .background(Trek.foreground.opacity(0.06), in: Capsule())
        .accessibilityElement(children: .combine)
    }
}

/// A machine's connection as a colour: green when connected, amber while connecting, red when it
/// can't be used; the demo's in indigo.
enum MachineStatus {
    static func color(_ link: HostLink, demo: Bool) -> Color {
        if demo { return Trek.plan }
        switch link.connection {
        case .connected: return Trek.done
        case .connecting: return Trek.approval
        default: return Trek.failed
        }
    }
}

/// Chips that wrap onto a new line when they don't fit (several machines, or large text).
struct FlowChips: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(subviews, width: proposal.width ?? .infinity)
        let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(0, rows.count - 1))
        let width = rows.map(\.width).max() ?? 0
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in arrange(subviews, width: bounds.width) {
            var x = bounds.minX
            for i in row.items {
                let size = subviews[i].sizeThatFits(ProposedViewSize(width: bounds.width, height: nil))
                subviews[i].place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(width: min(size.width, bounds.width), height: size.height))
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private struct Row { var items: [Int] = []; var width: CGFloat = 0; var height: CGFloat = 0 }

    private func arrange(_ subviews: Subviews, width: CGFloat) -> [Row] {
        var rows: [Row] = [Row()]
        for i in subviews.indices {
            let size = subviews[i].sizeThatFits(ProposedViewSize(width: width, height: nil))
            if !rows[rows.count - 1].items.isEmpty, rows[rows.count - 1].width + spacing + size.width > width {
                rows.append(Row())
            }
            var row = rows[rows.count - 1]
            row.width += (row.items.isEmpty ? 0 : spacing) + min(size.width, width)
            row.height = max(row.height, size.height)
            row.items.append(i)
            rows[rows.count - 1] = row
        }
        return rows
    }
}

/// Settings › Storage: what Trek keeps on this iPhone to open things at once, and Clear.
struct StorageView: View {
    @State private var usage: PhoneStorage.Usage?
    @State private var confirmClear = false
    @State private var clearing = false
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            Section {
                SettingsValueRow {
                    Text(usage.map { PhoneStorage.count($0.threads, $0.threadBytes) } ?? "…")
                } label: {
                    SettingsLabel(title: "Threads", subtitle: "The last \(TranscriptCache.keepThreads) you opened, so they show at once", symbol: "text.bubble")
                }
                SettingsValueRow {
                    Text(usage.map { PhoneStorage.count($0.files, $0.fileBytes) } ?? "…")
                } label: {
                    SettingsLabel(title: "Files", subtitle: "PDFs, videos and pictures opened from notes and threads", symbol: "doc")
                }
            } header: {
                Text("Kept on this iPhone")
            } footer: {
                Text("Copies only: your threads and notes stay on your machines, and come back from them the next time you open one. Files are kept up to \(ByteCountFormatter.string(fromByteCount: NoteFileStore.defaultLimit, countStyle: .memory)), the least recently opened going first.")
                    .font(.ui(.footnote))
            }

            Section {
                Button(role: .destructive) {
                    confirmClear = true
                } label: {
                    HStack {
                        Text("Clear saved threads and files")
                        Spacer()
                        if clearing { ProgressView().controlSize(.small) }
                    }
                }
                .disabled(clearing || usage?.bytes == 0)
            }
        }
        .scrollContentBackground(.hidden)
        .background(Trek.grouped)
        .navigationTitle("Storage")
        .navigationBarTitleDisplayMode(.inline)
        .task { usage = await PhoneStorage.measure() }
        .confirmationDialog("Clear saved threads and files?", isPresented: $confirmClear, titleVisibility: .visible) {
            Button("Clear", role: .destructive) {
                clearing = true
                Task {
                    await PhoneStorage.clear()
                    usage = await PhoneStorage.measure()
                    clearing = false
                    model.show("Cleared")
                }
            }
        } message: {
            Text("Nothing is lost: threads and files come back from your machines when you open them, a little slower the first time.")
        }
    }
}

/// One machine's push notifications on a page of their own (Settings › Notifications, with
/// several machines each has its own).
struct PushSettingsView: View {
    var link: HostLink
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            PhoneNotificationsSection(machine: link)
        }
        .scrollContentBackground(.hidden)
        .background(Trek.grouped)
        .navigationTitle("Notifications")
        .navigationSubtitle(model.links.count > 1 ? link.name : "")
        .navigationBarTitleDisplayMode(.inline)
        .task { model.loadSettings(on: link) }
    }
}

extension PushSettings {
    /// "When you're away, through ntfy", "Always, through ntfy", "Off".
    var summary: String {
        guard enabled else { return "Off" }
        return when == .away ? "When you're away, through ntfy" : "Always, through ntfy"
    }
}
