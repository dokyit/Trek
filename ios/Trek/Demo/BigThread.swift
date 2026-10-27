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

// MARK: Real content, and a stand-in for it

extension DemoMac {
    /// `-TrekBigRich YES` (with `-TrekBigThread 5000`): the long thread is made of rows as uneven
    /// as a real one's: answers from one line to several screens (headings, lists, code, tables),
    /// long runs of tool calls, long messages. The transcript's scrolling is measured on this.
    static var richThread: Bool { UserDefaults.standard.bool(forKey: "TrekBigRich") }

    /// `-TrekFixture /path/to/thread.json`: a real thread's items as wire JSON, exported read-only
    /// by `ios/scripts/export-thread.py` (the simulator reads the Mac's files directly).
    static var fixturePath: String? { UserDefaults.standard.string(forKey: "TrekFixture") }
    /// `-TrekFixtureTitle "…"`: the fixture thread's title.
    static var fixtureTitle: String? { UserDefaults.standard.string(forKey: "TrekFixtureTitle") }

    static func fixture(at path: String) -> [TItem]? {
        guard let data = FileManager.default.contents(atPath: path) else { return nil }
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return try? d.decode([TItem].self, from: data)
    }

    /// About `count` items whose rows differ in height as much as a real thread's do. The same
    /// every launch (a fixed generator), so a scroll run can be repeated.
    static func richThread(_ count: Int, endingAt now: Int64) -> [(ItemBody, Int64?)] {
        var rng = SplitMix(seed: 17)
        let words = ("the transcript keeps its place while rows above it are measured and a page of earlier items "
            + "lands, so the reader never loses the line they were on even when the answer is long and full of code "
            + "tables lists and links to files like store.rs or ThreadView.swift").split(separator: " ").map(String.init)
        func word() -> String { words[rng.int(words.count)] }
        func sentence(_ n: Int) -> String {
            let s = (0..<n).map { _ in word() }.joined(separator: " ")
            return s.prefix(1).uppercased() + s.dropFirst() + "."
        }
        func paragraph() -> String { (0..<(1 + rng.int(5))).map { _ in sentence(6 + rng.int(18)) }.joined(separator: " ") }
        func code() -> String {
            let n = 3 + rng.int(rng.int(4) == 0 ? 60 : 14)
            let lines = (0..<n).map { i in "let row\(i) = layout.frame(for: index + \(i)) // \(word())" }
            return "```swift\n" + lines.joined(separator: "\n") + "\n```"
        }
        func table() -> String {
            let rows = (0..<(2 + rng.int(10))).map { i in "| case \(i) | \(rng.int(900)) ms | \(rng.int(90)) ms | \(word()) |" }
            return "| Case | Before | After | Note |\n|---|---:|---:|---|\n" + rows.joined(separator: "\n")
        }
        func list() -> String {
            (0..<(2 + rng.int(7))).map { _ in "- **\(word()):** \(sentence(5 + rng.int(20)))" }.joined(separator: "\n")
        }
        func answer() -> String {
            switch rng.int(10) {
            case 0...3: return sentence(8 + rng.int(20))
            case 4...6: return (0..<(1 + rng.int(3))).map { _ in rng.int(2) == 0 ? paragraph() : list() }.joined(separator: "\n\n")
            default:
                // A long answer: several screens of mixed blocks.
                var parts = ["## \(sentence(3 + rng.int(4)))"]
                for _ in 0..<(4 + rng.int(12)) {
                    switch rng.int(6) {
                    case 0: parts.append(code())
                    case 1: parts.append(table())
                    case 2: parts.append(list())
                    case 3: parts.append("### \(sentence(2 + rng.int(4)))")
                    default: parts.append(paragraph())
                    }
                }
                return parts.joined(separator: "\n\n")
            }
        }
        let kinds: [(ToolKind, String)] = [(.read, "Read"), (.search, "Search"), (.edit, "Edit"), (.command, "Run")]
        var out: [(ItemBody, Int64?)] = []
        while out.count < count {
            let at = now - Int64(count - out.count) * 40_000
            let ask = rng.int(6) == 0 ? (0..<(2 + rng.int(4))).map { _ in paragraph() }.joined(separator: "\n\n") : sentence(6 + rng.int(25))
            out.append((.user(text: ask, images: rng.int(15) == 0 ? 1 + rng.int(3) : 0), at))
            for _ in 0..<(1 + rng.int(6)) {
                // A run of thinking and tool calls (one group), then what the agent said.
                for _ in 0..<(1 + rng.int(rng.int(3) == 0 ? 18 : 4)) {
                    if rng.int(3) == 0 {
                        out.append((.reasoning(text: paragraph()), nil))
                    } else {
                        let (kind, title) = kinds[rng.int(kinds.count)]
                        out.append((.tool(ToolCall(callId: "rich-\(out.count)", tool: kind, title: title,
                                                   detail: "crates/trek-core/src/\(word()).rs", status: rng.int(17) == 0 ? .failed : .done,
                                                   output: kind == .command ? "test result: ok. \(rng.int(90)) passed" : "",
                                                   added: kind == .edit ? rng.int(80) : nil, removed: kind == .edit ? rng.int(20) : nil)), nil))
                    }
                }
                out.append((.assistant(text: answer(), streaming: false), nil))
            }
            out.append((.turnEnd(tookSecs: 20 + rng.int(900)), at + 3 * 60_000))
        }
        return Array(out.prefix(count))
    }
}

/// A small fixed-seed generator: the same thread every launch.
nonisolated struct SplitMix {
    var state: UInt64
    init(seed: UInt64) { state = seed }
    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
    mutating func int(_ n: Int) -> Int { n <= 0 ? 0 : Int(next() % UInt64(n)) }
}
