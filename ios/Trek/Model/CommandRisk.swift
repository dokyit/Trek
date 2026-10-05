import Foundation

/// Spots approval requests that can destroy work: deleting files, rewriting remote history,
/// running as root, piping the internet into a shell. The approval card then gives Allow and Deny
/// equal weight and says why, so a quick thumb doesn't allow `rm -rf` the way it allows `cargo test`.
/// A heuristic for the eye, not a sandbox: anything not caught here still needs reading.
nonisolated enum CommandRisk {
    private static let rules: [(pattern: String, caseSensitive: Bool, reason: String)] = [
        (#"\brm\s+(-[a-z]*\s+)*-[a-z]*[rf]"#, false, "deletes files"),
        (#"\brm\s+(-[a-z]*\s+)*--(recursive|force)\b"#, false, "deletes files"),
        (#"\bfind\b.*\s-delete\b"#, false, "deletes files"),
        (#"\bgit\s+push\b.*(\s--force(-with-lease)?\b|\s-f\b|\s\+\S)"#, false, "rewrites history on the remote"),
        (#"\bgit\s+reset\s+--hard\b"#, false, "throws away uncommitted changes"),
        (#"\bgit\s+(checkout|restore)\s+(--\s+)?\.(\s|$)"#, false, "throws away uncommitted changes"),
        (#"\bgit\s+clean\s+-[a-z]*f"#, false, "deletes untracked files"),
        (#"\bgit\s+branch\s+(-D|--delete\s+--force)\b"#, true, "deletes a branch"),
        (#"(^|[\s;&|(])(sudo|doas)\s"#, false, "runs as root"),
        (#"\b(curl|wget)\b[^|;]*\|\s*(sudo\s+)?(ba|z|da|k|fi)?sh\b"#, false, "runs a script straight from the internet"),
        (#"\b(ba|z)?sh\s+(-c\s+)?["']?\$\((curl|wget)\b"#, false, "runs a script straight from the internet"),
        (#"\b(mkfs(\.\w+)?|diskutil\s+(erase\w*|partitionDisk))\b"#, false, "erases a disk"),
        (#"\bdd\s+.*\bof=/dev/"#, false, "writes straight to a disk"),
        (#"\bchmod\s+(-R\s+)?0?777\b"#, false, "opens up file permissions"),
        (#"\b(drop\s+(table|database|schema)|truncate\s+table)\b"#, false, "deletes database data"),
        (#"\b(terraform\s+destroy|kubectl\s+delete|docker\s+system\s+prune)\b"#, false, "destroys resources"),
    ]

    private static let compiled: [(NSRegularExpression, String)] = rules.compactMap { rule in
        let options: NSRegularExpression.Options = rule.caseSensitive ? [] : [.caseInsensitive]
        return (try? NSRegularExpression(pattern: rule.pattern, options: options)).map { ($0, rule.reason) }
    }

    /// Why the command looks destructive ("deletes files"), or nil.
    static func warning(for command: String) -> String? {
        let range = NSRange(command.startIndex..., in: command)
        var reasons: [String] = []
        for (regex, reason) in compiled where !reasons.contains(reason) {
            if regex.firstMatch(in: command, range: range) != nil { reasons.append(reason) }
        }
        guard !reasons.isEmpty else { return nil }
        return ListFormatter.localizedString(byJoining: Array(reasons.prefix(2)))
    }
}
