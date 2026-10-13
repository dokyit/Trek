import Foundation
import SwiftUI
import UIKit

// What the phone gained to keep up with the Mac (crates/trek-remote/src/protocol/parity.rs):
// Codex's granted resets, the follow-ups queued while a turn runs, a message's images, what an
// agent runs in the background, and added agents' and projects' icons. A Mac that answers these
// lists them in `welcome`'s `features` (`MacFeature`); on one that doesn't, the phone leaves them out.

/// A rate-limit reset Codex granted and the user hasn't spent.
nonisolated struct ResetCredit: Codable, Hashable, Identifiable {
    var id: String
    /// "Full reset (Weekly + 5 hr)".
    var title: String
    var description: String? = nil
    var expiresAt: Int64? = nil
}

/// A follow-up waiting to go: after the running turn, or (`reset`) once a usage limit resets.
nonisolated struct QueuedMessage: Codable, Hashable {
    var text: String
    var images: Int? = nil
    var reset: Bool? = nil
}

nonisolated enum BackgroundKind: String, LenientEnum {
    case shell, monitor, other
    static var fallback: BackgroundKind { .other }

    var symbol: String {
        switch self {
        case .shell: "apple.terminal"
        case .monitor: "dot.radiowaves.left.and.right"
        case .other: "waveform.path.ecg"
        }
    }

    /// As the Mac's strip says it.
    var label: String {
        switch self {
        case .shell: "Command"
        case .monitor: "Watching"
        case .other: "Task"
        }
    }
}

/// Something a thread's agent runs in the background: a dev server, a watcher.
nonisolated struct BackgroundTask: Codable, Hashable, Identifiable {
    var id: String
    var kind: BackgroundKind
    var title: String
    /// The last line it printed, as last read.
    var line: String? = nil
    /// When Trek first heard of it (ms).
    var since: Int64? = nil
    var readable: Bool? = nil
    var stoppable: Bool? = nil
    var stopping: Bool? = nil
}

/// The reply to `attachment`: one image of a message, as a JPEG.
nonisolated struct Attachment: Decodable {
    var threadId: String
    var itemId: String
    var index: Int
    var mime: String
    var data: String
    var width: Int
    var height: Int
}

/// The reply to `background_output`: the end of a task's output.
nonisolated struct BackgroundOutput: Decodable {
    var threadId: String
    var taskId: String
    var output: String? = nil
}

// MARK: Links between threads

/// How a link row's thread is related to the thread whose transcript it's in (the Mac's
/// `lineage::Link`).
nonisolated enum ThreadLinkKind: String, LenientEnum {
    case started
    case continuedIn = "continued_in"
    case startedBy = "started_by"
    case continuedFrom = "continued_from"
    static var fallback: ThreadLinkKind { .startedBy }

    /// What the row says before the other thread's title, as on the Mac.
    var label: String {
        switch self {
        case .started: "Started thread"
        case .continuedIn: "Continued in"
        case .startedBy: "Started by"
        case .continuedFrom: "Continued from"
        }
    }

    /// The row stands for this thread's agent's own call (`start_thread`, `handoff`): it says
    /// how that thread is doing. The other two say where this one came from.
    var isOutgoing: Bool { self == .started || self == .continuedIn }

    var symbol: String {
        switch self {
        case .started: "plus.bubble"
        case .startedBy: "arrow.turn.down.right"
        case .continuedIn, .continuedFrom: "arrow.left.arrow.right"
        }
    }
}

/// Where a thread an agent started stands, as the Mac's row says it.
nonisolated enum LinkedThreadState: String, LenientEnum {
    case working
    case needsApproval = "needs_approval"
    case paused, finished, failed
    static var fallback: LinkedThreadState { .finished }

    /// As its thread's row says it now.
    init(_ t: ThreadSummary) {
        if let needs = t.needs {
            switch needs.kind {
            case .failed: self = .failed
            case .limit: self = .paused
            default: self = .needsApproval
            }
        } else {
            switch t.runState {
            case .working: self = .working
            case .needsYou: self = .needsApproval
            case .failed: self = .failed
            case .idle: self = .finished
            }
        }
    }

    var label: String {
        switch self {
        case .working: "Working"
        case .needsApproval: "Needs your approval"
        case .paused: "Paused at a usage limit"
        case .finished: "Finished"
        case .failed: "Failed"
        }
    }
}

