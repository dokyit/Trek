import Foundation

// What the phone gained to draw a transcript as the Mac does (docs/MOBILE.md, "Transcript
// components"): fields newer Macs add to items and rows, and the requests behind them. An older
// Mac sends none of the fields and lists none of the features; the phone then draws what it did.

/// A turn that ran the project's verification CLI: whether its last run passed, and what ran.
nonisolated struct Verification: Codable, Hashable {
    var passed: Bool
    /// Newest last.
    var commands: [String] = []
}

/// What a `watch_video` call watched: the video (when it's still on the Mac) and the frames the
/// model saw.
nonisolated struct WatchedMedia: Codable, Hashable {
    var source: String = ""
    var video: String? = nil
    var duration: Double? = nil
    var frames: [WatchedFrame] = []
}

nonisolated struct WatchedFrame: Codable, Hashable {
    /// Seconds into the video.
    var t: Double
    var path: String
}

/// Item fields beside the kind's own.
nonisolated struct ItemExtras: Hashable {
    /// A `turn_end`'s verification.
    var verified: Verification? = nil
    /// A `handoff`'s agents, for their glyphs.
    var fromAgent: AgentRef? = nil
    var toAgent: AgentRef? = nil
    /// Videos a message names (`@/path/clip.mov`), on the Mac.
    var videos: [String] = []
    /// A `watch_video` call's video and frames.
    var watch: WatchedMedia? = nil
}

/// A thread paused at its agent's usage limit (the Mac's limit bar).
nonisolated struct LimitPause: Codable, Hashable {
    /// When the limit resets (ms), if the agent said.
    var resetsAt: Int64? = nil
    /// It sends what waits by itself once the limit resets.
    var resume: Bool = false
    /// Trek stopped it ahead of the limit, rather than the limit stopping it.
    var wrapped: Bool = false
}

/// The reply to `media`: a file's bytes from `offset` (a whole scaled-down image, or a piece of a
/// video), and how long it is in all.
nonisolated struct MediaChunk: Decodable, Sendable {
    var threadId: String? = nil
    var path: String
    var mime: String
    var offset: Int64 = 0
    var total: Int64
    var bytesB64: String
    var width: Int? = nil
    var height: Int? = nil

    /// The same piece, as the notes' file downloader takes it.
    var asNoteChunk: NoteFileChunk { NoteFileChunk(path: path, offset: offset, total: total, bytesB64: bytesB64, mime: mime) }
}

/// The reply to `context_card`: what fills the thread's context window, as the Mac's card shows it.
nonisolated struct ContextCard: Decodable, Hashable {
    var threadId: String? = nil
    var used: Int64
    var window: Int64
    /// What the agent keeps free so compacting has room (an estimate).
    var reserved: Int64 = 0
    var parts: [ContextPart] = []
    /// "ready", "running", "busy" (a turn runs, or it's paused), "none" (nothing to compact).
    var compact: String = "none"
}

nonisolated struct ContextPart: Decodable, Hashable, Identifiable {
    /// "system", "mcp", "skills", "messages", or one the phone doesn't know.
    var kind: String
    var label: String
    var tokens: Int64
    var detail: [ContextDetail] = []
    var note: String? = nil
    var id: String { kind + label }
}

nonisolated struct ContextDetail: Decodable, Hashable {
    var label: String
    var value: String? = nil
}

/// The reply to `turn_preview`: the files restoring would put back, each with what happens to it.
nonisolated struct TurnPreview: Decodable, Hashable {
    var threadId: String? = nil
    var itemId: String? = nil
    /// The agent can't take its session back: it carries on in a new one, with a recap.
    var recap: Bool = false
    /// Nil when there's no checkpoint to restore (`unavailable` says why).
    var files: [PreviewFile]? = nil
    var unavailable: String? = nil
}

nonisolated struct PreviewFile: Decodable, Hashable, Identifiable {
    var path: String
    /// "revert", "delete", "bring_back", "nested", "kept".
    var change: String
    var id: String { path }

    /// What restoring does to it, as the Mac's confirmation says it.
    var verb: String {
        switch change {
        case "revert": "revert"
        case "delete": "delete"
        case "bring_back": "bring back"
        case "kept": "left as is (newer)"
        default: "left as is"
        }
    }
}

/// Consultants for a message (`send` with `consult`), keyed as the Mac keys them
/// (`codex/gpt-5.6-sol/high`).
nonisolated struct ConsultRequest: Hashable {
    var with: [String]
    var style: MessageParts.Style = .advise
    var implement: Bool = true
    var judge: String? = nil

    var styleKey: String {
        switch style {
        case .advise: "advise"
        case .discuss: "discuss"
        case .arena: "arena"
        }
    }

    var json: [String: Any] {
        var o: [String: Any] = ["with": with, "style": styleKey, "then": implement ? "implement" : "report"]
        if let judge { o["judge"] = judge }
        return o
    }
}

nonisolated extension ClientMessage {
    /// The JSON of the requests above.
    var transcriptParityJSON: [String: Any] {
        switch self {
        case .media(let threadId, let path, let maxPx, let offset, let length):
            var o: [String: Any] = ["type": "media", "thread_id": threadId, "path": path, "offset": offset]
            if let maxPx { o["max_px"] = maxPx }
            if let length { o["length"] = length }
            return o
        case .contextCard(let threadId):
            return ["type": "context_card", "thread_id": threadId]
        case .turnPreview(let threadId, let itemId, let action):
            return ["type": "turn_preview", "thread_id": threadId, "item_id": itemId, "action": action.rawValue]
        case .sendConsult(let threadId, let text, let mode, let images, let consult):
            var o: [String: Any] = ["type": "send", "thread_id": threadId, "text": text, "mode": mode?.rawValue ?? NSNull(),
                                    "consult": consult.json]
            if !images.isEmpty { o["images"] = images.map(\.json) }
            return o
        default:
            return [:]
        }
    }
}
