import Foundation
import Testing
@testable import Trek

// An answer reads on the phone as it does on the Mac: what the block parser makes of the Markdown
// agents write (`Markdown.parse`), and that an answer still streaming is read the same way a
// piece at a time as it would be whole.

private typealias B = Markdown.Block

private func p(_ s: String) -> B { .paragraph(s) }
private func item(_ blocks: B..., task: Bool? = nil) -> Markdown.Item { Markdown.Item(task: task, blocks: blocks) }
private func bullets(loose: Bool = false, _ items: Markdown.Item...) -> B {
    .list(Markdown.List(ordered: false, start: 0, loose: loose, items: items))
}
private func numbered(from start: Int = 1, loose: Bool = false, _ items: Markdown.Item...) -> B {
    .list(Markdown.List(ordered: true, start: start, loose: loose, items: items))
}
private func code(_ lang: String, _ code: String, closed: Bool = true) -> B { .code(lang: lang, code: code, closed: closed) }
private func picture(_ name: String, _ link: String) -> MediaLink { MediaLink(name: name, link: link) }

struct MarkdownRuleTests {
    @Test func aRuleIsABlockOfItsOwn() {
        for rule in ["---", "***", "___", "- - -", "* * *", "_____", "---  "] {
            #expect(Markdown.parse("Above.\n\n\(rule)\n\nBelow.") == [p("Above."), .rule, p("Below.")], "\(rule)")
        }
        #expect(Markdown.parse("--") == [p("--")], "two aren't a rule")
        #expect(Markdown.parse("**bold**") == [p("**bold**")])
    }

    @Test func theRecapStartsUnderItsRule() {
        // What Trek asks agents to start a work answer with (`changes::RECAP`): the rule ran into
        // "**Done**" as "--- **Done** …" before.
        let recap = "---\n**Done** The cache survives a restart.\n\n**Still to do** Nothing.\n\n**Found** The old flag can go.\n\nDetail follows."
        #expect(Markdown.parse(recap) == [.rule, p("**Done** The cache survives a restart."), p("**Still to do** Nothing."),
                                          p("**Found** The old flag can go."), p("Detail follows.")])
        #expect(Markdown.parse("---\n\n**Done**\nAll of it.") == [.rule, p("**Done** All of it.")])
        // A rule right under text ends the paragraph: `***` and `___` always, as on the Mac.
        #expect(Markdown.parse("Above.\n***\nBelow.") == [p("Above."), .rule, p("Below.")])
    }

    @Test func aParagraphOverDashesOrEqualsIsAHeading() {
        // CommonMark's setext headings, which the Mac reads.
        #expect(Markdown.parse("What changed\n---\nThe cache.") == [.heading(2, "What changed"), p("The cache.")])
        #expect(Markdown.parse("What changed\n===\n") == [.heading(1, "What changed")])
        // Not while the line under it is still being written: "-" may be a bullet on its way.
        #expect(Markdown.parse("The steps:\n-") == [p("The steps: -")])
        #expect(Markdown.parse("The steps:\n- one") == [p("The steps:"), bullets(item(p("one")))])
    }

    @Test func aLineEndingInTwoSpacesOrABackslashBreaks() {
        #expect(Markdown.parse("**Done** one  \n**Found** two") == [p("**Done** one\n**Found** two")])
        #expect(Markdown.parse("one\\\ntwo") == [p("one\ntwo")])
        #expect(Markdown.parse("one\ntwo") == [p("one two")])
    }
}

struct MarkdownListTests {
    @Test func aNumberedListKeepsTheNumberItStartsAt() {
        #expect(Markdown.parse("3. three\n4. four") == [numbered(from: 3, item(p("three")), item(p("four")))])
        #expect(Markdown.parse("7) seven\n8) eight") == [numbered(from: 7, item(p("seven")), item(p("eight")))])
        // The first number counts; the rest follow it, as on the Mac.
        #expect(Markdown.parse("1. a\n1. b\n1. c") == [numbered(item(p("a")), item(p("b")), item(p("c")))])
    }