extension LinkedThreadState {
    var color: Color {
        switch self {
        case .working: Trek.working
        case .needsApproval, .paused: Trek.approval
        case .finished: Trek.done
        case .failed: Trek.failed
        }
    }
}

/// A transcript row that opens another thread.
nonisolated struct ThreadLink: Hashable {
    var link: ThreadLinkKind
    var threadId: String
    /// Its title when the Mac sent the row (its own row's is fresher).
    var title: String
    var agent: AgentRef? = nil
    /// For a thread this one started or handed over to: how it was doing then.
    var state: LinkedThreadState? = nil
    /// Still in Trek (not archived or deleted): the row opens it.
    var open: Bool = true
}

// MARK: Notes' pictures

/// The reply to `note_asset`: one of a note's pictures, as a JPEG.
nonisolated struct NoteAsset: Decodable {
    var noteId: String
    var path: String
    var mime: String
    var data: String
    var width: Int
    var height: Int
}

/// The reply to `add_note_image`: where the Mac keeps the photo, and the line that shows it.
nonisolated struct NoteImage: Decodable {
    var noteId: String
    /// `attachments/<name>`.
    var path: String
    /// `![name](attachments/<name>)`.
    var markdown: String
}

/// The reply to `icon`: a 96 px PNG.
nonisolated struct IconReply: Decodable {
    var agent: String? = nil
    var projectId: String? = nil
    var hash: String
    var png: String
    /// A shape to draw in the phone's colour, rather than a picture.
    var template: Bool? = nil
}

nonisolated extension ClientMessage {
    /// The JSON of the requests above.
    var parityJSON: [String: Any] {
        switch self {
        case .useReset(let creditId):
            return ["type": "use_reset", "credit_id": creditId]
        case .queueAction(let threadId, let index, let reset, let text, let edit):
            var o: [String: Any] = ["type": "queue_action", "thread_id": threadId, "index": index, "text": text,
                                    "action": edit.map { ["kind": "edit", "text": $0] } ?? ["kind": "remove"]]
            if reset { o["reset"] = true }
            return o
        case .attachment(let threadId, let itemId, let index, let maxPx):
            var o: [String: Any] = ["type": "attachment", "thread_id": threadId, "item_id": itemId, "index": index]
            if let maxPx { o["max_px"] = maxPx }
            return o
        case .backgroundOutput(let threadId, let taskId):
            return ["type": "background_output", "thread_id": threadId, "task_id": taskId]
        case .stopBackground(let threadId, let taskId):
            return ["type": "stop_background", "thread_id": threadId, "task_id": taskId]
        case .icon(let agent, let projectId):
            var o: [String: Any] = ["type": "icon"]
            if let agent { o["agent"] = agent }
            if let projectId { o["project_id"] = projectId }
            return o
        case .noteAsset(let noteId, let path, let maxPx):
            var o: [String: Any] = ["type": "note_asset", "note_id": noteId, "path": path]
            if let maxPx { o["max_px"] = maxPx }
            return o
        case .addNoteImage(let noteId, let data, let name):
            var o: [String: Any] = ["type": "add_note_image", "note_id": noteId, "data": data]
            if let name { o["name"] = name }
            return o
        default:
            return [:]
        }
    }
}

// MARK: Requests

extension AppModel {
    /// Spend one of Codex's granted resets, then read usage again.
    func useReset(_ credit: ResetCredit, done: @escaping (Bool) -> Void) {
        request(.useReset(creditId: credit.id)) { [weak self] reply in
            guard let self else { return }
            if case .error(_, _, let text) = reply {
                self.show(text, error: true)
                done(false)
            } else {
                self.show("Usage limits reset")
                done(true)
                self.loadUsage()
            }
        }
    }

