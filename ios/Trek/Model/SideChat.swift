import Foundation

// A side chat from the phone (`side_chat`, mirrors crates/trek-remote/src/protocol/side_chat.rs):
// a quick question beside a thread, in a session of its own, which doesn't disturb the thread's
// agent. On the Mac it's the Side Chat panel; here a sheet over the thread, opened by `/side` or
// the thread's menu. A side chat is a thread no list holds: the Mac finds a thread's (or starts
// one with its first message) and answers with its row; from there it's read, sent to, answered
// and stopped like any other thread, by its id.

/// The reply to `side_chat`.
nonisolated struct SideChatReply: Decodable, Sendable {
    /// The thread it's beside, as asked.
    var threadId: String
    /// The side chat's row: the one found, or the one just started. Nil: the thread has none yet.
    var side: ThreadSummary? = nil
}

/// `/side`, as the composer reads it before anything goes to the Mac.
nonisolated enum SideCommand {
    static let name = "side"

    /// What follows `/side` in `text` ("" for the command alone), or nil when `text` isn't the
    /// command: another one that starts the same (`/sidebar`), or a message that only mentions it.
    static func question(_ text: String) -> String? {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let command = "/" + name
        guard t.count >= command.count, t.prefix(command.count).lowercased() == command else { return nil }
        let rest = t.dropFirst(command.count)
        guard rest.first.map(\.isWhitespace) ?? true else { return nil }
        return rest.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Its row in the `/` picker, one of Trek's own.
    static let info = CommandInfo(name: name, description: "Ask a quick question beside this thread", kind: .command, trek: true)

    /// The Mac's commands with `/side` among Trek's own: after the last of them, before the
    /// agent's. A list not in yet stays empty (the picker waits for the Mac's), and a Mac that
    /// lists one of its own keeps it.
    static func adding(to commands: [CommandInfo]) -> [CommandInfo] {
        guard !commands.isEmpty, !commands.contains(where: { $0.name == name }) else { return commands }
        var out = commands
        let at = commands.lastIndex { $0.trek == true }.map { $0 + 1 } ?? 0
        out.insert(info, at: at)
        return out
    }
}

/// What the side chat's sheet shows.
nonisolated enum SideChatPhase: Equatable {
    /// The Mac is being asked for the thread's side chat.
    case finding
    /// None yet, or a new one asked for: the first message starts it.
    case draft
    /// The side chat, by its (qualified) thread id.
    case chat(String)
    /// The Mac couldn't say: why.
    case unavailable(String)

    /// Where the sheet goes once the Mac has said which side chat the thread has (`found`, nil
    /// for none). A chat on screen that the Mac still has stays; one it no longer has gives way
    /// to the one it has, or to a new one. A draft the user asked for (`fresh`) stays a draft.
    func found(_ found: String?, fresh: Bool) -> SideChatPhase {
        if fresh, self == .draft { return self }
        return found.map(SideChatPhase.chat) ?? .draft
    }

    /// The Mac couldn't be asked: what's on screen stays if it's a chat (its transcript is here
    /// already) or a draft the user is writing; a sheet still waiting says why.
    func failed(_ why: String) -> SideChatPhase {
        self == .finding ? .unavailable(why) : self
    }
}

extension AppModel {
    /// Asks the Mac for the newest side chat beside `tid` (qualified). `done` gets its qualified
    /// id (nil: none yet), or why the Mac couldn't say.
    func findSideChat(of tid: String, done: @escaping (Result<String?, SideChatFailure>) -> Void) {
        sideChat(.sideChat(threadId: tid, text: nil, images: []), beside: tid, done: done)
    }

    /// Starts a side chat beside `tid` with its first message. `done` gets its qualified id.
    func startSideChat(beside tid: String, text: String, images: [ImageUpload] = [], done: @escaping (Result<String?, SideChatFailure>) -> Void) {
        sideChat(.sideChat(threadId: tid, text: text, images: images), beside: tid, done: done)
    }

    private func sideChat(_ message: ClientMessage, beside tid: String, done: @escaping (Result<String?, SideChatFailure>) -> Void) {
        let link = route(message)
        request(message) { [weak self, weak link] reply in
            switch reply {
            case .sideChat(_, let r):
                guard let self, let link, let side = r.side else {
                    self?.sideChats[tid] = nil
                    return done(.success(nil))
                }
                // Its row joins the ones no list holds: the sheet's thread reads it from there,
                // and the Mac sends it again as it changes while the sheet has it open.
                let row = self.qualify(side, on: link)
                self.setUnlisted(row)
                self.sideChats[tid] = row.id
                done(.success(row.id))
            case .error(_, _, let text):
                done(.failure(SideChatFailure(message: text)))
            default:
                done(.failure(SideChatFailure(message: "That Mac didn't answer about the side chat.")))
            }
        }
    }
}

/// Why a side chat couldn't be found or started, in the Mac's words.
nonisolated struct SideChatFailure: Error, Equatable {
    var message: String
}
