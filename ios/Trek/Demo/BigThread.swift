import Foundation

// MARK: A huge thread, to measure with

extension DemoMac {
    /// `-TrekBigThread 3000`: how many items the demo's long thread has (0: no such thread).
    static var bigThreadSize: Int { UserDefaults.standard.integer(forKey: "TrekBigThread") }

    /// About `count` items of a long working session, turn after turn, every kind the phone draws:
    /// a message, thinking and tool calls, a Markdown answer (lists, code, chips, sometimes a
    /// table), the turn's end and what it changed. `at` is set on messages and turn ends.
    static func bigThread(_ count: Int, endingAt now: Int64) -> [(ItemBody, Int64?)] {
        let files = ["crates/trek-core/src/store.rs", "crates/trek-app/src/thread_view.rs", "crates/trek-remote/src/protocol.rs",
                     "ios/Trek/Views/Transcript.swift", "ios/Trek/Model/AppModel.swift", "docs/MOBILE.md", "Cargo.toml"]
        let asks = ["Make the transcript load faster on the phone.", "Why does `subscribe` send the whole thread?",
                    "Add paging to the protocol, and keep the scroll position when a page arrives.",
                    "Profile the markdown renderer with a 3,000-item thread.", "Cache parsed markdown by item id.",
                    "Write tests for the reset after a rewind.", "Tidy up the naming in `store.rs`."]
        var out: [(ItemBody, Int64?)] = []
        let turns = max(1, (count + 9) / 10)
        for t in 0..<turns {
            let at = now - Int64(turns - t) * 7 * 60_000
            let file = files[t % files.count]
            let other = files[(t + 3) % files.count]
            out.append((.user(text: "\(asks[t % asks.count]) (step \(t + 1))", images: t % 17 == 0 ? 1 : 0), at))
            out.append((.reasoning(text: "Turn \(t + 1): I'll read `\(file)` first, then check how `\(other)` uses it before changing anything."), nil))
            out.append((.tool(ToolCall(callId: "b\(t)-1", tool: .read, title: "Read", detail: file, status: .done, output: "", added: nil, removed: nil)), nil))
            out.append((.tool(ToolCall(callId: "b\(t)-2", tool: .search, title: "Search", detail: "\"fn transcript\" in crates/", status: .done,
                                       output: "", added: nil, removed: nil)), nil))
            out.append((.assistant(text: "Looking at `\(file)`, the slow part is that **every** update rebuilds the whole list. I'll keep the blocks and only touch the ones that changed.", streaming: false), nil))
            out.append((.tool(ToolCall(callId: "b\(t)-3", tool: .edit, title: "Edit", detail: file, status: .done, output: "",
                                       added: 12 + t % 30, removed: t % 9)), nil))
            out.append((.tool(ToolCall(callId: "b\(t)-4", tool: .command, title: "Run", detail: "cargo test -p trek-core transcript",
                                       status: t % 13 == 5 ? .failed : .done,
                                       output: "running 42 tests\n..........................................\ntest result: ok. 42 passed; 0 failed",
                                       added: nil, removed: nil)), nil))
            var answer = "## Step \(t + 1)\n\nDone. The change is in `\(file)` and `\(other)`:\n\n- Blocks are built *once*, when items arrive\n- Markdown is parsed once per item and cached\n- The composer re-renders on its own\n\n```swift\nlet blocks = store.blocks // built as items arrive\nForEach(blocks) { BlockRow(block: $0) }\n```\n\nThe tests pass (**42** of 42)."
            if t % 5 == 0 {
                answer += "\n\n| Case | Before | After |\n|---|---:|---:|\n| Open | 1.9 s | 80 ms |\n| Type | 120 ms | 4 ms |"
            }
            out.append((.assistant(text: answer, streaming: false), nil))
            out.append((.turnEnd(tookSecs: 20 + (t * 37) % 400), at + 3 * 60_000))
            let changed = [ChangedFile(path: file, status: .modified, added: 12 + t % 30, removed: t % 9),
                           ChangedFile(path: other, status: t % 4 == 0 ? .added : .modified, added: 6, removed: 1)]
            out.append((.changes(TurnChanges(files: changed, added: changed.reduce(0) { $0 + $1.added },
                                             removed: changed.reduce(0) { $0 + $1.removed })), at + 3 * 60_000))
        }
        return out
    }
}
