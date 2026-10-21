import Foundation
import Observation

// MARK: Wire

/// The reply to `note_file`: `bytes_b64` from `offset` of a file kept with the Mac's notes, and
/// how long the whole file is.
nonisolated struct NoteFileChunk: Decodable, Sendable {
    var path: String
    var offset: Int64
    var total: Int64
    var bytesB64: String
    var mime: String
}

nonisolated extension ClientMessage {
    /// The most one `note_file` piece carries (`trek_remote::MAX_NOTE_FILE_CHUNK`).
    static let noteFileChunk = 1 << 20
}

// MARK: Fetching

/// Notes' files coming from the Mac into `NoteFileStore`: one download per link at a time
/// (a second card showing the same file waits on the first), with how far each has got.
@Observable
final class NoteFiles {
    static let shared = NoteFiles()

    /// What came of fetching a file.
    enum Outcome: Equatable {
        case file(URL)
        /// Bigger than the most that's fetched without asking (`total` bytes): fetch it on a tap.
        case large(Int64)
        /// The Mac couldn't send it (its message), or the connection went.
        case failed(String)
        /// Not here, and the Mac doesn't send notes' files (an older one, or none connected).
        case onMac
    }

    /// Links coming in now: how much of each is here (0…1).
    private(set) var progress: [String: Double] = [:]
    /// The length of files the Mac said, by link (for a card to show before they're fetched).
    private(set) var sizes: [String: Int64] = [:]
    @ObservationIgnored private var running: [String: Task<Outcome, Never>] = [:]
    @ObservationIgnored var store = NoteFileStore.shared

    /// Fetched without a tap (a PDF's first page, a video's poster) up to this size.
    static let autoLimit: Int64 = 24 << 20

    /// `path` under `link`'s name: two machines' `attachments/x.pdf` stay two files.
    func scoped(_ path: String, on link: HostLink?) -> String {
        "\(link?.id ?? "")/\(NoteFileStore.normalized(path))"
    }

    /// The file, if it's here already: no Mac needed.
    func cached(_ path: String, on link: HostLink? = nil) -> URL? { store.cached(scoped(path, on: link)) }

    /// The file `path` (`attachments/…`): from the cache, else from `link`'s Mac a piece at a
    /// time (the selected one's without `link`). With `upTo`, a file the Mac says is bigger
    /// stops after its first piece (kept, so a tap carries on from there).
    func fetch(_ path: String, model: AppModel, on link: HostLink? = nil, upTo limit: Int64? = nil) async -> Outcome {
        let link = link ?? model.selected
        let key = scoped(path, on: link)
        if let url = store.cached(key) { return .file(url) }
        if let task = running[key] {
            let got = await task.value
            // A fetch that stopped at the size limit doesn't answer one that has none.
            if case .large = got, limit == nil { return await fetch(path, model: model, on: link, upTo: nil) }
            return got
        }
        guard link?.serves(.noteFiles) == true else { return .onMac }
        let task = Task<Outcome, Never> {
            let got = await self.download(key, wire: NoteFileStore.normalized(path), model: model, on: link, limit: limit)
            // Done before anyone waiting on it hears, so a fetch after it starts afresh.
            self.running[key] = nil
            self.progress[key] = nil
            return got
        }
        running[key] = task
        return await task.value
    }

    /// `path` is already host-scoped (`scoped`); `wire` is the link the Mac knows.
    private func download(_ path: String, wire: String, model: AppModel, on link: HostLink?, limit: Int64?) async -> Outcome {
        var parts: ChunkAssembler
        do { parts = try store.assembler(for: path) } catch { return .failed(error.localizedDescription) }
        var restarted = false
        while true {
            let reply = await piece(wire, offset: parts.next, model: model, on: link)
            let chunk: NoteFileChunk
            switch reply {
            case .success(let c): chunk = c
            case .failure(let e) where e.code == .badRequest && parts.next > 0 && !restarted:
                // What a stopped download left is longer than the file is now: start again, once.
                restarted = true
                parts.restart()
                continue
            case .failure(let e): return .failed(e.message)
            }
            sizes[path] = chunk.total
            do {
                let (next, whole) = try await Self.write(chunk, into: parts)
                parts = next
                if whole {
                    store.evict(keeping: parts.final)
                    return .file(parts.final)
                }
            } catch let failure as ChunkAssembler.Failure where !restarted && failure != .overrun && failure != .stalled {
                // The file changed on the Mac since part of it came (or what's here doesn't fit
                // what the Mac sends): all of it again, once.
                restarted = true
                parts.restart()
                continue
            } catch {
                return .failed("That file didn't come through whole")
            }
            progress[path] = parts.fraction
            if let limit, chunk.total > limit { return .large(chunk.total) }
        }
    }

    /// One piece, from `offset`.
    private func piece(_ path: String, offset: Int64, model: AppModel, on link: HostLink?) async -> Result<NoteFileChunk, PieceError> {
        await withCheckedContinuation { c in
            model.request(.noteFile(path: path, offset: offset, length: ClientMessage.noteFileChunk), on: link) { reply in
                switch reply {
                case .noteFileChunk(_, let chunk): c.resume(returning: .success(chunk))
                case .error(_, let code, let message): c.resume(returning: .failure(PieceError(code: code, message: message)))
                default: c.resume(returning: .failure(PieceError(code: .unknown, message: "The Mac sent something else")))
                }
            }
        }
    }

    private struct PieceError: Error {
        var code: ErrorCode
        var message: String
    }

    /// Decoded and written off the main actor.
    @concurrent nonisolated private static func write(_ chunk: NoteFileChunk, into parts: ChunkAssembler) async throws -> (ChunkAssembler, Bool) {
        guard let data = Data(base64Encoded: chunk.bytesB64) else { throw ChunkAssembler.Failure.stalled }
        var parts = parts
        let whole = try parts.append(offset: chunk.offset, total: chunk.total, data: data)
        return (parts, whole)
    }
}
