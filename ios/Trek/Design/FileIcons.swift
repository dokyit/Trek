import SwiftUI
import UIKit

/// Catppuccin's file and folder icons (https://github.com/catppuccin/zed-icons, MIT), as the Mac
/// draws them (`file_icon::icon_name`). `script/file-icons.py` imports them: `FileIcons.xcassets`
/// holds each as `fi-<icon>`, Latte for light and Mocha for dark, and `file-icons.json` says which
/// goes with what.
nonisolated enum FileIcons {
    private struct Map: Decodable {
        /// Whole file names (`Cargo.toml`, `README.md`).
        var names: [String: String] = [:]
        /// Suffixes after a dot (`rs`, `component.ts`), and some whole names (`Dockerfile`).
        var suffixes: [String: String] = [:]
        /// Folder names; `<icon>_open` is the open one.
        var folders: [String: String] = [:]
    }

    private static let map: Map = {
        guard let url = Bundle.main.url(forResource: "file-icons", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let map = try? JSONDecoder().decode(Map.self, from: data) else { return Map() }
        return map
    }()

    /// Looks `key` up as written, then in lower case.
    private static func find(_ table: [String: String], _ key: String) -> String? {
        table[key] ?? table[key.lowercased()]
    }

    private static func lastName(_ path: String) -> String {
        let trimmed = path.hasSuffix("/") ? String(path.dropLast()) : path
        return trimmed.split(separator: "/").last.map(String.init) ?? trimmed
    }

    /// The asset for a file at `path`: by its whole name, then its suffixes, longest first.
    static func file(_ path: String) -> String {
        let name = lastName(path)
        if let icon = find(map.names, name) ?? find(map.suffixes, name) { return "fi-\(icon)" }
        var rest = Substring(name)
        while let dot = rest.firstIndex(of: ".") {
            rest = rest[rest.index(after: dot)...]
            if let icon = find(map.suffixes, String(rest)) { return "fi-\(icon)" }
        }
        return "fi-_file"
    }

    /// The asset for the folder at `path` (`src`, `docs`, `.github`), open or closed.
    static func folder(_ path: String, open: Bool = false) -> String {
        let icon = find(map.folders, lastName(path)) ?? "_folder"
        return open ? "fi-\(icon)_open" : "fi-\(icon)"
    }

    /// A file's asset, or a folder's when `path` ends in "/".
    static func any(_ path: String) -> String {
        path.hasSuffix("/") ? folder(path) : file(path)
    }
}

/// A folder's icon (`file_icon::folder`).
struct FolderIcon: View {
    var path: String
    var open = false
    var size: CGFloat = 14

    var body: some View {
        Image(FileIcons.folder(path, open: open))
            .resizable()
            .interpolation(.high)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}
