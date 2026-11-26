import Foundation

// The lenient reading of a `trek-viz` block, as the Mac does it (`trek_core::visualization`: `tidy`,
// `normalize` and their alias tables, ported line for line). A block is tidied of JSON5-ish noise,
// read as JSON, then its schema-known fields are folded to their spellings and shapes; after that
// the ordinary strict reader and every bound run as before. No data is invented, truncated,
// sorted, split on punctuation or dropped.

/// A JSON value that can be changed in place: what `normalize` works on.
nonisolated indirect enum LooseJSON: Equatable, Sendable {
    case null
    case bool(Bool)
    /// `integer`: written without a fraction or exponent. `text`: as serde_json would print it.
    case number(Double, integer: Bool, text: String)
    case string(String)
    case array([LooseJSON])
    case object(LooseObject)

    /// As serde_json compares values: `1` and `1.0` differ, and how a number was written doesn't matter.
    static func == (a: LooseJSON, b: LooseJSON) -> Bool {
        switch (a, b) {
        case (.null, .null): true
        case (.bool(let x), .bool(let y)): x == y
        case (.number(let x, let xi, _), .number(let y, let yi, _)): x == y && xi == yi
        case (.string(let x), .string(let y)): x == y
        case (.array(let x), .array(let y)): x == y
        case (.object(let x), .object(let y)): x == y
        default: false
        }
    }

    /// From the strict reader's value; of a key written twice, the last counts (as serde_json's).
    init(_ value: JSONValue) {
        switch value {
        case .null: self = .null
        case .bool(let b): self = .bool(b)
        case .number(let v, let integer, let text): self = .number(v, integer: integer, text: LooseJSON.printed(v, integer: integer, text: text))
        case .string(let s): self = .string(s)
        case .array(let items): self = .array(items.map { LooseJSON($0.value) })
        case .object(let members):
            var object = LooseObject()
            for m in members { object[m.key] = LooseJSON(m.value) }
            self = .object(object)
        }
    }

    /// For the strict reader (which no longer says where: serde reads a normalized value too).
    var json: JSONValue {
        switch self {
        case .null: .null
        case .bool(let b): .bool(b)
        case .number(let v, let integer, let text): .number(v, integer: integer, text: text)
        case .string(let s): .string(s)
        case .array(let items): .array(items.map { JSONItem(value: $0.json, end: 0) })
        case .object(let o): .object(o.keys.map { JSONMember(key: $0, keyEnd: 0, value: o.map[$0]?.json ?? .null, end: 0) })
        }
    }

    /// A number as serde_json prints it: an integer as written, anything else as its shortest form.
    static func printed(_ v: Double, integer: Bool, text: String) -> String {
        if integer, text.utf8.count <= 18 {
            // "-0" is a float to serde_json; other integers print as they were written.
            return text == "-0" ? "-0.0" : text
        }
        return "\(v)"
    }

    static func float(_ v: Double) -> LooseJSON { .number(v, integer: false, text: "\(v)") }

    var isString: Bool { if case .string = self { true } else { false } }
    var isNumber: Bool { if case .number = self { true } else { false } }
    var isObject: Bool { if case .object = self { true } else { false } }
    var isNull: Bool { if case .null = self { true } else { false } }
    var string: String? { if case .string(let s) = self { s } else { nil } }
    var array: [LooseJSON]? { if case .array(let a) = self { a } else { nil } }
    var object: LooseObject? { if case .object(let o) = self { o } else { nil } }
    var bool: Bool? { if case .bool(let b) = self { b } else { nil } }
    /// A member of an object (nil for anything else, as `Value::get`).
    func get(_ key: String) -> LooseJSON? { object?[key] }
}

/// An object's members in the order written.
nonisolated struct LooseObject: Equatable, Sendable {
    private(set) var keys: [String] = []
    private(set) var map: [String: LooseJSON] = [:]

    static func == (a: LooseObject, b: LooseObject) -> Bool { a.map == b.map }

    func contains(_ key: String) -> Bool { map[key] != nil }

    subscript(key: String) -> LooseJSON? {
        get { map[key] }
        set {
            if let newValue {
                if map.updateValue(newValue, forKey: key) == nil { keys.append(key) }
            } else if map.removeValue(forKey: key) != nil {
                keys.removeAll { $0 == key }
            }
        }
    }

    /// `from`'s value under `to`, in its place. A different value already under `to` is a
    /// conflict; the same one is just the one.
    mutating func rename(_ from: String, _ to: String) throws(VizError) {
        guard let value = map[from], let at = keys.firstIndex(of: from) else { return }
        if let existing = map[to] {
            if existing != value { throw .invalid("conflicting `\(from)` and `\(to)`") }
            self[from] = nil
        } else {
            map[from] = nil
            map[to] = value
            keys[at] = to
        }
    }
}

