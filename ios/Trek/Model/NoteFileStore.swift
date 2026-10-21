import CryptoKit
import Foundation

// Notes' files (PDFs, videos, anything kept in the Mac's notes `attachments/`), kept on the phone
// once fetched (`note_file`, a mebibyte at a time): one file per link, named by the SHA-256 of
// its path, in the app's caches. The same link in another note is the same file, a cached one
// opens without the Mac, and the folder is trimmed to `limit` (least recently opened first).
// Only Foundation here, so a script can check it outside the app.

/// The disk cache of notes' files.
nonisolated struct NoteFileStore: Sendable {
    let dir: URL
    /// Bytes kept at most; the least recently used files go first past it.
    let limit: Int64

    static let defaultLimit: Int64 = 200 << 20

    static let shared = NoteFileStore(
        dir: FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("NoteFiles", isDirectory: true),
        limit: defaultLimit)

    /// A note's link as the Mac serves it: `attachments/<name>` (a leading `./` taken off).
    static func normalized(_ path: String) -> String {
        path.hasPrefix("./") ? String(path.dropFirst(2)) : path
    }

    /// The SHA-256 of the link, in hex: the file's name here.
    static func key(_ path: String) -> String {
        SHA256.hash(data: Data(normalized(path).utf8)).map { String(format: "%02x", $0) }.joined()
    }

    /// Where the whole file goes (its extension kept, for QuickLook and AVFoundation).
    func url(for path: String) -> URL {
        let ext = (Self.normalized(path) as NSString).pathExtension.lowercased()
        let name = ext.isEmpty ? Self.key(path) : "\(Self.key(path)).\(ext)"
        return dir.appendingPathComponent(name)
    }

    /// Where it's written while it comes in; a download that stopped carries on from there.
    func partURL(for path: String) -> URL {
        url(for: path).appendingPathExtension("part")
    }

    /// The file, if it's here whole; it counts as used just now.
    func cached(_ path: String) -> URL? {
        let url = url(for: path)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        try? FileManager.default.setAttributes([.modificationDate: Date()], ofItemAtPath: url.path)
        return url
    }

    /// Whether it's here whole, without counting as used.
    func has(_ path: String) -> Bool {
        FileManager.default.fileExists(atPath: url(for: path).path)
    }

    /// Something to put the next piece of `path` in: carrying on from what came before.
    func assembler(for path: String) throws -> ChunkAssembler {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return ChunkAssembler(part: partURL(for: path), final: url(for: path))
    }

    /// Bytes the folder holds now.
    func size() -> Int64 {
        entries().reduce(0) { $0 + $1.size }
    }

    /// Takes the least recently used files out until the folder is within `limit`; `keeping` (the
    /// one just fetched, or being shown) stays even if it alone is bigger. Returns what went.
    @discardableResult
    func evict(keeping: URL? = nil) -> [URL] {
        var all = entries()
        var total = all.reduce(0) { $0 + $1.size }
        guard total > limit else { return [] }
        all.sort { $0.used < $1.used }
        var gone: [URL] = []
        // A download still coming in (written to in the last few minutes) isn't pulled from under it.
        let busy = Date().addingTimeInterval(-600)
        for e in all where total > limit {
            if let keeping, e.url.standardizedFileURL == keeping.standardizedFileURL { continue }
            if e.url.pathExtension == "part", e.used > busy { continue }
            if (try? FileManager.default.removeItem(at: e.url)) != nil {
                total -= e.size
                gone.append(e.url)
            }
        }
        return gone
    }

    private struct Entry {
        var url: URL
        var size: Int64
        var used: Date
    }

    private func entries() -> [Entry] {
        let keys: [URLResourceKey] = [.fileSizeKey, .contentModificationDateKey, .isRegularFileKey]
        let urls = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: keys)) ?? []
        return urls.compactMap { url in
            guard let v = try? url.resourceValues(forKeys: Set(keys)), v.isRegularFile == true else { return nil }
            return Entry(url: url, size: Int64(v.fileSize ?? 0), used: v.contentModificationDate ?? .distantPast)
        }
    }
}

/// A file put together from the Mac's pieces, in order, into a `.part` file that becomes the
/// whole file once the last byte is in.
nonisolated struct ChunkAssembler: Sendable {
    enum Failure: Error, Equatable {
        /// A piece that doesn't start where the last one ended.
        case outOfOrder(expected: Int64, got: Int64)
        /// The file's length changed between pieces (it changed on the Mac): start again.
        case changed
        /// More bytes than the file has.
        case overrun
        /// An empty piece before the end: the Mac has nothing more to send.
        case stalled
    }

    let part: URL
    let final: URL
    /// Bytes in so far (what a stopped download left counts).
    private(set) var received: Int64
    /// The whole file's length, once a piece said.
    private(set) var total: Int64?

    init(part: URL, final: URL) {
        self.part = part
        self.final = final
        let had = (try? FileManager.default.attributesOfItem(atPath: part.path)[.size] as? NSNumber)?.int64Value
        received = had ?? 0
    }

    /// Where the next piece starts.
    var next: Int64 { received }

    var fraction: Double {
        guard let total, total > 0 else { return 0 }
        return min(1, Double(received) / Double(total))
    }

    var done: Bool { total.map { received >= $0 } ?? false }

    /// Starts again from nothing (the file changed on the Mac).
    mutating func restart() {
        try? FileManager.default.removeItem(at: part)
        received = 0
        total = nil
    }

    /// Adds the piece at `offset` of a file `total` bytes long; true once the file is whole (then
    /// it's at `final`).
    @discardableResult
    mutating func append(offset: Int64, total: Int64, data: Data) throws -> Bool {
        if let known = self.total, known != total { throw Failure.changed }
        guard offset == received else { throw Failure.outOfOrder(expected: received, got: offset) }
        guard received + Int64(data.count) <= total else { throw Failure.overrun }
        if data.isEmpty, received < total { throw Failure.stalled }
        self.total = total
        let fm = FileManager.default
        if !fm.fileExists(atPath: part.path) { fm.createFile(atPath: part.path, contents: nil) }
        if !data.isEmpty {
            let handle = try FileHandle(forWritingTo: part)
            defer { try? handle.close() }
            try handle.seek(toOffset: UInt64(received))
            try handle.write(contentsOf: data)
        }
        received += Int64(data.count)
        guard received == total else { return false }
        try? fm.removeItem(at: final)
        try fm.moveItem(at: part, to: final)
        return true
    }
}
