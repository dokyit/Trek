import AVFoundation
import CoreTransferable
import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// A video or file picked to go with a message (`uploads`): a copy in the app's temporary folder
/// (what the picker hands over goes away, and a file from iCloud Drive or another app's provider
/// may not stay readable), sent to the Mac a piece at a time when the message goes.
struct PickedFile: Identifiable, Equatable {
    enum Kind: Equatable { case video, file }

    /// It's on a Mac already: which one (its link) and under what upload id.
    struct Sent: Equatable {
        var link: String
        var id: String
    }

    let id = UUID()
    /// The copy here.
    let url: URL
    let name: String
    let size: Int64
    let kind: Kind
    /// A video's length in seconds, and a frame of it.
    var duration: Double?
    var thumbnail: UIImage?
    var sent: Sent?
    /// How much of it is up (0…1) while it's being sent.
    var progress: Double?

    static func == (a: PickedFile, b: PickedFile) -> Bool {
        a.id == b.id && a.sent == b.sent && a.progress == b.progress && a.thumbnail === b.thumbnail
    }

    /// "0:42", "1:02:05": a video's length on its tile.
    static func clock(_ secs: Double) -> String {
        let s = Int(max(0, secs).rounded())
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }

    /// Takes the copy here away (it was sent, or taken off the message).
    func discard() {
        try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
    }

    /// Where the copies are kept.
    nonisolated static var copies: URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("Attachments", isDirectory: true)
    }

    /// Takes every copy away (at launch: no composer holds one then).
    nonisolated static func clearCopies() {
        try? FileManager.default.removeItem(at: copies)
    }

    /// Where a copy of `source` goes, as `name`: a folder of its own, so two files of one name
    /// both keep it.
    nonisolated static func copy(_ source: URL, named name: String) throws -> URL {
        let dir = copies.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let safe = name.replacingOccurrences(of: "/", with: "-")
        let dest = dir.appendingPathComponent(safe.isEmpty ? "file" : safe)
        // APFS clones it: a long video is copied in a moment, without the room it takes twice.
        try FileManager.default.copyItem(at: source, to: dest)
        return dest
    }

    // MARK: Videos from the photo library

    /// A video the photo picker hands over as a file.
    struct Movie: Transferable {
        let url: URL

        static var transferRepresentation: some TransferRepresentation {
            // The kinds the library keeps first: asked for as `public.movie` alone, it can hand
            // back nothing for a clip it has as an MPEG-4.
            FileRepresentation(importedContentType: .quickTimeMovie, importing: imported)
            FileRepresentation(importedContentType: .mpeg4Movie, importing: imported)
            FileRepresentation(importedContentType: .movie, importing: imported)
        }

        /// The picker's file is gone once this returns: copied first.
        nonisolated private static func imported(_ received: ReceivedTransferredFile) throws -> Movie {
            Movie(url: try PickedFile.copy(received.file, named: received.file.lastPathComponent))
        }
    }

    /// Whether the picker's item is a video (else it's a photo).
    static func isVideo(_ item: PhotosPickerItem) -> Bool {
        item.supportedContentTypes.contains { $0.conforms(to: .movie) || $0.conforms(to: .video) }
    }

    /// The videos among what the photo picker handed back, copied here, each with its length and
    /// a frame of it.
    static func loadVideos(_ items: [PhotosPickerItem]) async -> [PickedFile] {
        var out: [PickedFile] = []
        for item in items where isVideo(item) {
            let movie: Movie
            do {
                guard let loaded = try await item.loadTransferable(type: Movie.self) else {
                    protocolLog.error("A video from the library came as nothing (\(item.supportedContentTypes.map(\.identifier), privacy: .public))")
                    continue
                }
                movie = loaded
            } catch {
                protocolLog.error("A video from the library didn't load: \(String(describing: error), privacy: .public)")
                continue
            }
            let size = (try? UploadReader(url: movie.url).size()) ?? 0
            let (duration, frame) = await look(at: movie.url)
            out.append(PickedFile(url: movie.url, name: movie.url.lastPathComponent, size: size, kind: .video,
                                  duration: duration, thumbnail: frame))
        }
        return out
    }

    /// A video's length, and a frame from its start a tile's size.
    private static func look(at url: URL) async -> (Double?, UIImage?) {
        let asset = AVURLAsset(url: url)
        let duration = try? await asset.load(.duration).seconds
        let frames = AVAssetImageGenerator(asset: asset)
        frames.appliesPreferredTrackTransform = true
        frames.maximumSize = CGSize(width: 240, height: 240)
        let frame = try? await frames.image(at: CMTime(seconds: 0.1, preferredTimescale: 600)).image
        return (duration.flatMap { $0.isFinite ? $0 : nil }, frame.map(UIImage.init(cgImage:)))
    }

    // MARK: Files from the Files app

    /// What `.fileImporter` handed back, copied here (off the main actor, each read with the
    /// access its provider grants, coordinated so an iCloud file that isn't downloaded yet is). A
    /// folder, or one too big for the Mac, comes back as a reason instead.
    @concurrent nonisolated static func importFiles(_ urls: [URL]) async -> [Result<(url: URL, name: String, size: Int64), UploadError>] {
        urls.map { url in
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            let name = url.lastPathComponent
            if (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true {
                return .failure(UploadError("\(name) is a folder: send the files in it, or a zip of it"))
            }
            var copied: Result<(url: URL, name: String, size: Int64), UploadError> = .failure(UploadError("Couldn't read \(name)"))
            var problem: NSError?
            NSFileCoordinator().coordinate(readingItemAt: url, options: [.withoutChanges], error: &problem) { readable in
                do {
                    let size = try UploadReader(url: readable).size()
                    guard size <= ClientMessage.maxUpload else {
                        copied = .failure(UploadError("\(name) is \(fileSizeWords(size)): the most the Mac takes is \(fileSizeWords(ClientMessage.maxUpload))"))
                        return
                    }
                    copied = .success((try copy(readable, named: name), name, size))
                } catch {
                    copied = .failure(UploadError("Couldn't read \(name): \(error.localizedDescription)"))
                }
            }
            if let problem { return .failure(UploadError("Couldn't read \(name): \(problem.localizedDescription)")) }
            return copied
        }
    }

    /// Files from the Files app, ready to go: what couldn't be is said in a toast.
    static func load(files urls: [URL], model: AppModel?) async -> [PickedFile] {
        var out: [PickedFile] = []
        for result in await importFiles(urls) {
            switch result {
            case .success(let f): out.append(PickedFile(url: f.url, name: f.name, size: f.size, kind: UTType(filenameExtension: (f.name as NSString).pathExtension)?.conforms(to: .movie) == true ? .video : .file))
            case .failure(let why): model?.show(why.message, error: true)
            }
        }
        // A video from Files gets its frame and length too.
        for i in out.indices where out[i].kind == .video {
            (out[i].duration, out[i].thumbnail) = await look(at: out[i].url)
        }
        return out
    }
}

