import Foundation

/// The last threads opened, kept in Caches so they show at once when opened again (the
/// subscription then asks only for what's newer). Small and bounded: the last `keepThreads`
/// threads, each its last `keepItems` items (and any requests still pending). Per Mac, so another
/// Mac's never show.
nonisolated enum TranscriptCache {
    static let keepThreads = 20
    static let keepItems = 80
    private static let version = 1

    struct Entry: Codable, Sendable {
        var v: Int
        var seq: Int64
        var more: Bool
        var items: [TItem]
    }

    private static func folder(host: String) -> URL? {
        guard let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first else { return nil }
        return caches.appending(path: "Transcripts", directoryHint: .isDirectory).appending(path: safe(host), directoryHint: .isDirectory)
    }

    private static func safe(_ s: String) -> String {
        String(s.map { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" ? $0 : "_" })
    }

    private static func file(host: String, thread: String) -> URL? {
        folder(host: host)?.appending(path: safe(thread) + ".json")
    }

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()

    /// The cached transcript, or nil (none, unreadable, or an older format).
    static func load(host: String, thread: String) -> Entry? {
        guard let url = file(host: host, thread: thread), let data = try? Data(contentsOf: url),
              let entry = try? decoder.decode(Entry.self, from: data), entry.v == version else { return nil }
        return entry
    }

    /// Writes one at a time, in order, off the main thread.
    private static let queue = DispatchQueue(label: "dev.trek.TrekMobile.transcript-cache", qos: .utility)

    /// Saves in the background; the oldest threads beyond `keepThreads` are dropped.
    static func save(host: String, thread: String, items: [TItem], seq: Int64, more: Bool) {
        queue.async { write(host: host, thread: thread, items: items, seq: seq, more: more) }
    }

    private static func write(host: String, thread: String, items: [TItem], seq: Int64, more: Bool) {
        guard let dir = folder(host: host), let url = file(host: host, thread: thread) else { return }
        var kept = items
        var more = more
        if items.count > keepItems {
            let cut = items.count - keepItems
            // Requests still waiting stay, wherever they are.
            kept = items.prefix(cut).filter(\.isPendingRequest) + items.suffix(keepItems)
            more = true
        }
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let data = try? encoder.encode(Entry(v: version, seq: seq, more: more, items: kept)) else { return }
        let fm = FileManager.default
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
        try? data.write(to: url, options: .atomic)
        // Bounded: the most recently saved threads.
        let keys: Set<URLResourceKey> = [.contentModificationDateKey]
        guard let files = try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: Array(keys)), files.count > keepThreads else { return }
        let dated = files.map { ($0, (try? $0.resourceValues(forKeys: keys).contentModificationDate) ?? .distantPast) }
        for (old, _) in dated.sorted(by: { $0.1 > $1.1 }).dropFirst(keepThreads) { try? fm.removeItem(at: old) }
    }

    /// Drops one thread's cache (after a reset that couldn't be applied, or the thread's gone).
    static func remove(host: String, thread: String) {
        queue.async { if let url = file(host: host, thread: thread) { try? FileManager.default.removeItem(at: url) } }
    }
}
