import Foundation

// Videos and files sent with a message (`uploads`, `trek_remote`'s `protocol/uploads.rs`): each
// goes up first, a piece at a time (`upload_part`, answered with `upload_progress`), and the
// message names them by upload id (`send.files`, `new_thread.files`). The Mac keeps each in its
// data folder and mentions it after the message's words, `@"/…/uploads/iPhone <date> <name>"`, as
// its own composer mentions a file; the bubble shows those as chips (`MessageFiles`). Only
// Foundation here, so the tests and a script can check it outside the app.

// MARK: Wire

/// The reply to `upload_part`: how much of the file the Mac has.
nonisolated struct UploadProgress: Decodable, Sendable, Equatable {
    var uploadId: String
    var received: Int64
    var total: Int64
    /// The whole file is in: its id may go with the message.
    var done: Bool?

    var finished: Bool { done ?? false }
}

nonisolated extension ClientMessage {
    /// The most one `upload_part` carries (`trek_remote::MAX_UPLOAD_PART`), before base64.
    static let uploadPiece = 4 << 20
    /// The biggest file the Mac takes (`trek_remote::MAX_UPLOAD`).
    static let maxUpload: Int64 = 2 << 30
}

// MARK: Pieces

/// Where each piece of a file starts and how long it is: one piece of `size` after another, the
/// last one short, and a single empty piece for an empty file (the Mac needs one to have it).
nonisolated enum UploadPieces {
    static func of(total: Int64, size: Int = ClientMessage.uploadPiece) -> [(offset: Int64, length: Int)] {
        guard total > 0 else { return [(0, 0)] }
        let size = Int64(max(1, size))
        return stride(from: Int64(0), to: total, by: Int(size)).map { ($0, Int(min(size, total - $0))) }
    }
}

/// Reads a file a piece at a time, so a long video never sits in memory whole.
nonisolated struct UploadReader: Sendable {
    let url: URL

    /// The file's length in bytes.
    func size() throws -> Int64 {
        let values = try url.resourceValues(forKeys: [.fileSizeKey])
        if let size = values.fileSize { return Int64(size) }
        return (try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.int64Value ?? 0
    }

    /// `length` bytes from `offset` (fewer at the end of the file).
    func read(offset: Int64, length: Int) throws -> Data {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        try handle.seek(toOffset: UInt64(offset))
        return try handle.read(upToCount: length) ?? Data()
    }

    /// The `upload_part` for the piece at `offset`, base64 and all (made off the main actor).
    func part(id: String, name: String, total: Int64, offset: Int64, length: Int) throws -> ClientMessage {
        let data = try read(offset: offset, length: length)
        guard data.count == length else { throw UploadError("\(name) changed while it was being sent") }
        return .uploadPart(uploadId: id, name: name, total: total, offset: offset, data: data.base64EncodedString())
    }
}

/// Why a file didn't go up, in words for a toast.
nonisolated struct UploadError: LocalizedError, Equatable {
    var message: String
    /// The Mac didn't take the message: a file it names may be gone from it, so send them again.
    var resend = false
    init(_ message: String, resend: Bool = false) { self.message = message; self.resend = resend }
    var errorDescription: String? { message }
}

// MARK: Showing them in a message

/// The files a message mentions at its end, as the Mac and its composer add them
/// (`trek_core::media::mention_of`): `@/path` or, with whitespace in it, `@"/path"`. Only absolute
/// (or `~/`) paths at the very end count, after the words; a mention in the middle of a sentence
/// stays part of it.
nonisolated enum MessageFiles {
    /// `text` without its trailing file mentions, and the paths they name, in order.
    static func split(_ text: String) -> (text: String, files: [String]) {
        guard text.contains("@") else { return (text, []) }
        var mentions: [(range: Range<String.Index>, path: String)] = []
        var i = text.startIndex
        while let at = text[i...].firstIndex(of: "@") {
            let wordStart = at == text.startIndex || text[text.index(before: at)].isWhitespace
            let rest = text.index(after: at)
            var end = rest
            var path = ""
            if wordStart, rest < text.endIndex, text[rest] == "\"" {
                let open = text.index(after: rest)
                guard let close = text[open...].firstIndex(of: "\"") else { break }
                path = String(text[open..<close])
                end = text.index(after: close)
            } else {
                end = text[rest...].firstIndex(where: \.isWhitespace) ?? text.endIndex
                path = String(text[rest..<end])
            }
            if wordStart, path.hasPrefix("/") || path.hasPrefix("~/") {
                mentions.append((at..<end, path))
            }
            i = end > at ? end : text.index(after: at)
            if i >= text.endIndex { break }
        }
        // The run of them at the end: nothing but whitespace after each.
        var cut = text.endIndex
        var files: [String] = []
        for m in mentions.reversed() {
            guard text[m.range.upperBound..<cut].allSatisfy(\.isWhitespace) else { break }
            files.insert(m.path, at: 0)
            cut = m.range.lowerBound
        }
        guard !files.isEmpty else { return (text, []) }
        let said = String(text[..<cut]).trimmingCharacters(in: .whitespacesAndNewlines)
        return (said, files)
    }

    /// The name to show for a file the Mac keeps: its own name, without the `iPhone <date> at
    /// <time> ` it was kept under (`uploads.rs`).
    static func displayName(_ path: String) -> String {
        let name = (path as NSString).lastPathComponent
        // `0` stands for a digit.
        let prefix = Array("iPhone 0000-00-00 at 00.00.00 ")
        let chars = Array(name)
        guard chars.count > prefix.count else { return name }
        for (c, p) in zip(chars, prefix) where p == "0" ? !(c.isASCII && c.isNumber) : c != p {
            return name
        }
        return String(chars[prefix.count...])
    }

    /// What goes with attachments sent with no words: the agent and the transcript read it.
    static func caption(photos: Int, videos: Int, files: Int) -> String {
        let total = photos + videos + files
        switch (photos, videos, files) {
        case (_, 0, 0): return total > 1 ? "Here are \(total) photos." : "Here's a photo."
        case (0, _, 0): return total > 1 ? "Here are \(total) videos." : "Here's a video."
        case (0, 0, _): return total > 1 ? "Here are \(total) files." : "Here's a file."
        default: return "Here are \(total) attachments."
        }
    }
}

/// "820 KB", "12.4 MB", "1.2 GB": a file's size on its chip.
nonisolated func fileSizeWords(_ bytes: Int64) -> String {
    ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
}