    @Test func blankLinesBetweenItemsDontRestartTheNumbers() {
        // Every item of a loose list read "1." before.
        #expect(Markdown.parse("1. one\n\n2. two\n\n3. three") == [numbered(loose: true, item(p("one")), item(p("two")), item(p("three")))])
        #expect(Markdown.parse("- a\n\n- b") == [bullets(loose: true, item(p("a")), item(p("b")))])
    }

    @Test func whatIsUnderAnItemStaysInItsList() {
        let text = """
        1. Write it.
        2. Read it back:

           ```rust
           let cache = load();
           ```

           - only fresh entries
           - at most 10k

        3. Drop the stale ones.
        """
        #expect(Markdown.parse(text) == [numbered(loose: true,
            item(p("Write it.")),
            item(p("Read it back:"), code("rust", "let cache = load();"), bullets(item(p("only fresh entries")), item(p("at most 10k")))),
            item(p("Drop the stale ones.")))])
    }

    @Test func codeWrittenAtTheMarginBetweenItemsKeepsTheirNumbers() {
        // Not in the list by CommonMark, so the list ends and another starts: at 2, not at 1.
        let text = "1. Build it.\n\n```sh\nmake\n```\n\n2. Run it.\n3. Ship it."
        #expect(Markdown.parse(text) == [numbered(item(p("Build it."))), code("sh", "make"),
                                         numbered(from: 2, item(p("Run it.")), item(p("Ship it.")))])
    }

    @Test func subItemsAreNestedByTheirIndentation() {
        let text = """
        - one
          - two
            - three
              1. four
              2. five
        - six
        """
        #expect(Markdown.parse(text) == [bullets(
            item(p("one"), bullets(item(p("two"), bullets(item(p("three"), numbered(item(p("four")), item(p("five")))))))),
            item(p("six")))])
        // Two spaces under "1." is how agents write sub-bullets; the phone takes them as nested.
        #expect(Markdown.parse("1. one\n  - sub\n  - sub two\n2. two")
            == [numbered(item(p("one"), bullets(item(p("sub")), item(p("sub two")))), item(p("two")))])
        // A tab nests as well.
        #expect(Markdown.parse("- one\n\t- two") == [bullets(item(p("one"), bullets(item(p("two")))))])
    }

    @Test func aParagraphUnderAnItemBelongsToIt() {
        #expect(Markdown.parse("1. one\n\n   More about one.\n\n2. two")
            == [numbered(loose: true, item(p("one"), p("More about one.")), item(p("two")))])
        // A line carried on at the margin is still the item's (a lazy continuation).
        #expect(Markdown.parse("- one\ncarried on\n- two") == [bullets(item(p("one carried on")), item(p("two")))])
        // After a blank line, text at the margin is a paragraph after the list.
        #expect(Markdown.parse("- one\n\nAfter.") == [bullets(item(p("one"))), p("After.")])
    }

    @Test func aDifferentMarkerStartsAnotherList() {
        #expect(Markdown.parse("- a\n- b\n1. c") == [bullets(item(p("a")), item(p("b"))), numbered(item(p("c")))])
        #expect(Markdown.parse("• a\n• b") == [.list(Markdown.List(ordered: false, start: 0, loose: false, items: [item(p("a")), item(p("b"))]))])
    }

    @Test func numbersInTextArentLists() {
        #expect(Markdown.parse("It shipped in\n2019. Then it grew.") == [p("It shipped in 2019. Then it grew.")],
                "only a 1. starts a list right under text")
        #expect(Markdown.parse("1.5 seconds") == [p("1.5 seconds")])
        #expect(Markdown.parse("-5 degrees") == [p("-5 degrees")])
    }

    @Test func taskItemsCarryTheirBox() {
        let text = "- [ ] write the note\n- [x] test it\n- [X] ship it\n- plain"
        #expect(Markdown.parse(text) == [bullets(item(p("write the note"), task: false), item(p("test it"), task: true),
                                                 item(p("ship it"), task: true), item(p("plain")))])
        #expect(Markdown.parse("1. [x] done") == [numbered(item(p("done"), task: true))])
        #expect(Markdown.parse("- [link](x)") == [bullets(item(p("[link](x)")))], "a link isn't a box")
        #expect(Markdown.parse("- [ ]no space") == [bullets(item(p("[ ]no space")))])
    }
}