// MARK: Sending them

extension AppModel {
    /// Sends each of `files` that isn't on `link`'s machine yet, one after another and a piece at a
    /// time; `change` hears how far each has got and its upload id once it's in. Their upload ids,
    /// in order. Throws the first failure (the Mac's words, or the connection's).
    func upload(_ files: [PickedFile], on link: HostLink, change: @escaping (PickedFile.ID, (inout PickedFile) -> Void) -> Void) async throws -> [String] {
        var ids: [String] = []
        for file in files {
            if let sent = file.sent, sent.link == link.id {
                ids.append(sent.id)
                continue
            }
            change(file.id) { $0.progress = 0 }
            do {
                let id = try await upload(file, on: link) { fraction in change(file.id) { $0.progress = fraction } }
                change(file.id) { $0.progress = nil; $0.sent = PickedFile.Sent(link: link.id, id: id) }
                ids.append(id)
            } catch {
                change(file.id) { $0.progress = nil }
                throw error
            }
        }
        return ids
    }

    /// One file, a piece at a time: each piece waits for the Mac to have the one before (it says
    /// how far the file got), so a refusal stops it there.
    private func upload(_ file: PickedFile, on link: HostLink, progress: @escaping (Double) -> Void) async throws -> String {
        let id = UUID().uuidString
        let reader = UploadReader(url: file.url)
        let total = try reader.size()
        guard total <= ClientMessage.maxUpload else {
            throw UploadError("\(file.name) is \(fileSizeWords(total)): the most the Mac takes is \(fileSizeWords(ClientMessage.maxUpload))")
        }
        for piece in UploadPieces.of(total: total) {
            try Task.checkCancellation()
            let part = try await Self.part(reader, id: id, name: file.name, total: total, offset: piece.offset, length: piece.length)
            let reply = await withCheckedContinuation { (c: CheckedContinuation<ServerMessage, Never>) in
                request(part, on: link) { c.resume(returning: $0) }
            }
            switch reply {
            case .uploadProgress(_, let p):
                progress(Double(p.received) / Double(max(1, p.total)))
                if p.finished { return id }
            case .error(_, _, let message):
                throw UploadError(message)
            default:
                throw UploadError("The Mac answered something else while \(file.name) was going up")
            }
        }
        throw UploadError("The Mac didn't say it had all of \(file.name)")
    }

    /// Read and made into base64 off the main actor.
    @concurrent nonisolated private static func part(_ reader: UploadReader, id: String, name: String, total: Int64, offset: Int64,
                                                     length: Int) async throws -> ClientMessage {
        try reader.part(id: id, name: name, total: total, offset: offset, length: length)
    }
}