    /// Drop a queued follow-up (`edit` nil; its text comes back for the composer) or change it.
    func queueAction(_ tid: String, index: Int, _ q: QueuedMessage, edit: String? = nil, taken: ((String) -> Void)? = nil) {
        request(.queueAction(threadId: tid, index: index, reset: q.reset ?? false, text: q.text, edit: edit)) { [weak self] reply in
            switch reply {
            case .error(_, _, let text): self?.show(text, error: true)
            case .ack(_, _, _, let text):
                if let text { taken?(text) } else if edit != nil { self?.show("Changed") }
            default: break
            }
        }
    }

    /// Image `index` of a message, fitting `maxPx` a side; kept a while in memory.
    func attachment(_ tid: String, item: String, index: Int, maxPx: Int) async -> UIImage? {
        let key = "\(tid)/\(item)/\(index)@\(maxPx)" as NSString
        if let image = Attachments.cache.object(forKey: key) { return image }
        let data: Data? = await withCheckedContinuation { c in
            request(.attachment(threadId: tid, itemId: item, index: index, maxPx: maxPx)) { reply in
                if case .attachment(_, let a) = reply { c.resume(returning: Data(base64Encoded: a.data)) } else { c.resume(returning: nil) }
            }
        }
        guard let data, let image = await Attachments.decode(data) else { return nil }
        Attachments.cache.setObject(image, forKey: key, cost: data.count)
        return image
    }

    /// A note's picture (`attachments/…`), fitting `maxPx` a side; kept a while in memory.
    func noteAsset(_ noteID: String, path: String, maxPx: Int) async -> UIImage? {
        let key = "note:\(noteID)/\(path)@\(maxPx)" as NSString
        if let image = Attachments.cache.object(forKey: key) ?? Attachments.cache.object(forKey: "note:\(noteID)/\(path)@full" as NSString) {
            return image
        }
        let data: Data? = await withCheckedContinuation { c in
            request(.noteAsset(noteId: noteID, path: path, maxPx: maxPx)) { reply in
                if case .noteAsset(_, let a) = reply { c.resume(returning: Data(base64Encoded: a.data)) } else { c.resume(returning: nil) }
            }
        }
        guard let data, let image = await Attachments.decode(data) else { return nil }
        Attachments.cache.setObject(image, forKey: key, cost: data.count)
        return image
    }

    /// Keep `photo` with the Mac's notes, for note `noteID`; `done` gets the line that shows it
    /// (nil when it couldn't be kept, which a toast says).
    func addNoteImage(_ noteID: String, photo: PickedPhoto, name: String? = nil, done: @escaping (NoteImage?) -> Void) {
        guard let upload = photo.upload else {
            show("That photo couldn't be read", error: true)
            return done(nil)
        }
        request(.addNoteImage(noteId: noteID, data: upload.data, name: name)) { [weak self] reply in
            switch reply {
            case .noteImage(_, let image):
                // What the phone just sent: drawn at once, without asking for it back.
                Attachments.cache.setObject(photo.image, forKey: "note:\(noteID)/\(image.path)@full" as NSString)
                done(image)
            case .error(_, _, let text):
                self?.show(text, error: true)
                done(nil)
            default: done(nil)
            }
        }
    }

    /// The end of a background task's output; nil when it couldn't be read.
    func backgroundOutput(_ tid: String, task: String) async -> BackgroundOutput? {
        await withCheckedContinuation { c in
            request(.backgroundOutput(threadId: tid, taskId: task)) { reply in
                if case .backgroundOutput(_, let out) = reply { c.resume(returning: out) } else { c.resume(returning: nil) }
            }
        }
    }

    func stopBackground(_ tid: String, task: String) {
        request(.stopBackground(threadId: tid, taskId: task)) { [weak self] reply in
            if case .error(_, _, let text) = reply { self?.show(text, error: true) }
        }
    }