struct MarkdownQuoteTests {
    @Test func consecutiveLinesAreOneQuote() {
        // Each line was a quote of its own, with a bar each.
        #expect(Markdown.parse("> one\n> two\n> three") == [.quote([p("one two three")])])
        #expect(Markdown.parse("> one\n>\n> two") == [.quote([p("one"), p("two")])])
        #expect(Markdown.parse(">no space") == [.quote([p("no space")])])
        #expect(Markdown.parse("> one\n\n> two") == [.quote([p("one")]), .quote([p("two")])], "a blank line ends it")
    }

    @Test func aQuoteHoldsBlocksOfItsOwn() {
        #expect(Markdown.parse("> Note:\n> - a\n> - b") == [.quote([p("Note:"), bullets(item(p("a")), item(p("b")))])])
        #expect(Markdown.parse("> ```sh\n> make\n> ```") == [.quote([code("sh", "make")])])
        #expect(Markdown.parse("> one\ncarried on\n\nAfter.") == [.quote([p("one carried on")]), p("After.")])
        #expect(Markdown.parse("- item\n  > quoted\n  > more") == [bullets(item(p("item"), .quote([p("quoted more")])))])
    }
}

struct MarkdownFenceTests {
    @Test func aFenceClosesOnlyOnItsOwnKindAndLength() {
        // Markdown shown inside Markdown: the inner ``` closed the block before.
        let text = "````markdown\nReload with:\n\n```sh\ntrek cache reload\n```\n````\n\nAfter."
        #expect(Markdown.parse(text) == [code("markdown", "Reload with:\n\n```sh\ntrek cache reload\n```"), p("After.")])
        #expect(Markdown.parse("~~~\n```\ninside\n```\n~~~") == [code("", "```\ninside\n```")])
        #expect(Markdown.parse("```\n~~~\n```") == [code("", "~~~")])
        // A longer run closes too; one with text after it doesn't.
        #expect(Markdown.parse("```\na\n`````\nb") == [code("", "a"), p("b")])
        #expect(Markdown.parse("```\na\n``` not the end\nb\n```") == [code("", "a\n``` not the end\nb")])
    }

    @Test func aFenceStillOpenSaysSo() {
        #expect(Markdown.parse("```rust\nfn main() {") == [code("rust", "fn main() {", closed: false)])
        #expect(Markdown.parse("```rust\nfn main() {}\n```") == [code("rust", "fn main() {}")])
        #expect(Markdown.parse("```trek-html\n<p>half") == [code("trek-html", "<p>half", closed: false)])
        if case .viz(.arriving)? = Markdown.parse("```trek-viz\n{\"type\":\"bar\"").first {} else { Issue.record("an open trek-viz block is arriving") }
    }

    @Test func codeKeepsItsLinesAsWritten() {
        #expect(Markdown.parse("```make\nall:\n\tcc main.c\n\n  # indented\n```") == [code("make", "all:\n\tcc main.c\n\n  # indented")])
        // In a list item, less the item's own indentation.
        #expect(Markdown.parse("- run:\n  ```sh\n  if x; then\n    y\n  fi\n  ```")
            == [bullets(item(p("run:"), code("sh", "if x; then\n  y\nfi")))])
        // Code in a line isn't a fence.
        #expect(Markdown.parse("```inline``` and text") == [p("```inline``` and text")])
    }

    @Test func aFenceEndsAParagraph() {
        #expect(Markdown.parse("Run:\n```sh\nmake\n```\nThen wait.") == [p("Run:"), code("sh", "make"), p("Then wait.")])
    }
}

struct MarkdownHeadingTests {
    @Test func aHeadingNeedsASpaceAfterItsHashes() {
        #expect(Markdown.parse("#128 is merged") == [p("#128 is merged")])
        #expect(Markdown.parse("#hashtag\n#another") == [p("#hashtag #another")])
        #expect(Markdown.parse("# Title") == [.heading(1, "Title")])
        #expect(Markdown.parse("### Title ###") == [.heading(3, "Title")])
        #expect(Markdown.parse("## C# and F#") == [.heading(2, "C# and F#")])
        #expect(Markdown.parse("####### seven") == [p("####### seven")])
        #expect(Markdown.parse("Text\n## Heading\nMore") == [p("Text"), .heading(2, "Heading"), p("More")])
        #expect(Markdown.parse("#") == [], "nothing to show yet")
    }
}

