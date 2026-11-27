import Foundation

/// What was typed and not sent, by thread: kept so that leaving a thread, or iOS ending the app
/// in the background, doesn't lose it. Keys are qualified thread ids (`"host:thread"`), so two
/// machines' threads of the same id keep their own; the new-thread sheet has a key of its own.
///
/// Bounded: the newest `keep` drafts, each cut to `maxLength` characters, none older than
/// `maxAge`. A plain value, so its rules are tested without a file or a view.
nonisolated struct DraftBook: Codable, Equatable {
    struct Entry: Codable, Equatable {
        var text: String
        /// When it was last changed (seconds since 1970).
        var at: Double
    }

    static let keep = 50
    static let maxLength = 20_000
    static let maxAge: TimeInterval = 30 * 24 * 3600
    /// The new-thread sheet's text: no thread has it yet.
    static let newThread = "~new-thread"

    private(set) var entries: [String: Entry] = [:]

    func text(_ key: String) -> String? { entries[key]?.text }

    var isEmpty: Bool { entries.isEmpty }

    /// Keeps `text` for `key`; nothing but whitespace is no draft. True when something changed.
    @discardableResult
    mutating func set(_ key: String, _ text: String, now: Date) -> Bool {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return clear(key) }
        let kept = text.count > Self.maxLength ? String(text.prefix(Self.maxLength)) : text
        guard entries[key]?.text != kept else { return false }
        entries[key] = Entry(text: kept, at: now.timeIntervalSince1970)
        // Over the count: the ones left longest go.
        if entries.count > Self.keep {
            let oldest = entries.sorted { $0.value.at < $1.value.at }.prefix(entries.count - Self.keep)
            for (k, _) in oldest where k != key { entries[k] = nil }
        }
        return true
    }

    @discardableResult
    mutating func clear(_ key: String) -> Bool { entries.removeValue(forKey: key) != nil }

    /// Drops what hasn't been touched for `maxAge`.
    @discardableResult
    mutating func prune(now: Date) -> Bool {
        let floor = now.timeIntervalSince1970 - Self.maxAge
        let before = entries.count
        entries = entries.filter { $0.value.at >= floor }
        return entries.count != before
    }

    /// Drops every draft for `host`'s threads (the machine was unpaired).
    @discardableResult
    mutating func forget(host: String) -> Bool {
        let before = entries.count
        entries = entries.filter { !$0.key.hasPrefix("\(host):") }
        return entries.count != before
    }

    @discardableResult
    mutating func removeAll() -> Bool {
        guard !entries.isEmpty else { return false }
        entries = [:]
        return true
    }
}

/// The drafts and the file that keeps them (Application Support, not Caches: iOS empties Caches
/// when the disk runs short, and a draft is the user's own words). Changes are written a moment
/// later, off the main thread; `flush` writes at once, for when the app leaves the front.
final class DraftStore {
    private(set) var book = DraftBook()
    private let url: URL?
    private let now: () -> Date
    private var dirty = false
    private var saveScheduled = false

    nonisolated static var defaultURL: URL? {
        guard let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return nil }
        return dir.appending(path: "drafts.json")
    }

    /// `url` nil keeps them in memory only (tests that don't look at the file).
    init(url: URL? = DraftStore.defaultURL, now: @escaping () -> Date = { Date() }) {
        self.url = url
        self.now = now
        if let url, let read = DraftFile.read(url) {
            book = read
            if book.prune(now: now()) { changed() }
        }
    }

    func text(_ key: String) -> String? { book.text(key) }

    func set(_ key: String, _ text: String) {
        if book.set(key, text, now: now()) { changed() }
    }

    func clear(_ key: String) {
        if book.clear(key) { changed() }
    }

    func forget(host: String) {
        if book.forget(host: host) { changed() }
    }

    func removeAll() {
        if book.removeAll() { changed() }
    }

    private func changed() {
        dirty = true
        guard !saveScheduled else { return }
        saveScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in
            self?.saveScheduled = false
            self?.write(wait: false)
        }
    }

    /// Writes now and waits for it: the app is leaving the front, and iOS may end it from here on.
    func flush() { write(wait: true) }

    private func write(wait: Bool) {
        guard dirty, let url else { return }
        dirty = false
        DraftFile.write(book, to: url, wait: wait)
    }
}

/// The drafts' file: written one at a time, in order, off the main thread.
nonisolated enum DraftFile {
    private static let queue = DispatchQueue(label: "dev.trek.TrekMobile.drafts", qos: .utility)

    static func read(_ url: URL) -> DraftBook? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(DraftBook.self, from: data)
    }

    /// `wait`: not back until it's on disk.
    static func write(_ book: DraftBook, to url: URL, wait: Bool) {
        if wait {
            queue.sync { save(book, to: url) }
        } else {
            queue.async { save(book, to: url) }
        }
    }

    /// The file as `book` has it; no drafts, no file.
    private static func save(_ book: DraftBook, to url: URL) {
        guard !book.isEmpty else {
            try? FileManager.default.removeItem(at: url)
            return
        }
        guard let data = try? JSONEncoder().encode(book) else { return }
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
}

/// What a send took out of a composer, to put back if the Mac doesn't take it.
struct HeldMessage {
    var text: String
    var photos: [PickedPhoto] = []
}

/// The words for a message that didn't go.
nonisolated enum SendFailure {
    static let backInDraft = "Your message is back in the thread's draft."

    /// `reason`: what the Mac (or the phone) said went wrong. `away`: the thread's screen isn't the
    /// one showing, so the composer the words went back into can't be seen: say where they are.
    static func toast(reason: String, code: ErrorCode, away: Bool) -> String {
        guard away else { return reason }
        // It may have gone (the connection dropped, no answer came): not "Not sent".
        if code.unsure { return reason.isEmpty ? backInDraft : "\(reason) \(backInDraft)" }
        return reason.isEmpty ? "Not sent. \(backInDraft)" : "Not sent. \(backInDraft) \(reason)"
    }
}