    /// Learn which agents have icons of their own, and let the icon store ask the Mac.
    func learnIcons() {
        let store = IconStore.shared
        if store.fetch == nil {
            store.fetch = { [weak self] agent, project, done in
                guard let self else { return done(nil) }
                self.request(.icon(agent: agent, projectId: project)) { reply in
                    if case .icon(_, let icon) = reply { done(icon) } else { done(nil) }
                }
            }
        }
        var byAgent: [String: String] = [:]
        for a in agents { if let icon = a.icon { byAgent[a.key] = icon } }
        for t in threads { if let icon = t.agent.icon { byAgent[t.agent.key] = icon } }
        if byAgent != store.agentIcons { store.agentIcons = byAgent }
    }
}

/// Images of messages, scaled and kept in memory.
enum Attachments {
    static let cache: NSCache<NSString, UIImage> = {
        let c = NSCache<NSString, UIImage>()
        c.totalCostLimit = 48 << 20
        return c
    }()

    /// Decoded off the main actor, ready to draw.
    @concurrent nonisolated static func decode(_ data: Data) async -> UIImage? {
        await UIImage(data: data)?.byPreparingForDisplay()
    }
}

// MARK: Icons

/// Added agents' and projects' icons, as the Mac draws them: fetched once, kept on disk by hash.
@Observable
final class IconStore {
    static let shared = IconStore()

    struct Icon {
        var image: UIImage
        /// A shape to tint.
        var template: Bool
    }

    /// Agents with an icon of their own, by key: their icon's hash.
    var agentIcons: [String: String] = [:]
    @ObservationIgnored private var images: [String: Icon] = [:]
    @ObservationIgnored private var loading: [String: Task<Icon?, Never>] = [:]
    @ObservationIgnored var fetch: ((_ agent: String?, _ project: String?, _ done: @escaping (IconReply?) -> Void) -> Void)?

    private static let folder: URL = {
        let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("icons", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }()

    /// What's in memory already, to draw at once.
    func cached(_ hash: String) -> Icon? { images[hash] }

    /// The icon `hash`: from memory, from disk, else from the Mac (for `agent` or `project`).
    func icon(_ hash: String, agent: String? = nil, project: String? = nil) async -> Icon? {
        if let icon = images[hash] { return icon }
        if let task = loading[hash] { return await task.value }
        let task = Task<Icon?, Never> { [weak self] in
            let files = (Self.folder.appendingPathComponent("\(hash).png"), Self.folder.appendingPathComponent("\(hash)-t.png"))
            for (file, template) in [(files.0, false), (files.1, true)] {
                if let data = try? Data(contentsOf: file), let image = UIImage(data: data) { return Icon(image: image, template: template) }
            }
            guard let fetch = self?.fetch else { return nil }
            let reply: IconReply? = await withCheckedContinuation { c in fetch(agent, project) { c.resume(returning: $0) } }
            guard let reply, let data = Data(base64Encoded: reply.png), let image = UIImage(data: data, scale: 3) else { return nil }
            let template = reply.template ?? false
            try? data.write(to: template ? files.1 : files.0)
            return Icon(image: image, template: template)
        }
        loading[hash] = task
        let icon = await task.value
        loading[hash] = nil
        if let icon { images[hash] = icon }
        return icon
    }
}

/// An icon from `IconStore`: a shape in `tint`, or a picture.
struct StoredIcon: View {
    var hash: String
    var agent: String? = nil
    var project: String? = nil
    var tint: Color = Trek.foreground
    /// Room around a shape (a picture fills its frame).
    var inset: CGFloat = 0
    @State private var loaded: IconStore.Icon?

    var body: some View {
        let icon = loaded ?? IconStore.shared.cached(hash)
        Group {
            if let icon {
                if icon.template {
                    Image(uiImage: icon.image).renderingMode(.template).resizable().interpolation(.high).scaledToFit().foregroundStyle(tint)
                        .padding(inset)
                } else {
                    Image(uiImage: icon.image).resizable().interpolation(.high).scaledToFill()
                }
            } else {
                Color.clear
            }
        }
        .task(id: hash) { loaded = await IconStore.shared.icon(hash, agent: agent, project: project) }
    }
}
