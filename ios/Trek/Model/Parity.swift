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