struct MarkdownPictureTests {
    @Test func aPictureOnTheLineAfterTextIsDrawnUnderIt() {
        // Only its alt text showed unless a blank line came before the picture.
        #expect(Markdown.parse("The page after the change:\n![Settings](docs/after.png)")
            == [p("The page after the change:"), .images([picture("Settings", "docs/after.png")])])
        #expect(Markdown.parse("One line,\nthen another:\n![a](a.png) ![b](b.jpg)")
            == [p("One line, then another:"), .images([picture("a", "a.png"), picture("b", "b.jpg")])])
        #expect(Markdown.parse("![shot](a.png)") == [.images([picture("shot", "a.png")])])
        #expect(Markdown.parse("![a](a.png) ![b](b.jpg)") == [.images([picture("a", "a.png"), picture("b", "b.jpg")])])
    }

    @Test func aPictureInAListItemOrAQuoteIsDrawnInIt() {
        #expect(Markdown.parse("- ![list](list.png)\n- The rest")
            == [bullets(item(.images([picture("list", "list.png")])), item(p("The rest")))])
        #expect(Markdown.parse("> ![q](q.png)") == [.quote([.images([picture("q", "q.png")])])])
    }

    @Test func whatIsntAPictureStaysText() {
        #expect(Markdown.parse("See ![notes](notes.txt) here.") == [p("See ![notes](notes.txt) here.")])
        #expect(Markdown.parse("Write `![alt](a.png)` for a picture.") == [p("Write `![alt](a.png)` for a picture.")])
    }
}

struct MarkdownRegressionTests {
    @Test func tablesStillRead() {
        let blocks = Markdown.parse("Numbers:\n\n| Name | Count |\n|:--|--:|\n| a | 1 |\n| b `x|y` | 2 | extra |\n\nAfter.")
        #expect(blocks == [p("Numbers:"),
                           .table(header: ["Name", "Count"], align: [.leading, .trailing], rows: [["a", "1"], ["b `x|y`", "2"]]),
                           p("After.")])
        #expect(Markdown.parse("| not | a table |") == [p("| not | a table |")])
        #expect(Markdown.parse("- in a list:\n\n  | A |\n  |---|\n  | 1 |")
            == [bullets(loose: true, item(p("in a list:"), .table(header: ["A"], align: [.leading], rows: [["1"]])))])
    }

    @Test func mathsStillReads() {
        #expect(Markdown.parse("Then:\n\n$$\nx = 1\n$$\n\nSo.") == [p("Then:"), .math(tex: "x = 1", source: "$$x = 1$$"), p("So.")])
        #expect(Markdown.parse("We get $$a+b$$ here.") == [p("We get"), .math(tex: "a+b", source: "$$a+b$$"), p("here.")])
        // A formula's own lines aren't read as Markdown: `|x|` isn't a table, `- y` isn't a list.
        #expect(Markdown.parse("$$\n|x| = 1\n- y\n$$").count == 1)
        #expect(Markdown.parse("It costs $5 and $10.") == [p("It costs $5 and $10.")])
    }

    @Test func paragraphsHeadingsAndCodeStillRead() {
        let text = "# Title\n\nFirst line\nsecond line.\n\n## Part\n\n```swift\nlet x = 1\n```\n\n- a\n- b\n\n1. one\n2. two"
        #expect(Markdown.parse(text) == [.heading(1, "Title"), p("First line second line."), .heading(2, "Part"), code("swift", "let x = 1"),
                                         bullets(item(p("a")), item(p("b"))), numbered(item(p("one")), item(p("two")))])
        #expect(Markdown.parse("") == [])
        #expect(Markdown.parse("\n\n  \n") == [])
        #expect(Markdown.parse("one\r\n\r\ntwo\r\n") == [p("one"), p("two")])
    }

    @Test func picturesAndVideosOnLinesOfTheirOwnStillRead() {
        let blocks = Markdown.parse("Here:\n\n![shot](a.png)\n\n[clip](b.mov)\n\nDone.")
        #expect(blocks == [p("Here:"), .images([picture("shot", "a.png")]), .video(MediaLink(name: "clip", link: "b.mov")), p("Done.")])
    }

    @Test func inlinesOfNestedBlocksAreFoundForParsingAhead() {
        let blocks = Markdown.parse("# H\n\n- a\n  - **b**\n\n> q\n\n| t |\n|---|\n| c |\n\n```\ncode\n```")
        #expect(blocks.flatMap(\.inlines) == ["H", "a", "**b**", "q", "t", "c"])
    }
}

