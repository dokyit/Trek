import UIKit

/// The demo Mac answering what the phone gained to draw a transcript as the Mac does
/// (`TranscriptParity.swift`): pictures and videos a thread names, the context card, the files an
/// undo would put back, and the limit bar's actions.
extension MockHost {
    func takeTranscriptParity(_ message: ClientMessage, id: String?) {
        switch message {
        case .media(let tid, let path, let maxPx, _, _):
            // Pictures are drawn; the demo Mac has no videos to send.
            guard let maxPx, let (data, w, h) = DemoMac.mediaImage(path, maxPx: min(maxPx, 2048)) else {
                return deliver(.error(re: id, code: .notFound, message: "That file isn't on the Mac any more"))
            }
            deliver(.media(re: id, MediaChunk(threadId: tid, path: path, mime: "image/jpeg", offset: 0, total: Int64(data.count),
                                              bytesB64: data.base64EncodedString(), width: w, height: h)), after: 0.25)
        case .contextCard(let tid):
            guard let t = thread(tid), let c = t.context else {
                return deliver(.error(re: id, code: .notFound, message: "No context reported yet"))
            }
            deliver(.contextCard(re: id, DemoMac.contextCard(c, working: t.runState == .working, paused: t.pause != nil)), after: 0.2)
        case .turnPreview(_, _, _):
            deliver(.turnPreview(re: id, TurnPreview(recap: false, files: [
                PreviewFile(path: "crates/trek-auth/src/session.rs", change: "revert"),
                PreviewFile(path: "crates/trek-auth/tests/session_cache.rs", change: "delete"),
                PreviewFile(path: "crates/trek-auth/src/cache.rs", change: "bring_back"),
            ])), after: 0.5)
        default:
            break
        }
    }

    /// The limit bar's actions and Compact now; false for the rest.
    func takeLimitAction(_ tid: String, _ action: ThreadAction, id: String?) -> Bool {
        switch action {
        case .resumeAtReset, .cancelResume:
            guard thread(tid)?.pause != nil else { deliver(.error(re: id, code: .conflict, message: "It isn't paused")); return true }
            ack(id)
            update(tid) { $0.pause?.resume = action == .resumeAtReset }
        case .tryAgain:
            ack(id)
            update(tid) { $0.pause = nil; $0.runState = .working; $0.workingSince = Self.now; $0.activity = "Trying again" }
            later(2.5) { [weak self] in
                self?.update(tid) { $0.runState = .idle; $0.activity = nil; $0.workingSince = nil }
            }
        case .snoozeUntilReset:
            guard let at = thread(tid)?.pause?.resetsAt else { deliver(.error(re: id, code: .conflict, message: "It isn't paused")); return true }
            ack(id)
            update(tid) { $0.snoozedUntil = at; $0.section = .snoozed; $0.pinned = false }
        case .compact:
            ack(id)
            update(tid) { t in
                if let c = t.context {
                    let used = c.used / 4
                    t.context = ContextUse(used: used, window: c.window, percent: Int(used * 100 / max(1, c.window)))
                }
            }
        default:
            return false
        }
        return true
    }
}

extension DemoMac {
    /// A picture for a path a demo thread names: drawn, a screen's worth of a made-up app.
    static func mediaImage(_ path: String, maxPx: Int) -> (Data, Int, Int)? {
        let frame = path.contains("frame")
        let full = frame ? CGSize(width: 1280, height: 720) : CGSize(width: 1600, height: 1000)
        let scale = min(1, CGFloat(maxPx) / max(full.width, full.height))
        let size = CGSize(width: (full.width * scale).rounded(), height: (full.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let seed = abs(path.hashValue % 360)
        let image = UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            let hue = CGFloat(seed) / 360
            UIColor(hue: hue, saturation: 0.35, brightness: 0.95, alpha: 1).setFill()
            ctx.fill(CGRect(origin: .zero, size: size))
            UIColor(hue: hue, saturation: 0.55, brightness: 0.7, alpha: 1).setFill()
            for i in 0..<6 {
                let y = size.height * (0.12 + CGFloat(i) * 0.13)
                ctx.fill(CGRect(x: size.width * 0.08, y: y, width: size.width * (0.84 - CGFloat(i % 3) * 0.18), height: size.height * 0.06))
            }
            UIColor.white.withAlphaComponent(0.85).setFill()
            ctx.fill(CGRect(x: size.width * 0.62, y: size.height * 0.55, width: size.width * 0.3, height: size.height * 0.36))
        }
        guard let data = image.jpegData(compressionQuality: 0.8) else { return nil }
        return (data, Int(size.width), Int(size.height))
    }

    /// The context card for `c`, split as the Mac estimates it.
    static func contextCard(_ c: ContextUse, working: Bool, paused: Bool) -> ContextCard {
        let reserved = c.window / 30
        let system: Int64 = 21_000
        let mcp: Int64 = 9_000
        let skills: Int64 = 1_200
        let messages = max(0, c.used - system - mcp - skills)
        return ContextCard(used: c.used, window: c.window, reserved: min(reserved, c.window - c.used), parts: [
            ContextPart(kind: "system", label: "System", tokens: system,
                        detail: [ContextDetail(label: "The agent's own prompt and its tools' definitions")],
                        note: "Estimated: what the agent counts that isn't a message, skill or server."),
            ContextPart(kind: "mcp", label: "MCP servers", tokens: mcp,
                        detail: [ContextDetail(label: "trek-orchestrate, trek-browser", value: "1.8k"),
                                 ContextDetail(label: "Instructions", value: "7.2k")],
                        note: "Estimated, about 900 tokens a server."),
            ContextPart(kind: "skills", label: "Skills", tokens: skills,
                        detail: [ContextDetail(label: "15 skills", value: "1.2k")], note: "Estimated, about 80 tokens a skill."),
            ContextPart(kind: "messages", label: "Messages", tokens: messages,
                        detail: [ContextDetail(label: "42 messages and tool results")], note: nil),
            ContextPart(kind: "free", label: "Free", tokens: max(0, c.window - c.used - min(reserved, c.window - c.used))),
            ContextPart(kind: "reserved", label: "Reserved for compacting", tokens: min(reserved, c.window - c.used)),
        ], compact: working || paused ? "busy" : "ready")
    }

    /// A message sent with consultants, as the Mac writes it for the agent.
    static func withConsult(_ text: String, _ consult: ConsultRequest) -> String {
        let style = consult.styleKey
        let judge = consult.judge.map { " judge=\"\($0)\"" } ?? ""
        return "\(text)\n\n<trek-consult style=\"\(style)\" then=\"\(consult.implement ? "implement" : "report")\" with=\"\(consult.with.joined(separator: ","))\"\(judge)>\nBefore you act on this, consult other models through Trek's `delegate_task` tool:\n\n…\n</trek-consult>"
    }
}
