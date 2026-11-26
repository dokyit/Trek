import Foundation
import Testing
@testable import Trek

// A `trek-viz` block the Mac draws is drawn on the phone: the Mac's lenient reading (`tidy`,
// `normalize`, the alias tables in `trek_core::visualization`), with its tests' cases.

private func common(_ kind: String, _ body: String) -> String {
    "{\"version\":1,\"title\":\"Demo\",\"summary\":\"Useful summary\",\"type\":\"\(kind)\",\(body)}"
}

private func parse(_ payload: String) -> Result<Visualization, VizError> {
    do throws(VizError) { return .success(try VizParse.parse(payload)) } catch { return .failure(error) }
}

private func viz(_ payload: String, _ comment: Comment? = nil, at: SourceLocation = #_sourceLocation) -> Visualization? {
    switch parse(payload) {
    case .success(let v): return v
    case .failure(let e):
        Issue.record("\(comment.map { "\($0): " } ?? "")\(e.reason) for \(payload)", sourceLocation: at)
        return nil
    }
}

private func reason(_ payload: String) -> String? {
    if case .failure(let e) = parse(payload) { return e.reason }
    return nil
}

struct VisualizationLenientTests {
    @Test func strictBlocksStillParse() {
        let bar = viz(common("bar", "\"bars\":[{\"label\":\"Build\",\"value\":42,\"tone\":\"positive\"}],\"y_label\":\"seconds\",\"unit\":\"s\""))
        #expect(bar?.content == .bar(bars: [VizMark(label: "Build", value: 42, tone: .positive)], xLabel: nil, yLabel: "seconds", unit: "s"))
        let heatmap = viz(common("heatmap", "\"x_labels\":[\"Mon\",\"Tue\"],\"y_labels\":[\"API\"],\"values\":[[2,4]]"))
        #expect(heatmap?.content == .heatmap(xLabels: ["Mon", "Tue"], yLabels: ["API"], values: [[2, 4]]))
        #expect(viz(common("mockup", "\"nodes\":[{\"kind\":\"frame\",\"device\":\"phone\",\"children\":[{\"kind\":\"text\",\"text\":\"Hi\"}]}]")) != nil)
    }