struct MarkdownStreamingTests {
    /// A long answer with everything in it, as agents write on the Mac.
    static let rich = """
    ---
    **Done**
    The session cache survives a restart: it's written to `cache/sessions.bin`.

    **Still to do** Nothing.

    **Found**
    #128 is merged, so the old flag can go.

    ## What changed

    Steps 1 and 2 were done in [the first thread](trek://thread/t-parity). The rest:

    3. Write the cache on shutdown.

       - only entries read in the last hour
       - at most `10_000` of them
         - oldest dropped first

    4. Read it back on boot:

       ```rust
       let cache = SessionCache::load(&path).unwrap_or_default();

       cache.prune();
       ```

    5. Drop an entry whose rotation counter moved.

    Left for the release:

    - [x] Tests for a rotated token
    - [ ] A note in `CHANGELOG.md`

    > A cold cache after a deploy sent every request to Postgres at once.
    > p99 spiked for about a minute.

    Timing
    ------

    | Step | Before | After |
    |:--|--:|--:|
    | p50 | 3.1 ms | 0.4 ms |
    | p99 | 40 ms | 2 ms |

    The saving is

    $$
    \\frac{3.1 - 0.4}{3.1} \\approx 87\\%
    $$

    of each read, or \\(2.7\\) ms. In the docs:

    ````markdown
    Reload with:

    ```sh
    trek cache reload
    ```
    ````

    ~~~text
    $$ not maths $$

    - not a list
    ~~~

    ```trek-viz
    {"version":1,"title":"Reads","summary":"p50 by build","type":"bar","bars":[{"label":"before","value":3.1},{"label":"after","value":0.4}]}
    ```

    1. one
    2. two

    ```sh
    make
    ```

    3. three

    The page after the change:
    ![Settings, after](docs/screens/settings-after.png)

    ***

    Costs $5 a month, not $10.
    """

    @Test func theCacheCarriesAStreamingAnswerOn() {
        let tag = UUID().uuidString
        var text = "# Answer \(tag)\n\n"
        for n in 0..<40 {
            text += "Paragraph \(n) of the answer.\n\n- item \(n)\n\n"
            #expect(Markdown.blocks(text) == Markdown.parse(text))
        }
        #expect(Markdown.isCached(text))
    }

    @Test func answersAreParsedAheadOffTheMainThread() async throws {
        let tag = UUID().uuidString
        let short = "Short answer \(tag) with `code`."
        let long = "# Long \(tag)\n\n" + (0..<80).map { "Paragraph \($0) of a long answer, with **bold** in it.\n\n" }.joined()
        #expect(long.utf8.count >= AnswerPart.threshold)
        Markdown.prewarm([short, long])
        let parts = AnswerPart.cut(long)
        #expect(parts.count > 1)
        for _ in 0..<200 where !(Markdown.isCached(short) && parts.allSatisfy(Markdown.isCached)) {
            try await Task.sleep(for: .milliseconds(25))
        }
        #expect(Markdown.isCached(short))
        #expect(parts.allSatisfy(Markdown.isCached), "a long answer is parsed in the parts it's drawn in")
        #expect(Inline.isParsedCached(short))
    }

    @Test func longAnswersAreCutOutsideFencesOfAnyLength() {
        // The cut took any ``` line for a fence's end, so it could fall inside a four-backtick block.
        let filler = String(repeating: "A line of the answer that fills it up.\n", count: 40)
        let text = filler + "\n````markdown\n```sh\nmake\n```\n\n" + filler + "\n````\n\nAfter the block.\n\n" + filler
        let parts = AnswerPart.cut(text)
        #expect(parts.count > 1)
        #expect(parts.flatMap { Markdown.parse($0) } == Markdown.parse(text))
        for part in parts {
            #expect(part.components(separatedBy: "````").count % 2 == 1, "no part ends inside the block")
        }
    }
}