nonisolated enum VizLenient {
    enum Shape { case text, number, texts, numbers, objects, matrix }

    // The coercion table: scalar <-> sequence and numeric <-> textual values are only meaningful
    // in these contexts. In particular, table cells retain their types and ids are never joined.
    static let collections: [(String, String, Shape)] = [
        ("bar", "bars", .objects),
        ("line", "series", .objects),
        ("line", "x_labels", .texts),
        ("donut", "parts", .objects),
        ("stats", "tiles", .objects),
        ("table", "columns", .texts),
        ("heatmap", "x_labels", .texts),
        ("heatmap", "y_labels", .texts),
        ("heatmap", "values", .matrix),
        ("treemap", "items", .objects),
        ("timeline", "events", .objects),
        ("timeline", "ticks", .texts),
        ("flow", "nodes", .objects),
        ("flow", "edges", .objects),
        ("layers", "layers", .objects),
        ("mockup", "nodes", .objects),
    ]

    static let plurals: [(String, String)] = [
        ("bar", "bars"), ("part", "parts"), ("tile", "tiles"), ("column", "columns"), ("row", "rows"), ("item", "items"),
        ("event", "events"), ("tick", "ticks"), ("node", "nodes"), ("edge", "edges"), ("layer", "layers"), ("child", "children"),
        ("value", "values"), ("x_label", "x_labels"), ("y_label", "y_labels"), ("milestone", "events"), ("slice", "parts"),
    ]

    static let typeAliases: [(String, String)] = [
        ("bars", "bar"), ("bar_chart", "bar"), ("barchart", "bar"), ("bar_graph", "bar"), ("column_chart", "bar"), ("histogram", "bar"),
        ("line_chart", "line"), ("linechart", "line"), ("area", "line"),
        ("doughnut", "donut"), ("pie", "donut"), ("pie_chart", "donut"), ("pie_graph", "donut"),
        ("stat", "stats"), ("statistics", "stats"), ("metrics", "stats"), ("kpi", "stats"), ("kpis", "stats"),
        ("heat_map", "heatmap"), ("matrix", "heatmap"),
        ("tree_map", "treemap"),
        ("gantt", "timeline"), ("time_line", "timeline"), ("roadmap", "timeline"), ("schedule", "timeline"),
        ("flowchart", "flow"), ("flow_chart", "flow"), ("diagram", "flow"), ("graph", "flow"),
        ("layer", "layers"), ("stack", "layers"), ("tiers", "layers"),
        ("wireframe", "mockup"), ("wire_frame", "mockup"), ("mock", "mockup"),
    ]

    // One type's collection under another name. `data` is mapped only where the type has a single
    // obvious dataset; on `flow` (nodes or edges?) and `heatmap`'s `rows` it stays an error.
    static let fieldAliases: [(String, String, String)] = [
        ("bar", "data", "bars"),
        ("line", "data", "series"),
        ("line", "labels", "x_labels"),
        ("line", "categories", "x_labels"),
        ("line", "x_axis", "x_label"),
        ("line", "y_axis", "y_label"),
        ("line", "units", "unit"),
        ("donut", "data", "parts"),
        ("donut", "slices", "parts"),
        ("donut", "segments", "parts"),
        ("donut", "units", "unit"),
        ("stats", "data", "tiles"),
        ("stats", "metrics", "tiles"),
        ("stats", "cards", "tiles"),
        ("stats", "kpis", "tiles"),
        ("table", "data", "rows"),
        ("table", "headers", "columns"),
        ("heatmap", "data", "values"),
        ("treemap", "data", "items"),
        ("timeline", "data", "events"),
        ("timeline", "phases", "events"),
        ("timeline", "milestones", "events"),
        ("timeline", "units", "unit"),
        ("flow", "links", "edges"),
        ("flow", "connections", "edges"),
        ("layers", "tiers", "layers"),
        ("bar", "x_axis", "x_label"),
        ("bar", "y_axis", "y_label"),
    ]

    // Keys a mark carries under another name, per type (and collection, where a type has two).
    // `"*"` matches every collection of the type. Conflicting originals stay errors via `rename`.
    static let markAliases: [(String, String, String, String)] = [
        ("*", "*", "name", "label"),
        ("*", "*", "title", "label"),
        ("bar", "*", "x", "label"),
        ("bar", "*", "y", "value"),
        ("donut", "*", "x", "label"),
        ("donut", "*", "y", "value"),
        ("treemap", "*", "x", "label"),
        ("treemap", "*", "y", "weight"),
        ("treemap", "*", "size", "weight"),
        ("treemap", "*", "value", "weight"),
        ("line", "series", "data", "values"),
        ("line", "series", "points", "values"),
        ("stats", "*", "change", "delta"),
        ("stats", "*", "sparkline", "trend"),
        ("stats", "*", "data", "trend"),
        ("timeline", "*", "date", "start"),
        ("timeline", "*", "time", "start"),
        ("timeline", "*", "at", "start"),
        ("timeline", "*", "begin", "start"),
        ("timeline", "*", "from", "start"),
        ("timeline", "*", "until", "end"),
        ("timeline", "*", "through", "end"),
        ("timeline", "*", "to", "end"),
        ("timeline", "*", "track", "lane"),
        ("timeline", "*", "swimlane", "lane"),
        ("timeline", "*", "group", "lane"),
        ("flow", "edges", "source", "from"),
        ("flow", "edges", "target", "to"),
        ("flow", "edges", "text", "label"),
        ("flow", "nodes", "key", "id"),
        ("layers", "*", "contents", "items"),
        ("layers", "*", "components", "items"),
        ("layers", "*", "includes", "items"),
        ("table", "rows", "data", "cells"),
        ("table", "rows", "values", "cells"),
        ("mockup", "*", "type", "kind"),
        ("mockup", "*", "name", "text"),
        ("mockup", "*", "title", "text"),
        ("mockup", "*", "body", "text"),
        ("mockup", "*", "content", "text"),
        ("mockup", "*", "items", "children"),
        ("mockup", "*", "nodes", "children"),
    ]

    // `tone` names a renderer maps onto the six semantic tones; CSS and hex stay errors.
    static let toneAliases: [String: String] = [
        "neutral": "neutral", "gray": "neutral", "grey": "neutral", "muted": "neutral", "secondary": "neutral", "default": "neutral",
        "accent": "accent", "primary": "accent", "purple": "accent", "violet": "accent",
        "positive": "positive", "green": "positive", "success": "positive", "ok": "positive", "good": "positive",
        "warning": "warning", "warn": "warning", "yellow": "warning", "orange": "warning", "amber": "warning", "caution": "warning",
        "negative": "negative", "red": "negative", "danger": "negative", "error": "negative", "fail": "negative", "failure": "negative",
        "critical": "negative",
        "info": "info", "blue": "info", "information": "info",
    ]

    // The mockup kinds models reach for that aren't in the schema, to the nearest one that is.
    static let mockKindAliases: [String: String] = [
        "hstack": "row",
        "col": "column", "vstack": "column", "stack": "column", "box": "column", "container": "column", "div": "column",
        "section": "column", "group": "column",
        "panel": "card", "sheet": "card", "modal": "card", "dialog": "card", "popup": "card", "screen": "card",
        "label": "text", "heading": "text", "h1": "text", "h2": "text", "h3": "text", "paragraph": "text", "p": "text", "span": "text",
        "caption": "text",
        "text_field": "input", "textfield": "input", "field": "input", "search": "input", "search_bar": "input", "textarea": "input",
        "checkbox": "toggle", "switch": "toggle", "radio": "toggle",
        "slider": "progress", "spinner": "progress", "loader": "progress",
        "img": "image", "picture": "image", "photo": "image", "placeholder": "image",
        "icon": "avatar",
        "tag": "badge", "chip": "badge", "pill": "badge",
        "link": "button", "cta": "button",
        "hr": "divider", "separator": "divider", "spacer": "divider", "line": "divider",
        "stat": "metric", "kpi": "metric", "counter": "metric",
        "header": "nav", "footer": "nav", "toolbar": "nav", "navbar": "nav", "nav_bar": "nav", "menu": "nav", "menubar": "nav",
        "menu_bar": "nav", "sidebar": "nav", "tabbar": "nav", "tab_bar": "nav",
        "tab": "tabs", "segmented": "tabs", "segmented_control": "tabs",
        "ul": "list", "ol": "list",
        "window": "frame", "app": "frame",
    ]

    static let mockDeviceAliases: [String: String] = [
        "ios": "phone", "iphone": "phone", "mobile": "phone",
        "mac": "window", "desktop": "window", "app": "window",
        "web": "browser", "site": "browser", "page": "browser",
        "ipad": "tablet",
    ]

    // Every top-level and mark-level field or alias, for folding a wrong-case key to its spelling.
    static let knownKeys: Set<String> = [
        "version", "title", "summary", "type", "bars", "x_label", "y_label", "x_labels", "y_labels", "values", "items", "nodes",
        "series", "area", "unit", "parts", "tiles", "columns", "rows", "events", "ticks", "edges", "layers", "data", "slices",
        "segments", "metrics", "cards", "kpis", "headers", "links", "connections", "tiers", "phases", "milestones", "labels",
        "categories", "x_axis", "y_axis", "units", "description", "subtitle", "name", "heading", "label", "value", "weight", "tone",
        "color", "colour", "text", "kind", "children", "device", "start", "end", "lane", "id", "detail", "group", "cells", "delta",
        "trend", "from", "to", "x", "y", "points", "change", "sparkline", "size", "date", "time", "at", "begin", "until", "through",
        "track", "swimlane", "key", "body", "content", "contents", "components", "includes", "source", "target", "bar", "part",
        "tile", "column", "row", "item", "event", "tick", "node", "edge", "layer", "child", "milestone", "slice",
    ]

    private static func invalid(_ message: String) -> VizError { .invalid(message) }

    /// ASCII letters lowered; everything else as it is (`to_ascii_lowercase`).
    static func asciiLower(_ s: String) -> String {
        String(String.UnicodeScalarView(s.unicodeScalars.map { u in
            u.value >= 65 && u.value <= 90 ? Unicode.Scalar(u.value + 32)! : u
        }))
    }

    /// A name as the alias tables spell it: trimmed, ASCII-lowered, `-` and spaces as `_`.
    static func folded(_ s: String) -> String {
        asciiLower(s.trimmingCharacters(in: .whitespacesAndNewlines)).replacingOccurrences(of: "-", with: "_").replacingOccurrences(of: " ", with: "_")
    }

    static func toneAlias(_ name: String) -> String? { toneAliases[folded(name)] }

    // MARK: Tidying

    /// Strip JSON5-ish noise a strict parser rejects: `//` and `/* */` comments and commas that
    /// only precede a closing bracket, all outside strings, so a "https://" in a label survives.
    static func tidy(_ payload: String) -> String {
        let bytes = Array(payload.utf8)
        var out: [UInt8] = []
        out.reserveCapacity(bytes.count)
        var i = 0, inString = false, escaped = false
        let slash = UInt8(ascii: "/"), star = UInt8(ascii: "*"), newline = UInt8(ascii: "\n")
        func isSpace(_ b: UInt8) -> Bool { b == 0x20 || b == 0x09 || b == 0x0A || b == 0x0C || b == 0x0D }
        func pair(_ j: Int, _ a: UInt8, _ b: UInt8) -> Bool { j + 1 < bytes.count && bytes[j] == a && bytes[j + 1] == b }
        // Skip whitespace and comments starting at `j`; where the next token begins.
        func skipGap(_ from: Int) -> Int {
            var j = from
            while true {
                while j < bytes.count, isSpace(bytes[j]) { j += 1 }
                if pair(j, slash, slash) {
                    while j < bytes.count, bytes[j] != newline { j += 1 }
                } else if pair(j, slash, star) {
                    j += 2
                    while j + 1 < bytes.count, !pair(j, star, slash) { j += 1 }
                    j = min(j + 2, bytes.count)
                } else {
                    return j
                }
            }
        }
        while i < bytes.count {
            let b = bytes[i]
            if inString {
                out.append(b)
                if escaped {
                    escaped = false
                } else if b == UInt8(ascii: "\\") {
                    escaped = true
                } else if b == UInt8(ascii: "\"") {
                    inString = false
                }
                i += 1
                continue
            }
            if b == UInt8(ascii: "\"") {
                inString = true
                out.append(b)
                i += 1
            } else if pair(i, slash, slash) {
                while i < bytes.count, bytes[i] != newline { i += 1 }
            } else if pair(i, slash, star) {
                i += 2
                while i + 1 < bytes.count, !pair(i, star, slash) { i += 1 }
                i = min(i + 2, bytes.count)
            } else if b == UInt8(ascii: ","), case let next = skipGap(i + 1), next < bytes.count,
                      bytes[next] == UInt8(ascii: "]") || bytes[next] == UInt8(ascii: "}") {
                i += 1
            } else {
                out.append(b)
                i += 1
            }
        }
        return String(decoding: out, as: UTF8.self)
    }

    /// One stray inner fence, when a model wraps the JSON in ```json…``` inside the block.
    static func unwrapInnerFence(_ payload: String) -> String {
        let trimmed = payload.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("```") else { return payload }
        let rest = trimmed.dropFirst(3)
        guard let nl = rest.firstIndex(of: "\n") else { return payload }
        var body = rest[rest.index(after: nl)...]
        while let last = body.last, last.isWhitespace { body = body.dropLast() }
        guard body.hasSuffix("```") else { return payload }
        let inner = body.dropLast(3)
        return inner.contains("```") ? payload : String(inner)
    }

    /// The title and summary a block declares even when it doesn't parse: shown over the failure
    /// note so the answer still carries its message.
    static func declaredMetadata(_ payload: String) -> (title: String?, summary: String?) {
        guard let root = try? JSONReader.parse(tidy(unwrapInnerFence(payload))), case .object(let object) = LooseJSON(root) else {
            return (nil, nil)
        }
        func text(_ keys: [String]) -> String? {
            for key in keys {
                guard var value = object[key] else { continue }
                coerce(&value, .text)
                if let s = value.string, !s.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return s }
            }
            return nil
        }
        return (text(["title", "name", "heading"]), text(["summary", "description", "subtitle"]))
    }

    // MARK: Coercion

    static func coerce(_ value: inout LooseJSON, _ shape: Shape) {
        switch shape {
        case .text:
            switch value {
            case .number(_, _, let text):
                value = .string(text)
            case .array(var items):
                // Only strings/numbers: flattening nested objects would hide a schema error.
                for i in items.indices where items[i].isNumber { coerce(&items[i], .text) }
                if items.allSatisfy(\.isString) {
                    value = .string(items.compactMap(\.string).joined(separator: " "))
                } else {
                    value = .array(items)
                }
            default:
                break
            }
        case .number:
            if let s = value.string, let n = looseNumber(s) { value = n }
        case .texts, .numbers, .objects:
            let singleton = switch shape {
            case .texts, .numbers: value.isString || value.isNumber
            case .objects: value.isObject
            default: false
            }
            if singleton { value = .array([value]) }
            if case .array(var items) = value {
                for i in items.indices {
                    switch shape {
                    case .texts where items[i].isNumber: coerce(&items[i], .text)
                    case .numbers: coerce(&items[i], .number)
                    default: break
                    }
                }
                value = .array(items)
            }
        case .matrix:
            if case .array(var rows) = value {
                for i in rows.indices { coerce(&rows[i], .numbers) }
                value = .array(rows)
            }
        }
    }

    private static func isDigits(_ s: Substring) -> Bool { s.utf8.allSatisfy { $0 >= 48 && $0 <= 57 } }

    /// A number a model dressed up: commas, a currency sign, a percent sign or a k/M/B scale,
    /// dropped only when what's left is still one number ("1,200" yes, "1,2,3" no).
    static func looseNumber(_ source: String) -> LooseJSON? {
        func trimEnd(_ s: Substring) -> Substring {
            var s = s
            while let last = s.last, last.isWhitespace { s = s.dropLast() }
            return s
        }
        var text = Substring(source.trimmingCharacters(in: .whitespacesAndNewlines))
        if let first = text.first, "$€£¥".contains(first) { text = text.dropFirst().drop(while: \.isWhitespace) }
        if text.hasSuffix("%") { text = trimEnd(text.dropLast()) }
        var scale = 1.0
        // Lowercase k and uppercase M/B only: "m" and "b" mean too many other things.
        if let last = text.last, "kKMB".contains(last), text.count > 1 {
            scale = last == "k" || last == "K" ? 1e3 : last == "M" ? 1e6 : 1e9
            text = trimEnd(text.dropLast())
        }
        var cleaned = String(text)
        if text.contains(",") {
            let parts = text.split(separator: ".", omittingEmptySubsequences: false)
            let int: Substring, frac: Substring
            switch parts.count {
            case 1: (int, frac) = (parts[0], "")
            case 2 where isDigits(parts[1]): (int, frac) = (parts[0], parts[1])
            default: return nil
            }
            let unsigned = int.hasPrefix("-") ? int.dropFirst() : int
            let groups = unsigned.split(separator: ",", omittingEmptySubsequences: false)
            guard let head = groups.first, (1...3).contains(head.utf8.count), isDigits(head) else { return nil }
            var joined = text.hasPrefix("-") ? "-" : ""
            joined += head
            for group in groups.dropFirst() {
                guard group.utf8.count == 3, isDigits(group) else { return nil }
                joined += group
            }
            if !frac.isEmpty { joined += "." + frac }
            cleaned = joined
        }
        // Exactly one JSON number, as serde_json reads one.
        guard !cleaned.isEmpty, cleaned.utf8.allSatisfy({ !(($0 == 0x20) || ($0 >= 0x09 && $0 <= 0x0D)) }),
              case .number(let v, let integer, let written)? = try? JSONReader.parse(cleaned), v.isFinite else { return nil }
        if scale == 1 { return .number(v, integer: integer, text: LooseJSON.printed(v, integer: integer, text: written)) }
        let scaled = v * scale
        return scaled.isFinite ? .float(scaled) : nil
    }

    /// Vocabularies models use as timeline endpoints that already carry an order: ISO dates and
    /// years (0), months (1), weekdays (2), quarters (3). Endpoints from different vocabularies
    /// don't sort together.
    static func timeRank(_ source: String) -> (Int, Int)? {
        let months = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"]
        let weekdays = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"]
        let text = source.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.utf8.count >= 4, text.utf8.allSatisfy({ ($0 >= 48 && $0 <= 57) || $0 == 45 }), !text.contains("--") {
            let parts = text.split(separator: "-", omittingEmptySubsequences: false)
            guard let year = parts.first else { return nil }
            func part(_ i: Int) -> Int? { i < parts.count ? (isDigits(parts[i]) ? Int(parts[i]) : nil) : 0 }
            guard let month = part(1), let day = part(2) else { return nil }
            if year.utf8.count == 4, parts.count <= 3, (0...12).contains(month), (0...31).contains(day) {
                guard isDigits(year), let y = Int(year) else { return nil }
                return (0, y * 400 + month * 32 + day)
            }
            return nil
        }
        let lower = text.lowercased()
        let tokens = lower.unicodeScalars
            .split(whereSeparator: { !($0.isASCII && ($0.properties.isAlphabetic || ($0.value >= 48 && $0.value <= 57))) })
            .map { String(String.UnicodeScalarView($0)) }
        func number(_ t: String) -> Int? { !t.isEmpty && isDigits(Substring(t)) ? Int(t) : nil }
        let year = tokens.lazy.compactMap { $0.utf8.count == 4 ? number($0) : nil }.first
        func position(_ token: String, in names: [String]) -> Int? {
            names.firstIndex { token.utf8.count >= 3 && $0.hasPrefix(token) && token.utf8.count <= $0.utf8.count }
        }
        if let month = tokens.lazy.compactMap({ position($0, in: months) }).first {
            return (1, (year ?? 0) * 100 + month)
        }
        if tokens.count == 1, let day = position(tokens[0], in: weekdays) {
            return (2, day)
        }
        if let quarter = tokens.first, quarter.hasPrefix("q"), let q = number(String(quarter.dropFirst())), (1...4).contains(q),
           tokens.dropFirst().allSatisfy({ number($0) != nil }) {
            return (3, (year ?? 0) * 10 + q)
        }
        return nil
    }

    private static func fields(_ object: inout LooseObject, _ rules: [(String, Shape)]) throws(VizError) {
        for (from, to) in plurals where rules.contains(where: { $0.0 == to }) {
            try object.rename(from, to)
        }
        for (field, shape) in rules {
            if var value = object[field] {
                coerce(&value, shape)
                object[field] = value
            }
        }
    }

    /// Rename a key that only differs from a known one in case ("Title" -> "title"). If the right
    /// spelling is already taken, the odd key stays put and remains an unknown field.
    private static func foldKeyCase(_ object: inout LooseObject) {
        for key in object.keys {
            let lower = asciiLower(key)
            if lower != key, knownKeys.contains(lower), !object.contains(lower) {
                try? object.rename(key, lower)
            }
        }
    }

    // MARK: Normalizing

    static func normalize(_ value: inout LooseJSON) throws(VizError) {
        guard case .object(var object) = value else { return }
        defer { value = .object(object) }
        foldKeyCase(&object)
        // A null reads as "not given": let the defaults and inference below apply.
        for key in ["version", "type", "title", "summary"] where object[key]?.isNull == true {
            object[key] = nil
        }
        // A type is inferable only from a unique, type-specific collection. Never guess an axis.
        if !object.contains("type") {
            let candidates = [
                ("bar", "bars"), ("line", "series"), ("donut", "parts"), ("stats", "tiles"), ("table", "rows"), ("heatmap", "y_labels"),
                ("treemap", "items"), ("timeline", "events"), ("layers", "layers"),
            ].filter { _, field in
                object.contains(field) || plurals.contains { from, to in to == field && object.contains(from) }
            }
            if candidates.count == 1 { object["type"] = .string(candidates[0].0) }
            if !object.contains("type") {
                // A field alias that only one type uses ("links" -> flow's edges) gives it away;
                // one shared by several ("data") doesn't.
                let kinds = Set(fieldAliases
                    .filter { _, from, _ in object.contains(from) }
                    .filter { _, from, _ in fieldAliases.filter { $0.1 == from }.count == 1 }
                    .map(\.0))
                if kinds.count == 1, let kind = kinds.first { object["type"] = .string(kind) }
            }
        }
        if var kind = object["type"] {
            coerce(&kind, .text)
            object["type"] = kind
        }
        if let written = object["type"]?.string {
            let name = folded(written)
            object["type"] = .string(typeAliases.first { $0.0 == name }?.1 ?? name)
            if name == "area" {
                if let area = object["area"], area != .bool(true) { throw invalid("type `area` conflicts with `area`") }
                object["area"] = .bool(true)
            }
        }
        let kind = object["type"]?.string ?? ""
        guard VizSchema.types.contains(kind) else { return }
        if !object.contains("version") { object["version"] = .number(Double(VizSchema.version), integer: true, text: "\(VizSchema.version)") }
        // The metadata and the collection under the names models actually write.
        try object.rename("name", "title")
        try object.rename("heading", "title")
        try object.rename("description", "summary")
        try object.rename("subtitle", "summary")
        for (k, from, to) in fieldAliases where k == kind {
            try object.rename(from, to)
        }
        try fields(&object, [("version", .number), ("title", .text), ("summary", .text), ("unit", .text)])
        // Reuse provided metadata, not made-up chart titles or data labels.
        for (missing, supplied) in [("summary", "title"), ("title", "summary")] {
            if !object.contains(missing), let text = object[supplied], text.isString { object[missing] = text }
        }
        var rules: [(String, Shape)] = collections.filter { $0.0 == kind }.map { ($0.1, $0.2) }
        switch kind {
        case "bar": rules += [("x_label", .text), ("y_label", .text)]
        case "line": rules.append(("y_label", .text))
        case "table": try object.rename("row", "rows")
        case "timeline":
            try object.rename("tick", "ticks")
            // A string isn't a set of ordinal ticks. It means a numeric axis unit only when
            // every supplied endpoint is numeric; otherwise this ambiguity must stay an error.
            if let unit = object["ticks"]?.string {
                let given = object["events"] ?? object["event"]
                let events: [LooseJSON] = switch given {
                case .array(let events)?: events
                case .object?: [given!]
                default: []
                }
                let numeric = !events.isEmpty && events.allSatisfy { event in
                    event.get("start") != nil && ["start", "end"].compactMap { event.get($0) }.allSatisfy { point in
                        var point = point
                        coerce(&point, .number)
                        return point.isNumber
                    }
                }
                if !numeric { throw invalid("`ticks` must be an array of axis labels; use `unit` for a numeric axis unit") }
                object["ticks"] = nil
                var alias = LooseObject()
                alias["ticks"] = .string(unit)
                if let unit = object["unit"] { alias["unit"] = unit }
                try alias.rename("ticks", "unit")
                object["unit"] = alias["unit"]
            }
        default: break
        }
        try fields(&object, rules)
        // Mark key aliases ("from" -> "start") settle before ticks are derived or a matrix is
        // read off the heatmap.
        for (field, shape) in rules {
            guard case .objects = shape, case .array(var marks)? = object[field] else { continue }
            for i in marks.indices {
                if case .object(var mark) = marks[i] {
                    try shapeMark(&mark, kind, field)
                    marks[i] = .object(mark)
                }
            }
            object[field] = .array(marks)
        }
        if kind == "heatmap" {
            // `values` is one row per y label. Rows that instead run one per x label are
            // transposed, but only when they can't already be right (a square grid stays put).
            if let x = object["x_labels"]?.array?.count, let y = object["y_labels"]?.array?.count, let rows = object["values"]?.array {
                let matrix = rows.map { $0.array ?? [] }
                func fits(_ m: [[LooseJSON]], _ w: Int, _ h: Int) -> Bool { m.count == h && m.allSatisfy { $0.count == w } }
                if fits(matrix, y, x), !fits(matrix, x, y) {
                    var flipped = [[LooseJSON]](repeating: [], count: y)
                    for row in matrix {
                        for (column, cell) in row.enumerated() { flipped[column].append(cell) }
                    }
                    object["values"] = .array(flipped.map { .array($0) })
                } else if !fits(matrix, x, y), x > 1, matrix.count == x * y, matrix.allSatisfy({ $0.count == 1 }) {
                    // A flat row-major list ("values":[1,2,3,4]) chunked to fill the grid.
                    let flat = matrix.map { $0[0] }
                    object["values"] = .array(stride(from: 0, to: flat.count, by: x).map { .array(Array(flat[$0..<min($0 + x, flat.count)])) })
                }
            }
        }
        if kind == "timeline", !object.contains("ticks"), let events = object["events"]?.array, !events.isEmpty,
           events.allSatisfy({ event in
               event.get("start")?.isString == true && (event.get("end").map { $0.isNull || $0.isString } ?? true)
           }) {
            // Every endpoint a string and no `ticks`: the endpoints are the axis, unless every
            // one is a dressed-up number, which is a numeric axis, not a set of labels.
            var ordered: [String] = []
            for event in events {
                for key in ["start", "end"] {
                    if let point = event.get(key)?.string, !ordered.contains(point) { ordered.append(point) }
                }
            }
            if !ordered.allSatisfy({ looseNumber($0) != nil }) {
                // When they're all dates, months, weekdays or quarters they sort into that
                // order; otherwise first appearance wins, and an event ending before it
                // starts still errors.
                let ranks = ordered.map(timeRank)
                if ranks.allSatisfy({ $0 != nil }), let first = ranks.first??.0, ranks.allSatisfy({ $0?.0 == first }) {
                    // A stable sort, as Rust's `sort_by_key`.
                    ordered = zip(ranks.map { $0!.1 }, ordered).enumerated()
                        .sorted { a, b in a.element.0 != b.element.0 ? a.element.0 < b.element.0 : a.offset < b.offset }
                        .map(\.element.1)
                }
                object["ticks"] = .array(ordered.map { .string($0) })
            }
        }
        let ticks = object["ticks"]?.array
        for (field, shape) in rules {
            guard case .objects = shape, case .array(var marks)? = object[field] else { continue }
            for i in marks.indices { try normalizeMark(&marks[i], kind, field, ticks) }
            object[field] = .array(marks)
        }
        // Toned table rows get mark-level aliases and semantic colour spellings too.
        if kind == "table", case .array(var rows)? = object["rows"] {
            for i in rows.indices { try normalizeMark(&rows[i], "table", "rows", nil) }
            object["rows"] = .array(rows)
        }
    }

    private static func normalizeTone(_ object: inout LooseObject) throws(VizError) {
        // Only semantic names (or the colour words for them), never CSS/hex colours; those stay
        // unknown fields.
        for alias in ["color", "colour"] {
            if let v = object[alias]?.string, toneAlias(v) != nil { try object.rename(alias, "tone") }
        }
        if let name = object["tone"]?.string {
            guard let tone = toneAlias(name) else {
                throw invalid("`tone` must be one of neutral, accent, positive, warning, negative, info (got `\(name)`)")
            }
            object["tone"] = .string(tone)
        }
    }

    /// The renaming half of mark normalization: case-folded keys, synonym aliases, tone names,
    /// mockup kinds. It runs once before timeline ticks are derived (aliases like `from`/`to`
    /// must be `start`/`end` by then), then again as part of `normalizeMark`; both idempotent.
    private static func shapeMark(_ object: inout LooseObject, _ kind: String, _ field: String) throws(VizError) {
        foldKeyCase(&object)
        for (k, f, from, to) in markAliases {
            // `*` marks the aliases every labelled mark takes; mockup nodes take their own set.
            let applies = (k == "*" ? kind != "mockup" : k == kind) && (f == "*" || f == field)
            if applies { try object.rename(from, to) }
        }
        try normalizeTone(&object)
        if kind == "mockup" { try normalizeMockNode(&object) }
    }

    private static func normalizeMark(_ value: inout LooseJSON, _ kind: String, _ field: String, _ ticks: [LooseJSON]?) throws(VizError) {
        guard case .object(var object) = value else { return }
        defer { value = .object(object) }
        try shapeMark(&object, kind, field)
        var rules: [(String, Shape)] = [("label", .text)]
        switch kind {
        case "bar", "donut": rules.append(("value", .number))
        case "treemap": rules.append(("weight", .number))
        case "line": rules.append(("values", .numbers))
        case "stats": rules += [("value", .text), ("delta", .text), ("trend", .numbers)]
        case "timeline":
            rules.append(("lane", .text))
            for key in ["start", "end"] {
                guard var point = object[key] else { continue }
                if let ticks {
                    if case .number(_, _, let text) = point, ticks.contains(where: { $0.string == text }) { point = .string(text) }
                } else {
                    coerce(&point, .number)
                }
                object[key] = point
            }
        case "flow":
            rules += [("detail", .text), ("group", .text)]
            if field == "nodes", !object.contains("label"), let id = object["id"], id.isString { object["label"] = id }
        case "layers": rules += [("detail", .text), ("items", .texts)]
        case "mockup": rules += [("text", .text), ("value", .text), ("children", .objects)]
        default: break
        }
        try fields(&object, rules)
        if kind == "mockup" {
            // A toggle's value written as a boolean is its on/off state.
            if object["kind"]?.string == "toggle", let on = object["value"]?.bool { object["value"] = .string(on ? "on" : "off") }
            if case .array(var children)? = object["children"] {
                for i in children.indices { try normalizeMark(&children[i], kind, "children", nil) }
                object["children"] = .array(children)
            }
        }
    }

    /// Fold a mockup node's `kind` and `device` to schema spellings: aliasing what it meant, or
    /// inferring a kind from what the node carries (device → frame, children → column, text → text).
    private static func normalizeMockNode(_ object: inout LooseObject) throws(VizError) {
        switch object["kind"] {
        case .string(let kind)?:
            var name = folded(kind)
            if let to = mockKindAliases[name] { name = to }
            if MockKind(rawValue: name) == nil { throw invalid("`\(kind)` isn't a mockup kind") }
            object["kind"] = .string(name)
        case nil, .null?:
            let inferred: String
            if object.contains("device") {
                inferred = "frame"
            } else if object.contains("children") {
                inferred = "column"
            } else if object.contains("text") {
                inferred = "text"
            } else {
                throw invalid("a mockup node needs a `kind`")
            }
            object["kind"] = .string(inferred)
        default:
            break
        }
        if let device = object["device"]?.string {
            var name = folded(device)
            if let to = mockDeviceAliases[name] { name = to }
            if MockDevice(rawValue: name) == nil { throw invalid("`\(device)` isn't a mockup device") }
            object["device"] = .string(name)
        }
    }
}