    @Test func wrongVersionsAndUnknownFieldsAreStillRefused() {
        let version = common("bar", "\"bars\":[{\"label\":\"x\",\"value\":1}]").replacingOccurrences(of: "\"version\":1", with: "\"version\":2")
        #expect(parse(version) == .failure(.version(2)))
        #expect(reason(common("bar", "\"bars\":[{\"label\":\"x\",\"value\":1}],\"html\":\"<b>\""))
            == "unknown field `html` (a bar takes `bars`, `x_label`, `y_label`, `unit`)")
        #expect(reason(common("sparkles", "\"bars\":[]")) == "unsupported visualization type `sparkles`")
        #expect(reason("[1]") == "top level must be an object")
        #expect(reason("{\"title\":\"A\"") != nil)
        #expect(reason(common("bar", "\"bars\":[]")) == "bar dataset must be nonempty")
    }

    @Test func theMinutesTimelineAUserHitStillParses() {
        // The Mac's regression: this exact payload once read "invalid type: string \"minutes\"".
        let payload = "{\"version\":1,\"title\":\"Where the minutes go\",\"summary\":\"Build, CI, and release all pay the same two costs.\",\"type\":\"timeline\",\"events\":[{\"label\":\"Local dev build\",\"start\":0,\"end\":2,\"lane\":\"Local\"},{\"label\":\"cargo test\",\"start\":2,\"end\":4,\"lane\":\"Local\"},{\"label\":\"ci.yml macOS job\",\"start\":0,\"end\":40,\"lane\":\"CI\"},{\"label\":\"ci.yml Windows job\",\"start\":0,\"end\":75,\"lane\":\"CI\"},{\"label\":\"Release\",\"start\":0,\"end\":42,\"lane\":\"Release\"}],\"ticks\":\"minutes\"}"
        guard case .timeline(let events, let ticks, let unit)? = viz(payload)?.content else { return }
        #expect(unit == "minutes")
        #expect(ticks == nil)
        #expect(events.count == 5)
        #expect(events.compactMap(\.lane) == ["Local", "Local", "CI", "CI", "Release"])
    }

    @Test func timelineStringTicksAreANumericAxisUnit() {
        let payload = "{\"version\":1,\"title\":\"Plan\",\"summary\":\"Duration\",\"type\":\"timeline\",\"events\":[{\"label\":\"Build\",\"start\":0,\"end\":5},{\"label\":\"Test\",\"start\":5,\"end\":12}],\"ticks\":\"minutes\"}"
        let whole = viz(payload)
        guard case .timeline(let events, let ticks, let unit)? = whole?.content else { return }
        #expect(unit == "minutes")
        #expect(ticks == nil)
        #expect(events[1].end == .number(12))
        #expect(VizParse.partial(payload).drawable == whole)
    }

    @Test func singletonSequencesAndSingularKeysAreSchemaAware() {
        for (kind, body) in [
            ("bar", "\"bar\":{\"label\":\"one\",\"value\":1}"),
            ("timeline", "\"event\":{\"label\":\"one\",\"start\":1}"),
            ("donut", "\"part\":{\"label\":\"one\",\"value\":1}"),
            ("stats", "\"tile\":{\"label\":\"one\",\"value\":\"1\"}"),
            ("treemap", "\"item\":{\"label\":\"one\",\"weight\":1}"),
            ("layers", "\"layer\":{\"label\":\"one\",\"item\":\"API\"}"),
            ("mockup", "\"node\":{\"kind\":\"card\",\"child\":{\"kind\":\"text\",\"text\":\"Hi\"}}"),
            ("table", "\"column\":\"A\",\"row\":[[1]]"),
            ("flow", "\"node\":{\"id\":\"a\",\"label\":\"A\"}"),
            ("line", "\"x_labels\":[\"a\",\"b\"],\"series\":{\"label\":\"A\",\"value\":[1,2]}"),
        ] {
            _ = viz(common(kind, body), "\(kind)")
        }
        // A string sequence is a single label, not arbitrarily split on spaces or commas.
        guard case .layers(let layers)? = viz(common("layers", "\"layers\":[{\"label\":\"L\",\"items\":\"A, B\"}]"))?.content else { return }
        #expect(layers[0].items == ["A, B"])
    }

    @Test func textArraysJoinAndNumbersBecomeTextOnlyInTextFields() {
        let v = viz("{\"version\":\"1\",\"title\":[\"Build\",\"time\"],\"summary\":42,\"type\":\"stats\",\"tiles\":[{\"label\":2026,\"value\":12.5,\"delta\":[\"+1\",\"today\"]}]}")
        #expect(v?.title == "Build time")
        #expect(v?.summary == "42")
        guard case .stats(let tiles)? = v?.content else { return }
        #expect(tiles[0].label == "2026")
        #expect(tiles[0].value == "12.5")
        #expect(tiles[0].delta == "+1 today")
    }

    @Test func numericStringsAreCoercedWithoutChangingOrdinalTicksOrTableCells() {
        if case .bar(let bars, _, _, _)? = viz(common("bar", "\"bars\":[{\"label\":\"A\",\"value\":\"1.25e2\"}]"))?.content {
            #expect(bars[0].value == 125)
        }
        if case .line(let xs, let series, _, _, _)? = viz(common("line", "\"x_labels\":[1,2],\"series\":[{\"label\":\"A\",\"values\":[\"1\",\"2\"]}]"))?.content {
            #expect(xs == ["1", "2"])
            #expect(series[0].values == [1, 2])
        }
        if case .timeline(let events, _, _)? = viz(common("timeline", "\"events\":[{\"label\":\"A\",\"start\":\"0\",\"end\":\"12.5\"}],\"ticks\":\"minutes\""))?.content {
            #expect(events[0].start == .number(0))
            #expect(events[0].end == .number(12.5))
        }
        if case .timeline(let events, _, _)? = viz(common("timeline", "\"ticks\":[\"2025\",\"2026\"],\"events\":[{\"label\":\"A\",\"start\":\"2025\",\"end\":2026}]"))?.content {
            #expect(events[0].start == .tick("2025"))
            #expect(events[0].end == .tick("2026"))
        }
        if case .table(_, let rows)? = viz(common("table", "\"columns\":[\"ID\"],\"rows\":[[\"001\"]]"))?.content {
            #expect(rows[0].cells == [.text("001")])
        }
    }

    @Test func closeTypeAliasesHaveExplicitMeanings() {
        for (alias, canonical) in VizLenient.typeAliases {
            let body = switch canonical {
            case "bar": "\"bars\":[{\"label\":\"A\",\"value\":1}]"
            case "line": "\"x_labels\":[\"a\",\"b\"],\"series\":[{\"label\":\"A\",\"values\":[1,2]}]"
            case "donut": "\"parts\":[{\"label\":\"A\",\"value\":1}]"
            case "stats": "\"tiles\":[{\"label\":\"A\",\"value\":\"1\"}]"
            case "heatmap": "\"x_labels\":[\"a\"],\"y_labels\":[\"b\"],\"values\":[[1]]"
            case "treemap": "\"items\":[{\"label\":\"A\",\"weight\":1}]"
            case "timeline": "\"events\":[{\"label\":\"A\",\"start\":1}]"
            case "flow": "\"nodes\":[{\"id\":\"a\",\"label\":\"A\"}]"
            case "layers": "\"layers\":[{\"label\":\"A\"}]"
            default: "\"nodes\":[{\"kind\":\"divider\"}]"
            }
            let v = viz(common(alias, body), "\(alias)")
            #expect(v?.kind == canonical, "\(alias)")
            if alias == "area", case .line(_, _, let area, _, _)? = v?.content { #expect(area) }
        }
        #expect(viz(common("Bar Chart", "\"bars\":[{\"label\":\"A\",\"value\":1}]"))?.kind == "bar")
        #expect(viz(common("pie", "\"parts\":[{\"label\":\"A\",\"value\":1}]"))?.kind == "donut")
    }

    @Test func semanticColourSpellingsAreToneAliasesNotCSS() {
        for spelling in ["color", "colour"] {
            if case .bar(let bars, _, _, _)? = viz(common("bar", "\"bars\":[{\"label\":\"A\",\"value\":1,\"\(spelling)\":\"positive\"}]"))?.content {
                #expect(bars[0].tone == .positive)
            }
        }
        _ = viz(common("mockup", "\"nodes\":[{\"kind\":\"badge\",\"text\":\"OK\",\"colour\":\"positive\"}]"))
        _ = viz(common("table", "\"columns\":[\"a\"],\"rows\":[{\"cells\":[1],\"color\":\"info\"}]"))
        #expect(reason(common("bar", "\"bars\":[{\"label\":\"A\",\"value\":1,\"colour\":\"#f00\"}]"))?.contains("unknown field") == true)
    }

    @Test func missingMetadataIsInferredOnlyFromSuppliedContent() {
        let v = viz("{\"title\":\"Plan\",\"event\":{\"label\":\"A\",\"start\":0}}")
        #expect(v?.summary == "Plan")
        #expect(v?.kind == "timeline")
        #expect(viz("{\"summary\":\"Build\",\"bars\":[{\"label\":\"A\",\"value\":1}]}")?.title == "Build")
        if case .flow(let nodes, _)? = viz(common("flow", "\"nodes\":[{\"id\":\"api\"}]"))?.content { #expect(nodes[0].label == "api") }
        #expect(reason("{\"bars\":[{\"label\":\"A\",\"value\":1}]}") != nil, "no invented title")
        #expect(reason("{\"title\":\"A\",\"nodes\":[{\"id\":\"api\"}]}") != nil, "ambiguous type")
    }

    @Test func ambiguousCoercionsAndConflictingAliasesStillFail() {
        for (kind, body) in [
            ("timeline", "\"ticks\":\"minutes\",\"events\":[{\"label\":\"A\",\"start\":\"Monday\"}]"),
            ("timeline", "\"ticks\":\"minutes\",\"unit\":\"hours\",\"events\":[{\"label\":\"A\",\"start\":0}]"),
            ("bar", "\"bar\":[{\"label\":\"A\",\"value\":1}],\"bars\":[{\"label\":\"B\",\"value\":2}]"),
            // "1,2,3" isn't grouped thousands; it stays a string and can't be a value.
            ("bar", "\"bars\":[{\"label\":\"A\",\"value\":\"1,2,3\"}]"),
            ("bar", "\"bars\":[{\"label\":\"A\",\"value\":\"NaN\"}]"),
            ("bar", "\"bars\":[{\"label\":\"A\",\"value\":1,\"color\":\"positive\",\"tone\":\"negative\"}]"),
            ("line", "\"x_labels\":[[\"a\",\"b\"],\"c\"],\"series\":[{\"label\":\"A\",\"values\":[1,2]}]"),
        ] {
            #expect(reason(common(kind, body)) != nil, "\(kind): \(body)")
        }
        #expect(reason(common("area", "\"x_labels\":[\"a\",\"b\"],\"series\":[{\"label\":\"A\",\"values\":[1,2]}],\"area\":false"))?.contains("conflicts") == true)
    }

    @Test func realisticModelSlipsParseOrFailClearly() {
        let ok: [(String, String)] = [
            // Synonym and wrong-case keys; numbers dressed with units and commas.
            ("bar", "\"data\":[{\"name\":\"Build\",\"value\":\"1,200\"},{\"x\":\"Test\",\"y\":\"45%\"}]"),
            ("bar", "\"Bars\":[{\"Label\":\"A\",\"Value\":\"$3.2k\"}]"),
            ("donut", "\"slices\":[{\"name\":\"Rust\",\"value\":\"62%\"},{\"name\":\"Other\",\"value\":38}],\"units\":\"%\""),
            ("line", "\"labels\":[\"Mon\",\"Tue\"],\"data\":{\"label\":\"p95\",\"values\":[\"1,000\",\"2.5k\"]},\"y_axis\":\"ms\""),
            ("stats", "\"metrics\":[{\"name\":\"Uptime\",\"value\":\"99.9%\",\"change\":\"+0.1\",\"data\":[\"1\",\"2\",\"3\"]}]"),
            ("stats", "\"kpis\":{\"label\":\"One\",\"value\":1}"),
            ("table", "\"headers\":[\"Service\",\"p95\"],\"data\":[[\"API\",\"1,200\"],{\"cells\":[\"DB\",42],\"color\":\"green\"}]"),
            ("table", "\"columns\":[\"A\"],\"rows\":[{\"data\":[\"x\"],\"tone\":\"red\"}]"),
            ("heatmap", "\"x_labels\":[\"Mon\",\"Tue\"],\"y_labels\":[\"AM\",\"PM\"],\"data\":[[\"1\",\"2\"],[\"3\",\"4\"]]"),
            // A matrix that runs one row per x label instead: transposed.
            ("heatmap", "\"x_labels\":[\"Mon\",\"Tue\"],\"y_labels\":[\"AM\",\"PM\",\"Mid\"],\"values\":[[1,2,3],[4,5,6]]"),
            // A flat row-major list that exactly fills the grid.
            ("heatmap", "\"x_labels\":[\"A\",\"B\"],\"y_labels\":[\"C\",\"D\"],\"values\":[1,2,3,4]"),
            ("treemap", "\"data\":[{\"name\":\"Core\",\"size\":\"2k\"},{\"label\":\"UI\",\"value\":3}]"),
            // Date-string endpoints, no ticks: ordinal ticks, sorted into date order.
            ("timeline", "\"phases\":[{\"title\":\"Ship\",\"from\":\"2026-08\",\"to\":\"2026-11\"},{\"title\":\"Design\",\"from\":\"2026-01\",\"until\":\"2026-03\"}]"),
            ("timeline", "\"events\":[{\"label\":\"Ship\",\"start\":\"Mar\",\"end\":\"May\",\"track\":\"Ops\",\"tone\":\"green\"}]"),
            // Nulls in optional fields read as "not given".
            ("timeline", "\"events\":[{\"label\":\"Ship\",\"start\":1,\"end\":2,\"lane\":null,\"tone\":null}]"),
            ("flow", "\"nodes\":[{\"id\":\"a\",\"name\":\"API\"},{\"id\":\"b\",\"label\":\"DB\"}],\"links\":[{\"source\":\"a\",\"target\":\"b\",\"text\":\"reads\"}]"),
            ("layers", "\"tiers\":[{\"name\":\"UI\",\"contents\":[\"Sidebar\",\"Transcript\"]},{\"name\":\"Core\"}]"),
            // `type` for `kind`, nearest kinds, a boolean toggle, an inferred leaf.
            ("mockup", "\"nodes\":[{\"type\":\"panel\",\"children\":[{\"kind\":\"Heading\",\"text\":\"Hi\"},{\"kind\":\"switch\",\"text\":\"On\",\"value\":true},{\"kind\":\"spinner\",\"text\":\"Load\",\"value\":\"30%\"}]}]"),
            ("mockup", "\"nodes\":[{\"text\":\"a bare leaf\"}]"),
            ("mockup", "\"nodes\":[{\"kind\":\"frame\",\"device\":\"iphone\"}]"),
        ]
        for (kind, body) in ok { _ = viz(common(kind, body), "\(kind)") }
        // Type aliases reach the same shapes.
        for (alias, body) in [
            ("pie", "\"parts\":[{\"label\":\"A\",\"value\":1}]"),
            ("gantt", "\"events\":[{\"label\":\"A\",\"start\":0}]"),
            ("kpi", "\"tiles\":[{\"label\":\"A\",\"value\":\"1\"}]"),
            ("metrics", "\"tiles\":[{\"label\":\"A\",\"value\":\"1\"}]"),
            ("diagram", "\"nodes\":[{\"id\":\"a\",\"label\":\"A\"}]"),
            ("graph", "\"nodes\":[{\"id\":\"a\",\"label\":\"A\"}]"),
            ("stack", "\"layers\":[{\"label\":\"A\"}]"),
        ] {
            let v = viz(common(alias, body), "\(alias)")
            #expect(v.map { VizSchema.types.contains($0.kind) } == true, "\(alias)")
        }
        // What must stay an error is still short and specific.
        let errors: [(String, String, String)] = [
            // `data` on a flow could be its nodes or its edges.
            ("flow", "\"data\":[{\"id\":\"a\",\"label\":\"A\"}]", "unknown field `data`"),
            // Two different readings of the same list.
            ("bar", "\"data\":[{\"label\":\"A\",\"value\":1}],\"bars\":[{\"label\":\"A\",\"value\":2}]", "conflicting"),
            // A unit word on `ticks` can't be both a unit and a label when an endpoint isn't a number.
            ("timeline", "\"ticks\":\"minutes\",\"events\":[{\"label\":\"A\",\"start\":\"Monday\"}]", "must be an array of axis labels"),
            // A hex colour is presentation, not a tone.
            ("bar", "\"bars\":[{\"label\":\"A\",\"value\":1,\"tone\":\"#fff\"}]", "`tone` must be one of"),
            ("bar", "\"bars\":[{\"label\":\"A\",\"value\":1,\"color\":\"chartreuse\"}]", "unknown field `color`"),
            ("mockup", "\"nodes\":[{\"kind\":\"carousel\"}]", "isn't a mockup kind"),
            ("mockup", "\"nodes\":[{\"kind\":\"frame\",\"device\":\"watch\"}]", "isn't a mockup device"),
            ("bar", "\"bars\":[{\"label\":\"A\",\"value\":1}],\"footnote\":\"x\"", "unknown field `footnote`"),
            // A number written as a list doesn't fit a value slot.
            ("bar", "\"bars\":[{\"label\":\"A\",\"values\":[1,2]}]", "unknown field `values`"),
        ]
        for (kind, body, needle) in errors {
            guard let error = reason(common(kind, body)) else {
                Issue.record("\(kind): \(body) should be refused")
                continue
            }
            #expect(error.contains(needle), "\(kind): \(body) -> \(error)")
            #expect(error.count < 160, "\(kind): \(body) -> \(error)")
        }
    }

    @Test func slipsAreReadToTheValuesMeant() {
        if case .bar(let bars, _, _, _)? = viz(common("bar", "\"data\":[{\"name\":\"Build\",\"value\":\"1,200\"},{\"x\":\"Test\",\"y\":\"45%\"}]"))?.content {
            #expect(bars == [VizMark(label: "Build", value: 1200, tone: nil), VizMark(label: "Test", value: 45, tone: nil)])
        }
        if case .bar(let bars, _, _, _)? = viz(common("bar", "\"Bars\":[{\"Label\":\"A\",\"Value\":\"$3.2k\"}]"))?.content {
            #expect(bars == [VizMark(label: "A", value: 3200, tone: nil)])
        }
        if case .heatmap(_, _, let values)? = viz(common("heatmap", "\"x_labels\":[\"Mon\",\"Tue\"],\"y_labels\":[\"AM\",\"PM\",\"Mid\"],\"values\":[[1,2,3],[4,5,6]]"))?.content {
            #expect(values == [[1, 4], [2, 5], [3, 6]])
        }
        if case .heatmap(_, _, let values)? = viz(common("heatmap", "\"x_labels\":[\"A\",\"B\"],\"y_labels\":[\"C\",\"D\"],\"values\":[1,2,3,4]"))?.content {
            #expect(values == [[1, 2], [3, 4]])
        }
        if case .mockup(let nodes)? = viz(common("mockup", "\"nodes\":[{\"type\":\"panel\",\"children\":[{\"kind\":\"Heading\",\"text\":\"Hi\"},{\"kind\":\"switch\",\"text\":\"On\",\"value\":true}]}]"))?.content {
            #expect(nodes[0].kind == .card)
            #expect(nodes[0].children.map(\.kind) == [.text, .toggle])
            #expect(nodes[0].children[1].value == "on")
        }
        #expect(VizLenient.looseNumber("1,2,3") == nil)
        #expect(VizLenient.looseNumber("m") == nil)
        #expect(VizLenient.looseNumber("€ 2.5M") == .float(2_500_000))
        #expect(VizLenient.looseNumber("-1,234.5") == .float(-1234.5))
    }

    @Test func dateStringEndpointsBecomeOrdinalTicksInOrder() {
        if case .timeline(let events, let ticks, _)? = viz(common("timeline", "\"events\":[{\"label\":\"Build\",\"start\":\"2026-04\",\"end\":\"2026-08\"},{\"label\":\"Design\",\"start\":\"2026-01\",\"end\":\"2026-04\"},{\"label\":\"Ship\",\"start\":\"2026-10\"}]"))?.content {
            #expect(ticks == ["2026-01", "2026-04", "2026-08", "2026-10"])
            #expect(events[1].end == .tick("2026-04"))
        }
        // Months and quarters sort in their own order too; anything else keeps the order the
        // events arrived in.
        if case .timeline(_, let ticks, _)? = viz(common("timeline", "\"events\":[{\"label\":\"B\",\"start\":\"March\",\"end\":\"May\"},{\"label\":\"A\",\"start\":\"January\",\"end\":\"March\"}]"))?.content {
            #expect(ticks == ["January", "March", "May"])
        }
        if case .timeline(_, let ticks, _)? = viz(common("timeline", "\"events\":[{\"label\":\"A\",\"start\":\"Q3 2026\",\"end\":\"Q1 2027\"},{\"label\":\"B\",\"start\":\"Q2 2026\"}]"))?.content {
            #expect(ticks == ["Q2 2026", "Q3 2026", "Q1 2027"])
        }
        if case .timeline(_, let ticks, _)? = viz(common("timeline", "\"events\":[{\"label\":\"A\",\"start\":\"alpha\",\"end\":\"beta\"},{\"label\":\"B\",\"start\":\"gamma\"}]"))?.content {
            #expect(ticks == ["alpha", "beta", "gamma"])
        }
        // But the derived axis can't rescue an event that ends before it starts.
        #expect(reason(common("timeline", "\"events\":[{\"label\":\"A\",\"start\":\"May\",\"end\":\"Jan\"}]"))?.contains("ends before it starts") == true)
    }

    @Test func json5ishNoiseAndASecondFenceAreStripped() {
        let body = """
        {
            // the build, by phase
            "version": 1,
            "title": "Build",
            "summary": "Where it went",
            "type": "bar",
            "bars": [
                {"label": "compile", "value": 3,},
                {"label": "link", "value": 1}, /* tail */
            ],
        }
        """
        if case .bar(let bars, _, _, _)? = viz(body)?.content { #expect(bars.count == 2) }
        // A JSON fence inside the trek-viz fence unwraps; deeper nesting still fails.
        if case .drawn = VizBlock.make(payload: "```json\n\(body)\n```", closed: true) {} else { Issue.record("an inner fence unwraps") }
        if case .invalid(let reason, _, _, _) = VizBlock.make(payload: "```a\n```b\n{}\n```\n```", closed: true) {
            #expect(reason == "it isn't one ```trek-viz block with JSON inside")
        } else {
            Issue.record("deeper fences are refused")
        }
        // A comment-looking token inside a string is text, not a comment.
        _ = viz(common("bar", "\"bars\":[{\"label\":\"https://x\",\"value\":1}]"))
        #expect(VizLenient.tidy("{\"a\":\"x, }\",}") == "{\"a\":\"x, }\"}")
        #expect(VizLenient.tidy("[1, /* a */ 2, // b\n]") == "[1,  2\n]")
    }

    @Test func aBlockThatCantBeDrawnKeepsItsTitleAndSummary() {
        // An unknown type: the answer still says what it meant to show.
        let payload = "{\"version\":1,\"title\":\"Reads by build\",\"summary\":\"p50 fell by 87%\",\"type\":\"radar\",\"axes\":[]}"
        guard case .invalid(let reason, _, let title, let summary) = VizBlock.make(payload: payload, closed: true) else {
            Issue.record("a radar isn't drawn")
            return
        }
        #expect(reason == "unsupported visualization type `radar`")
        #expect(title == "Reads by build")
        #expect(summary == "p50 fell by 87%")
        let named = VizLenient.declaredMetadata("{\"name\":[\"Reads\",\"by build\"],\"description\":42,}")
        #expect(named.title == "Reads by build" && named.summary == "42")
        let none = VizLenient.declaredMetadata("not json")
        #expect(none.title == nil && none.summary == nil)
    }

    @Test func aBlockStillArrivingDrawsItsWholeMarksLeniently() {
        // The type an alias, the version missing: what's written so far still draws.
        let partial = VizParse.partial("{\"title\":\"Share\",\"summary\":\"By language\",\"type\":\"pie\",\"slices\":[{\"name\":\"Rust\",\"value\":\"62%\"},{\"name\":\"Oth")
        #expect(partial.kind == "pie")
        #expect(partial.title == "Share")
        #expect(partial.drawable?.content == .donut(parts: [VizMark(label: "Rust", value: 62, tone: nil)], unit: nil))
        #expect(!partial.complete)
    }
}
