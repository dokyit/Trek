import SwiftUI

/// The `/` picker above the composer, as the Mac's: typing `/` at the start lists the thread's
/// slash commands (Trek's own first, then the agent's commands, skills and agents), narrowed as
/// the user types. Tapping one puts it in the message, ready for its arguments.
struct CommandPicker: View {
    var commands: [CommandInfo]
    /// What's typed after the slash.
    var query: String
    var pick: (CommandInfo) -> Void

    /// The text after the slash, while the message is still a command being named; nil otherwise.
    static func query(_ text: String) -> String? {
        guard text.hasPrefix("/"), !text.contains("\n") else { return nil }
        return String(text.dropFirst())
    }

    /// Names starting with the query first, then names containing it, then descriptions.
    static func matches(_ commands: [CommandInfo], _ query: String) -> [CommandInfo] {
        let q = query.lowercased()
        if q.isEmpty { return commands }
        let prefix = commands.filter { $0.name.lowercased().hasPrefix(q) }
        // A command typed out with its arguments under way: nothing left to pick.
        if q.contains(" ") { return prefix.filter { $0.name.lowercased() != q.trimmingCharacters(in: .whitespaces) } }
        let inName = commands.filter { !$0.name.lowercased().hasPrefix(q) && $0.name.lowercased().contains(q) }
        let inText = q.count < 2 ? [] : commands.filter { !$0.name.lowercased().contains(q) && $0.description.lowercased().contains(q) }
        return prefix + inName + inText
    }

    var body: some View {
        let list = Self.matches(commands, query)
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(list) { c in
                    Button { pick(c) } label: { row(c) }
                        .buttonStyle(.plain)
                }
            }
            .padding(6)
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(maxHeight: min(CGFloat(list.count) * 54 + 12, 236))
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .opacity(list.isEmpty ? 0 : 1)
        .accessibilityLabel("Commands")
    }

    private func row(_ c: CommandInfo) -> some View {
        HStack(spacing: 10) {
            Image(systemName: symbol(c))
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(c.trek == true ? Trek.ember : Trek.muted)
                .frame(width: 26, height: 26)
                .background(Trek.foreground.opacity(0.06), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    Text("/\(c.name)").font(.system(.subheadline, design: .monospaced).weight(.medium)).lineLimit(1)
                    if c.kind != .command {
                        Text(c.kind == .skill ? "skill" : "agent")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(Trek.muted)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(Trek.foreground.opacity(0.07), in: Capsule())
                    }
                }
                if !c.description.isEmpty {
                    Text(c.description).font(.caption).foregroundStyle(Trek.muted).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .frame(minHeight: 48)
        .contentShape(Rectangle())
    }

    private func symbol(_ c: CommandInfo) -> String {
        switch c.kind {
        case .skill: "sparkles"
        case .agent: "person.2"
        case .command: c.trek == true ? "mountain.2" : "slash.circle"
        }
    }
}
