import Foundation

/// A demo thread with what the Mac's transcript draws besides plain messages and answers (`t-parity`):
/// a message sent with consultants, one that asked for a restatement first, sub-agents reporting
/// back, a thought in Markdown, code with its language.
extension DemoMac {
    static func parityThread(endingAt now: Int64) -> [(ItemBody, Int64?, ItemExtras?)] {
        let m: Int64 = 60_000
        func tool(_ kind: ToolKind, _ title: String, _ detail: String, _ status: ToolStatus = .done, out: String = "",
                  add: Int? = nil, del: Int? = nil) -> ItemBody {
            .tool(ToolCall(callId: UUID().uuidString, tool: kind, title: title, detail: detail, status: status, output: out, added: add, removed: del))
        }
        let consult = """
        Session lookups hit Postgres on every request. Cache them, and have Sol and Opus look over the design first.

        <trek-consult style="advise" then="implement" with="codex/gpt-5.6-sol/high,claude-code/claude-opus-5-5/max">
        Before you act on this, consult other models through Trek's `delegate_task` tool:

        1. Call `delegate_task` for each consultant below…

        If `delegate_task` isn't available to you, say so and stop.
        </trek-consult>
        """
        let restate = """
        Then make the cache survive a restart.

        <trek-restate>
        Before you do anything else, restate in your own words and in plain English what you understand I'm asking for, and the problem underneath it. Read what you need to understand it, but don't change any files or start on the work, and don't hand it to anyone else yet. Then stop, and wait for me to confirm or correct you.
        </trek-restate>
        """
        let wake = """
        [Trek] Sub-agent “Review the cache design” (GPT-5.6 Sol, task t-41) finished:

        The design holds. Two things: key the cache on the session id **and** its rotation counter, or a rotated token reads a stale row; and bound it (an LRU of 10k entries is plenty for one node).

        ---

        [Trek] Sub-agent “Find every session read” (Opus 5.5, task t-42) finished:

        Seven reads, all through `SessionStore::get`, so one wrapper covers them.
        """
        let thought = """
        Two reviewers agree on the shape. What's left:

        - key on `(session_id, rotation)` so a **rotated** token misses
        - an LRU bound, `10_000` entries
        - invalidate on logout

        The store is behind `SessionStore::get`, so the cache wraps that one call.
        """
        let answer = """
        The cache wraps `SessionStore::get`, keyed on the session id and its rotation counter, so a rotated token never reads a stale row:

        ```rust
        pub fn get(&self, id: SessionId, rotation: u32) -> Option<Session> {
            if let Some(hit) = self.cache.lock().get(&(id, rotation)) {
                return Some(hit.clone());
            }
            let row = self.db.session(id)?;
            self.cache.lock().put((id, rotation), row.clone());
            Some(row)
        }
        ```

        Logging out evicts the entry. Requests that read a session went from 3.1 ms to 0.4 ms at p50.
        """
        let shot = "The settings page after the change, as the app draws it:\n\n![Settings, after](docs/screens/settings-after.png)\n\nAnd the whole flow, recorded:\n\n[flow.mp4](docs/screens/flow.mp4)"
        let verified = ItemExtras(verified: Verification(passed: true, commands: ["./.agents/skills/verify-trek/scripts/trek-dev check"]))
        let watched = ItemExtras(watch: WatchedMedia(source: "docs/screens/flow.mp4", video: "/Users/demo/trek/docs/screens/flow.mp4", duration: 14,
                                                     frames: (0..<6).map { WatchedFrame(t: Double($0) * 2.5, path: "/tmp/watch/frame-\($0).jpg") }))
        let rows: [(ItemBody, Int64?, ItemExtras?)] = [
            (.user(text: consult, images: 0), now - 40 * m, nil),
            (tool(.agent, "Subagent", "Review the cache design"), nil, nil),
            (tool(.agent, "Subagent", "Find every session read"), nil, nil),
            (.assistant(text: "I've asked Sol and Opus to look over the design; I'll pick it up when they report.", streaming: false), nil, nil),
            (.turnEnd(tookSecs: 34), now - 39 * m, nil),
            (.user(text: wake, images: 0), now - 31 * m, nil),
            (.reasoning(text: thought), nil, nil),
            (tool(.read, "Read", "crates/trek-auth/src/session.rs"), nil, nil),
            (tool(.edit, "Edit", "crates/trek-auth/src/session.rs", add: 48, del: 9), nil, nil),
            (tool(.edit, "Write", "crates/trek-auth/tests/session_cache.rs", add: 16, del: 3), nil, nil),
            (tool(.command, "Run", "cargo test -p trek-auth session_cache", .failed, out: "test session_cache::rotated_token_misses ... FAILED"), nil, nil),
            (tool(.command, "Run", "cargo test -p trek-auth", out: "test result: ok. 41 passed; 0 failed"), nil, nil),
            (tool(.command, "Run", "./.agents/skills/verify-trek/scripts/trek-dev check", out: "{\"ok\": true}"), nil, nil),
            (.assistant(text: answer, streaming: false), nil, nil),
            (.turnEnd(tookSecs: 412), now - 24 * m, verified),
            (.user(text: "Show me the settings page after the change.\n\n@/Users/demo/trek/docs/screens/before.mov", images: 0), now - 20 * m,
             ItemExtras(videos: ["/Users/demo/trek/docs/screens/before.mov"])),
            (tool(.other, "watch_video", "docs/screens/flow.mp4", out: "Manifest: /tmp/watch/manifest.json"), nil, watched),
            (.assistant(text: shot, streaming: false), nil, nil),
            (.turnEnd(tookSecs: 48), now - 18 * m, nil),
            (.user(text: restate, images: 0), now - 6 * m, nil),
            (.assistant(text: "You want the session cache to keep its entries across a restart of the server, so the first requests after a deploy don't all go to Postgres at once. Underneath: a cold cache after every deploy makes p99 spike for a minute. Is that what you meant?", streaming: false), nil, nil),
            (.turnEnd(tookSecs: 21), now - 5 * m, nil),
        ]
        return rows
    }
}

extension DemoMac {
    /// `t-limit`: Codex handed it over, then Claude's usage limit stopped it.
    static func limitThread(endingAt now: Int64) -> [(ItemBody, Int64?, ItemExtras?)] {
        let m: Int64 = 60_000
        let codex = AgentRef(key: "codex", name: "Codex", logo: "codex")
        let claudeRef = AgentRef(key: "claude-code", name: "Claude Code", logo: "claude-code")
        return [
            (.user(text: "Port the docs site's search index from TypeScript to Rust, behind the same API.", images: 0), now - 50 * m, nil),
            (.handoff(from: "Codex", to: "Claude Code"), now - 48 * m, ItemExtras(fromAgent: codex, toAgent: claudeRef)),
            (.tool(ToolCall(callId: "c-l1", tool: .read, title: "Read", detail: "search/index.ts", status: .done, output: "", added: nil, removed: nil)), nil, nil),
            (.assistant(text: "The index is a plain inverted map with BM25 scoring; I'll port it as a `SearchIndex` struct with the same three calls.", streaming: false), nil, nil),
            (.limit(text: "Claude usage limit reached. Your limit resets at 4:30 PM.", resetsAt: now + 134 * m), now - 30 * m, nil),
        ]
    }
}