struct LivePageBlockTests {
    @Test func aBlockStillOpenIsToldFromAWholeOne() {
        // The page runs only once its fence has closed: `closed` rides with the block.
        #expect(Markdown.parse("```trek-html\n<p>half") == [.code(lang: "trek-html", code: "<p>half", closed: false)])
        #expect(Markdown.parse("```trek-html\n<p>whole</p>\n```") == [.code(lang: "trek-html", code: "<p>whole</p>", closed: true)])
    }

    @Test func aTutorsQuestionIsKnownByItsFence() {
        // `study::is_question_fence`.
        for info in ["trek-study", "Trek-Study", "trek_study", "trekstudy", "trek-study json", "{.trek-study}"] {
            #expect(Markdown.isStudy(info), "\(info)")
        }
        #expect(!Markdown.isStudy("json"))
        #expect(!Markdown.isStudy("trek-html"))
        #expect(Markdown.isArtifact("trek-html"))
        #expect(LivePageBuilding.amount(512) == "512 bytes" && LivePageBuilding.amount(2048) == "2 KB")
    }
}

@MainActor
struct ThreadLinkTests {
    @Test func aThreadLinkNamesItsThread() {
        // As the Mac writes them (`thread_menu.rs`) and reads them (`deep_link::parse`).
        #expect(TrekLink.thread("trek://thread/1c0d8a52-77a0-4d0a-b0a3-0d2f1f0b9f11") == "1c0d8a52-77a0-4d0a-b0a3-0d2f1f0b9f11")
        #expect(TrekLink.thread("trek://thread/t-parity?from=answer") == "t-parity")
        #expect(TrekLink.thread("trek://thread/a%20b") == "a b")
        #expect(TrekLink.thread("TREK://thread/t-1") == "t-1")
        #expect(TrekLink.thread("trek://thread/") == nil)
        #expect(TrekLink.thread("trek://open?thread=t-1") == nil)
        #expect(TrekLink.thread("trek://pair?host=1.2.3.4:7420&code=K7Q2-9XMV") == nil)
        #expect(TrekLink.thread("https://example.com/thread/t-1") == nil)
    }

    @Test func itOpensAThreadThisIPhoneLists() {
        let model = AppModel()
        model.links = [HostLink(paired: .demo, backend: MockHost()),
                       HostLink(paired: .demoPC, backend: MockHost(name: "T-Book", id: "demo-pc", platform: "windows"))]
        func thread(_ id: String) -> ThreadSummary {
            ThreadSummary(id: id, title: id, project: nil, agent: AgentRef(key: "claude-code", name: "Claude Code"), model: nil, modelLabel: nil,
                          runState: .idle, needs: nil, section: .inbox, unseen: false, pinned: false, branch: nil, worktree: false,
                          activity: nil, workingSince: nil, updatedAt: 0, additions: 0, deletions: 0)
        }
        model.threads = [thread("demo:t-1"), thread("demo-pc:t-2")]
        #expect(model.listedThread("t-1") == "demo:t-1")
        #expect(model.listedThread("t-2") == "demo-pc:t-2", "on whichever machine has it")
        #expect(model.listedThread("t-3") == nil)

        #expect(model.open(link: URL(string: "trek://thread/t-2")!))
        #expect(model.openRequest == "demo-pc:t-2")
        model.openRequest = nil
        #expect(model.open(link: URL(string: "trek://thread/t-3")!))
        #expect(model.openRequest == nil)
        #expect(model.toast?.text == "That thread isn't on this iPhone's list")
        #expect(!model.open(link: URL(string: "trek://pair?host=1.2.3.4:7420&code=K7Q2-9XMV")!), "a pairing link is someone else's to read")
    }

    @Test func aNoticesMarksAreRead() {
        // The notice row draws its text as inline Markdown: no asterisks around "Compacted".
        let parsed = Inline.parsed("**Compacted** the conversation so far.")
        #expect(String(parsed.characters) == "Compacted the conversation so far.")
        #expect(parsed.runs.first?.inlinePresentationIntent?.contains(.stronglyEmphasized) == true)
    }
}
