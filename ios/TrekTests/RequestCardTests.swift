import Foundation
import Testing
@testable import Trek

private func decoder() -> JSONDecoder {
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    return d
}

private func approval(_ extra: [String: Any] = [:]) throws -> ApprovalRequest {
    var json: [String: Any] = [
        "id": "rr1", "seq": 4, "kind": "approval", "request_id": "r1",
        "title": "Run command", "detail": "./deploy.sh --prod", "state": "pending",
    ]
    json.merge(extra) { $1 }
    let item = try decoder().decode(TItem.self, from: JSONSerialization.data(withJSONObject: json))
    guard case .approval(let a) = item.body else { throw CocoaError(.coderReadCorrupt) }
    return a
}

private func row(_ extra: [String: Any] = [:]) throws -> ThreadSummary {
    var json: [String: Any] = [
        "id": "t", "title": "Ship it",
        "project": ["id": "p", "name": "trek", "hue": 24, "monogram": "TR"],
        "agent": ["key": "claude-code", "name": "Claude Code"],
        "run_state": "needs-you", "section": "inbox", "unseen": false, "pinned": false, "worktree": false,
        "updated_at": 1_791_020_400_000, "additions": 0, "deletions": 0,
    ]
    json.merge(extra) { $1 }
    return try decoder().decode(ThreadSummary.self, from: JSONSerialization.data(withJSONObject: json))
}

struct SessionApprovalTests {
    @Test func aRuleThatAlwaysAsksTakesAwayAllowForSession() throws {
        let ruled = try approval(["asked_by": "Ask before running ./deploy.sh"])
        #expect(ruled.askedBy == "Ask before running ./deploy.sh")
        #expect(!ApprovalCard.offersSession(ruled, macAllows: true))
        #expect(!ApprovalCard.offersSession(ruled, macAllows: nil))
    }

    @Test func aMacFromBeforeTheRuleOffersItAsBefore() throws {
        let old = try approval()
        #expect(old.askedBy == nil)
        #expect(ApprovalCard.offersSession(old, macAllows: true))
        #expect(ApprovalCard.offersSession(old, macAllows: nil), "a Mac that doesn't say")
        #expect(!ApprovalCard.offersSession(old, macAllows: false), "Settings › Phone has it off")
    }

    @Test func theRuleSurvivesTheTranscriptCache() throws {
        let item = TItem(id: "rr1", seq: 4, body: .approval(ApprovalRequest(
            requestId: "r1", title: "Run command", detail: "./deploy.sh", state: .pending, askedBy: "Ask before ./deploy.sh")))
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        let back = try decoder().decode(TItem.self, from: e.encode(item))
        #expect(back == item)
    }
}

struct DiffCommentsHeldBackTests {
    @Test func aQuestionOrASecretKeepsTheCommentsBack() throws {
        let asking = try row(["needs": ["kind": "question", "text": "Which database?"]])
        #expect(DiffComments.heldBack(asking) == "Answer what Claude Code asked first, then send the comments.")
        // The Mac's row for a secret's card is a question too.
        let secret = try row(["needs": ["kind": "question", "text": "Needs GITHUB_TOKEN. Enter it on this Mac."]])
        #expect(DiffComments.heldBack(secret) != nil)
    }

    @Test func anythingElseLetsThemGo() throws {
        #expect(DiffComments.heldBack(try row(["run_state": "idle"])) == nil)
        #expect(DiffComments.heldBack(try row(["needs": ["kind": "approval", "text": "Run it?"]])) == nil,
                "they decline an approval, as a typed message does")
        // A sub-agent's question is answered in its own thread.
        #expect(DiffComments.heldBack(try row(["needs": ["kind": "question", "text": "Sub-agent needs you"],
                                               "needs_thread": "kid"])) == nil)
        #expect(DiffComments.heldBack(nil) == nil)
    }
}
