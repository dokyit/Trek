import Foundation
import Synchronization

// Native visualizations in agent answers: strict JSON in a fenced `trek-viz` block. This mirrors
// `trek_core::visualization` (the schema, its limits, its error wording and the reading of a block
// still being written), so a block draws, or is refused, on the phone exactly as on the Mac.

nonisolated enum VizTone: String, Equatable, Sendable, CaseIterable {
    case neutral, accent, positive, warning, negative, info
}

/// A labelled number: a bar, a donut's part, a treemap's item (its weight).
nonisolated struct VizMark: Equatable, Sendable {
    var label: String
    var value: Double
    var tone: VizTone?
}

nonisolated struct VizSeries: Equatable, Sendable {
    var label: String
    var values: [Double]
    var tone: VizTone?
}

nonisolated struct VizTile: Equatable, Sendable {
    var label: String
    /// Already formatted by the agent ("1,284", "99.94%").
    var value: String
    var delta: String?
    var trend: [Double]?
    var tone: VizTone?
}

nonisolated enum VizCell: Equatable, Sendable {
    case number(Double)
    case text(String)
}

nonisolated struct VizRow: Equatable, Sendable {
    var cells: [VizCell]
    var tone: VizTone?
}

/// A position on a timeline: a number on a numeric axis, or one of its `ticks`.
nonisolated enum VizTime: Equatable, Sendable {
    case number(Double)
    case tick(String)
}

nonisolated struct VizEvent: Equatable, Sendable {
    var label: String
    var start: VizTime
    /// Nil: a milestone.
    var end: VizTime?
    var lane: String?
    var tone: VizTone?
}

nonisolated struct VizNode: Equatable, Sendable {
    var id: String
    var label: String
    var detail: String?
    var tone: VizTone?
    var group: String?
}

nonisolated struct VizEdge: Equatable, Sendable {
    var from: String
    var to: String
    var label: String?
}

nonisolated struct VizLayer: Equatable, Sendable {
    var label: String
    var detail: String?
    var items: [String]
    var tone: VizTone?
}

nonisolated enum MockKind: String, Equatable, Sendable, CaseIterable {
    case row, column, card, text, metric, button, badge, divider, input, toggle, progress, avatar, image, list, tabs, nav, frame

    var isContainer: Bool { [.row, .column, .card, .list, .tabs, .nav, .frame].contains(self) }
}

nonisolated enum MockDevice: String, Equatable, Sendable, CaseIterable {
    case phone, browser, window, tablet
}

nonisolated struct MockNode: Equatable, Sendable {
    var kind: MockKind
    var text: String?
    var value: String?
    var tone: VizTone?
    var device: MockDevice?
    var children: [MockNode]
}

nonisolated enum VizContent: Equatable, Sendable {
    case bar(bars: [VizMark], xLabel: String?, yLabel: String?)
    case line(xLabels: [String], series: [VizSeries], area: Bool, yLabel: String?, unit: String?)
    case donut(parts: [VizMark], unit: String?)
    case stats(tiles: [VizTile])
    case table(columns: [String], rows: [VizRow])
    case heatmap(xLabels: [String], yLabels: [String], values: [[Double]])
    case treemap(items: [VizMark])
    case timeline(events: [VizEvent], ticks: [String]?, unit: String?)
    case flow(nodes: [VizNode], edges: [VizEdge])
    case layers(layers: [VizLayer])
    case mockup(nodes: [MockNode])

    /// The wire name of its `type`.
    var kind: String {
        switch self {
        case .bar: "bar"
        case .line: "line"
        case .donut: "donut"
        case .stats: "stats"
        case .table: "table"
        case .heatmap: "heatmap"
        case .treemap: "treemap"
        case .timeline: "timeline"
        case .flow: "flow"
        case .layers: "layers"
        case .mockup: "mockup"
        }
    }
}

nonisolated struct Visualization: Equatable, Sendable {
    var title: String
    var summary: String
    var content: VizContent
    /// The block as parsed, for copying it as JSON.
    var json: JSONValue

    var kind: String { content.kind }

    /// Identifies one visualization across re-parses and the deltas of its stream (its type and
    /// title, both written before its marks), for its one-time entrance.
    var ident: Int {
        var h = Hasher()
        h.combine(kind)
        h.combine(title)
        return h.finalize()
    }

    static func == (a: Visualization, b: Visualization) -> Bool {
        a.title == b.title && a.summary == b.summary && a.content == b.content
    }
}

nonisolated enum VizError: Error, Equatable, Sendable {
    case tooLarge(bytes: Int)
    case json(String)
    case version(Int)
    case invalid(String)

    /// What went wrong, for a reader already told "Couldn't draw this visualization".
    var reason: String {
        switch self {
        case .tooLarge(let bytes):
            String(format: "it's %.1f KiB, over the %d KiB limit", Double(bytes) / 1024, VizSchema.maxPayloadBytes / 1024)
        case .json(let s), .invalid(let s): s
        case .version(let v): "schema version \(v) isn't supported (expected \(VizSchema.version))"
        }
    }
}

nonisolated enum VizSchema {
    static let version = 1
    static let maxPayloadBytes = 32 * 1024
    static let maxText = 256
    static let maxMarks = 128
    static let maxMockDepth = 6
    static let maxAbs = 1_000_000_000_000.0
    static let maxShort = 48
    static let maxSeries = 6
    static let maxParts = 12
    static let maxTiles = 12
    static let maxTrend = 64
    static let maxColumns = 12
    static let maxRows = 64
    static let maxTableCells = 384
    static let maxEvents = 48
    static let maxFlowNodes = 24
    static let maxFlowEdges = 48
    static let maxFlowGroups = 6
    static let maxLayers = 8
    static let maxLayerItems = 8
    static let maxTabs = 8
    static let imageShapes = ["wide", "square", "tall"]

    /// Every `type`, in the order the agent instructions list them.
    static let types = ["bar", "line", "donut", "stats", "table", "heatmap", "treemap", "timeline", "flow", "layers", "mockup"]

    /// The top-level fields each type takes.
    static let fields: [String: [String]] = [
        "bar": ["version", "title", "summary", "type", "bars", "x_label", "y_label"],
        "heatmap": ["version", "title", "summary", "type", "x_labels", "y_labels", "values"],
        "treemap": ["version", "title", "summary", "type", "items"],
        "mockup": ["version", "title", "summary", "type", "nodes"],
        "line": ["version", "title", "summary", "type", "x_labels", "series", "area", "y_label", "unit"],
        "donut": ["version", "title", "summary", "type", "parts", "unit"],
        "stats": ["version", "title", "summary", "type", "tiles"],
        "table": ["version", "title", "summary", "type", "columns", "rows"],
        "timeline": ["version", "title", "summary", "type", "events", "ticks", "unit"],
        "flow": ["version", "title", "summary", "type", "nodes", "edges"],
        "layers": ["version", "title", "summary", "type", "layers"],
    ]

    /// A progress node's `value` ("64", "64%") as a fraction, when it is one.
    static func progressFraction(_ value: String) -> Double? {
        var s = value.trimmingCharacters(in: .whitespacesAndNewlines)
        while s.hasSuffix("%") { s.removeLast() }
        guard let n = Double(s.trimmingCharacters(in: .whitespacesAndNewlines)), n.isFinite, (0...100).contains(n) else { return nil }
        return n / 100
    }
}

// MARK: JSON

/// A JSON value with its members in the order written (for unknown-field checks and for copying).
nonisolated indirect enum JSONValue: Sendable {
    case null
    case bool(Bool)
    /// `integer`: written without a fraction or exponent. `text`: as written.
    case number(Double, integer: Bool, text: String)
    case string(String)
    case array([JSONItem])
    case object([JSONMember])

    /// How serde names it in "invalid type: …".
    var unexpected: String {
        switch self {
        case .null: "null"
        case .bool(let b): "boolean `\(b)`"
        case .number(_, let integer, let text): integer ? "integer `\(text)`" : "floating point `\(text)`"
        case .string(let s): "string \(JSONValue.quote(s))"
        case .array: "sequence"
        case .object: "map"
        }
    }

    static func quote(_ s: String) -> String {
        var out = "\""
        for u in s.unicodeScalars {
            switch u {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            case "\u{08}": out += "\\b"
            case "\u{0C}": out += "\\f"
            default:
                if u.value < 0x20 { out += String(format: "\\u%04x", u.value) } else { out.unicodeScalars.append(u) }
            }
        }
        return out + "\""
    }

    /// Indented two spaces, members in the order written: what "Copy data as JSON" copies.
    func pretty(_ indent: String = "") -> String {
        let inner = indent + "  "
        switch self {
        case .null: return "null"
        case .bool(let b): return b ? "true" : "false"
        case .number(_, _, let text): return text
        case .string(let s): return JSONValue.quote(s)
        case .array(let items):
            if items.isEmpty { return "[]" }
            return "[\n" + items.map { inner + $0.value.pretty(inner) }.joined(separator: ",\n") + "\n\(indent)]"
        case .object(let members):
            if members.isEmpty { return "{}" }
            return "{\n" + members.map { inner + JSONValue.quote($0.key) + ": " + $0.value.pretty(inner) }.joined(separator: ",\n") + "\n\(indent)}"
        }
    }
}

/// A value in an array or object, with where it ends in the text (for errors that say where).
nonisolated struct JSONItem: Sendable {
    var value: JSONValue
    var end: Int
}

nonisolated struct JSONMember: Sendable {
    var key: String
    /// Just past the key's closing quote.
    var keyEnd: Int
    var value: JSONValue
    var end: Int
}

/// A strict JSON reader whose errors read like serde_json's ("expected `,` or `}` at line 1
/// column 40"), so a refused block says the same thing on both screens.
nonisolated struct JSONReader {
    private let b: [UInt8]
    private var i = 0
    private var depth = 0

    init(_ text: Substring) { b = Array(text.utf8) }
    init(_ text: String) { b = Array(text.utf8) }

    static func parse(_ text: String) throws -> JSONValue {
        var r = JSONReader(text)
        return try r.document().value
    }

    /// The value and where it ends.
    mutating func document() throws -> JSONItem {
        skip()
        let v = try value()
        let end = i
        skip()
        if i < b.count { throw fail("trailing characters") }
        return JSONItem(value: v, end: end)
    }

    private func fail(_ message: String, eof: Bool = false) -> VizError {
        .json("\(message) at \(JSONReader.place(b, eof ? i : i + 1))")
    }

    /// "line 1 column 40" for the byte just before `offset`, as serde_json counts.
    static func place(_ b: [UInt8], _ offset: Int) -> String {
        var line = 1, start = 0
        for (k, c) in b[..<min(offset, b.count)].enumerated() where c == 0x0A {
            line += 1
            start = k + 1
        }
        return "line \(line) column \(offset - start)"
    }

    var bytes: [UInt8] { b }

    private mutating func skip() {
        while i < b.count, b[i] == 0x20 || b[i] == 0x0A || b[i] == 0x0D || b[i] == 0x09 { i += 1 }
    }

    private mutating func value() throws -> JSONValue {
        guard i < b.count else { throw fail("EOF while parsing a value", eof: true) }
        switch b[i] {
        case UInt8(ascii: "{"): return try object()
        case UInt8(ascii: "["): return try array()
        case UInt8(ascii: "\""): return .string(try string())
        case UInt8(ascii: "t"): try word("true"); return .bool(true)
        case UInt8(ascii: "f"): try word("false"); return .bool(false)
        case UInt8(ascii: "n"): try word("null"); return .null
        case UInt8(ascii: "-"), UInt8(ascii: "0")...UInt8(ascii: "9"): return try number()
        default: throw fail("expected value")
        }
    }

    private mutating func word(_ w: String) throws {
        for c in w.utf8 {
            guard i < b.count else { throw fail("EOF while parsing a value", eof: true) }
            guard b[i] == c else { throw fail("expected ident") }
            i += 1
        }
    }

    private mutating func nest() throws {
        depth += 1
        if depth > 128 { throw fail("recursion limit exceeded") }
    }

    private mutating func object() throws -> JSONValue {
        try nest()
        defer { depth -= 1 }
        i += 1
        var members: [JSONMember] = []
        skip()
        if i < b.count, b[i] == UInt8(ascii: "}") { i += 1; return .object(members) }
        while true {
            skip()
            guard i < b.count else { throw fail("EOF while parsing an object", eof: true) }
            guard b[i] == UInt8(ascii: "\"") else { throw fail(b[i] == UInt8(ascii: "}") ? "trailing comma" : "key must be a string") }
            let key = try string()
            let keyEnd = i
            skip()
            guard i < b.count else { throw fail("EOF while parsing an object", eof: true) }
            guard b[i] == UInt8(ascii: ":") else { throw fail("expected `:`") }
            i += 1
            skip()
            let v = try value()
            members.append(JSONMember(key: key, keyEnd: keyEnd, value: v, end: i))
            skip()
            guard i < b.count else { throw fail("EOF while parsing an object", eof: true) }
            if b[i] == UInt8(ascii: ",") { i += 1; continue }
            if b[i] == UInt8(ascii: "}") { i += 1; return .object(members) }
            throw fail("expected `,` or `}`")
        }
    }

    private mutating func array() throws -> JSONValue {
        try nest()
        defer { depth -= 1 }
        i += 1
        var items: [JSONItem] = []
        skip()
        if i < b.count, b[i] == UInt8(ascii: "]") { i += 1; return .array(items) }
        while true {
            skip()
            if i < b.count, b[i] == UInt8(ascii: "]"), !items.isEmpty { throw fail("trailing comma") }
            let v = try value()
            items.append(JSONItem(value: v, end: i))
            skip()
            guard i < b.count else { throw fail("EOF while parsing a list", eof: true) }
            if b[i] == UInt8(ascii: ",") { i += 1; continue }
            if b[i] == UInt8(ascii: "]") { i += 1; return .array(items) }
            throw fail("expected `,` or `]`")
        }
    }

    private mutating func string() throws -> String {
        i += 1
        var out: [UInt8] = []
        while true {
            guard i < b.count else { throw fail("EOF while parsing a string", eof: true) }
            let c = b[i]
            if c == UInt8(ascii: "\"") { i += 1; return String(decoding: out, as: UTF8.self) }
            if c < 0x20 { throw fail("control character (\\u0000-\\u001F) found while parsing a string") }
            if c != UInt8(ascii: "\\") { out.append(c); i += 1; continue }
            i += 1
            guard i < b.count else { throw fail("EOF while parsing a string", eof: true) }
            let e = b[i]
            i += 1
            switch e {
            case UInt8(ascii: "\""): out.append(0x22)
            case UInt8(ascii: "\\"): out.append(0x5C)
            case UInt8(ascii: "/"): out.append(0x2F)
            case UInt8(ascii: "b"): out.append(0x08)
            case UInt8(ascii: "f"): out.append(0x0C)
            case UInt8(ascii: "n"): out.append(0x0A)
            case UInt8(ascii: "r"): out.append(0x0D)
            case UInt8(ascii: "t"): out.append(0x09)
            case UInt8(ascii: "u"):
                var scalar = try hex4()
                if (0xD800..<0xDC00).contains(scalar) {
                    guard i + 1 < b.count, b[i] == UInt8(ascii: "\\"), b[i + 1] == UInt8(ascii: "u") else {
                        throw fail("unexpected end of hex escape")
                    }
                    i += 2
                    let low = try hex4()
                    guard (0xDC00..<0xE000).contains(low) else { throw fail("lone leading surrogate in hex escape") }
                    scalar = 0x10000 + ((scalar - 0xD800) << 10) + (low - 0xDC00)
                }
                guard let u = Unicode.Scalar(scalar) else { throw fail("invalid unicode code point") }
                out.append(contentsOf: Array(String(Character(u)).utf8))
            default:
                throw fail("invalid escape")
            }
        }
    }

    private mutating func hex4() throws -> UInt32 {
        var v: UInt32 = 0
        for _ in 0..<4 {
            guard i < b.count else { throw fail("EOF while parsing a string", eof: true) }
            guard let d = Character(Unicode.Scalar(b[i])).hexDigitValue else { throw fail("invalid escape") }
            v = v * 16 + UInt32(d)
            i += 1
        }
        return v
    }

    private mutating func number() throws -> JSONValue {
        let start = i
        var integer = true
        if b[i] == UInt8(ascii: "-") { i += 1 }
        func digits() -> Int {
            let s = i
            while i < b.count, (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(b[i]) { i += 1 }
            return i - s
        }
        guard i < b.count else { throw fail("EOF while parsing a value", eof: true) }
        if b[i] == UInt8(ascii: "0") {
            i += 1
        } else if digits() == 0 {
            throw fail("invalid number")
        }
        if i < b.count, b[i] == UInt8(ascii: ".") {
            integer = false
            i += 1
            if digits() == 0 { throw fail(i < b.count ? "invalid number" : "EOF while parsing a value", eof: i >= b.count) }
        }
        if i < b.count, b[i] == UInt8(ascii: "e") || b[i] == UInt8(ascii: "E") {
            integer = false
            i += 1
            if i < b.count, b[i] == UInt8(ascii: "+") || b[i] == UInt8(ascii: "-") { i += 1 }
            if digits() == 0 { throw fail(i < b.count ? "invalid number" : "EOF while parsing a value", eof: i >= b.count) }
        }
        let text = String(decoding: b[start..<i], as: UTF8.self)
        guard let v = Double(text) else { throw fail("invalid number") }
        return .number(v, integer: integer, text: text)
    }
}

// MARK: Reading

nonisolated enum VizParse {
    /// Parse and validate a visualization's JSON (without its fence).
    static func parse(_ payload: String) throws(VizError) -> Visualization {
        let bytes = payload.utf8.count
        if bytes > VizSchema.maxPayloadBytes { throw .tooLarge(bytes: bytes) }
        do {
            var reader = JSONReader(payload)
            let root = try reader.document()
            let viz = try Decoder(bytes: reader.bytes).visualization(root)
            try validate(viz)
            return viz
        } catch let e as VizError {
            throw e
        } catch {
            throw .json("\(error)")
        }
    }

    /// Reads the schema's types out of JSON, failing as serde would and saying where.
    private struct Decoder {
        let bytes: [UInt8]
        typealias Node = JSONItem

        struct Fields {
            var map: [String: Node]
            /// Just past the object's closing brace.
            var end: Int
        }

        /// Where every error is said to be, when set: serde reads a flattened type out of the whole
        /// object, buffered, so its errors point at the object's end.
        var at: Int?

        func fail(_ message: String, _ offset: Int) -> VizError {
            .json("\(message) at \(JSONReader.place(bytes, at ?? offset))")
        }

        func typeError(_ n: Node, _ expected: String) -> VizError {
            fail("invalid type: \(n.value.unexpected), expected \(expected)", n.end)
        }

        func expectedList(_ names: [String]) -> String {
            switch names.count {
            case 0: "there are no fields"
            case 1: "`\(names[0])`"
            case 2: "`\(names[0])` or `\(names[1])`"
            default: "one of " + names.map { "`\($0)`" }.joined(separator: ", ")
            }
        }

        /// An object's members by key, refusing unknown ones (`deny`) and repeats.
        func fields(_ n: Node, _ what: String, _ allowed: [String], deny: Bool = true) throws -> Fields {
            guard case .object(let members) = n.value else { throw typeError(n, what) }
            var out: [String: Node] = [:]
            for m in members {
                if deny, !allowed.contains(m.key) { throw fail("unknown field `\(m.key)`, expected \(expectedList(allowed))", m.keyEnd) }
                if allowed.contains(m.key), out[m.key] != nil { throw fail("duplicate field `\(m.key)`", m.keyEnd) }
                out[m.key] = Node(value: m.value, end: m.end)
            }
            return Fields(map: out, end: n.end)
        }

        func required(_ f: Fields, _ key: String) throws -> Node {
            guard let v = f.map[key] else { throw fail("missing field `\(key)`", f.end) }
            return v
        }

        func string(_ n: Node) throws -> String {
            guard case .string(let s) = n.value else { throw typeError(n, "a string") }
            return s
        }

        func number(_ n: Node) throws -> Double {
            guard case .number(let v, _, _) = n.value else { throw typeError(n, "f64") }
            return v
        }

        func list(_ n: Node) throws -> [Node] {
            guard case .array(let a) = n.value else { throw typeError(n, "a sequence") }
            return a
        }

        func optional<T>(_ f: Fields, _ key: String, _ read: (Node) throws -> T) throws -> T? {
            guard let n = f.map[key] else { return nil }
            if case .null = n.value { return nil }
            return try read(n)
        }

        func strings(_ n: Node) throws -> [String] { try list(n).map(string) }
        func numbers(_ n: Node) throws -> [Double] { try list(n).map(number) }

        func variant<E: RawRepresentable & CaseIterable>(_ n: Node, _ type: E.Type) throws -> E where E.RawValue == String {
            guard case .string(let s) = n.value else { throw typeError(n, "string or map") }
            guard let e = E(rawValue: s) else {
                throw fail("unknown variant `\(s)`, expected \(expectedList(E.allCases.map(\.rawValue)))", n.end)
            }
            return e
        }

        func tone(_ f: Fields) throws -> VizTone? { try optional(f, "tone") { try variant($0, VizTone.self) } }

        func mark(_ n: Node, _ what: String, value key: String) throws -> VizMark {
            let f = try fields(n, "struct \(what)", ["label", key, "tone"])
            return VizMark(label: try string(required(f, "label")), value: try number(required(f, key)), tone: try tone(f))
        }

        func visualization(_ root: Node) throws -> Visualization {
            guard case .object(let members) = root.value else { throw VizError.json("top level must be an object") }
            guard case .string(let kind)? = members.last(where: { $0.key == "type" })?.value else {
                throw VizError.json("missing or non-string `type`")
            }
            guard let allowed = VizSchema.fields[kind] else { throw VizError.json("unsupported visualization type `\(kind)`") }
            if let field = members.map(\.key).first(where: { !allowed.contains($0) }) {
                let takes = allowed.dropFirst(4).map { "`\($0)`" }.joined(separator: ", ")
                throw VizError.json("unknown field `\(field)` (a \(kind) takes \(takes))")
            }
            let f = try fields(root, "struct Visualization", allowed, deny: false)
            let version: Int
            let v = try required(f, "version")
            switch v.value {
            case .number(let n, true, let text):
                guard n >= 0, n <= 255 else { throw fail("invalid value: integer `\(text)`, expected u8", v.end) }
                version = Int(n)
            default: throw typeError(v, "u8")
            }
            let title = try string(required(f, "title"))
            let summary = try string(required(f, "summary"))
            return try Decoder(bytes: bytes, at: root.end).content(kind, f, version: version, title: title, summary: summary, json: root.value)
        }

        func content(_ kind: String, _ f: Fields, version: Int, title: String, summary: String, json: JSONValue) throws -> Visualization {
            let content: VizContent
            switch kind {
            case "bar":
                content = .bar(bars: try list(required(f, "bars")).map { try mark($0, "BarMark", value: "value") },
                               xLabel: try optional(f, "x_label", string), yLabel: try optional(f, "y_label", string))
            case "heatmap":
                content = .heatmap(xLabels: try strings(required(f, "x_labels")), yLabels: try strings(required(f, "y_labels")),
                                   values: try list(required(f, "values")).map(numbers))
            case "treemap":
                content = .treemap(items: try list(required(f, "items")).map { try mark($0, "TreemapItem", value: "weight") })
            case "mockup":
                content = .mockup(nodes: try list(required(f, "nodes")).map(mockNode))
            case "line":
                let series = try list(required(f, "series")).map { n -> VizSeries in
                    let s = try fields(n, "struct LineSeries", ["label", "values", "tone"])
                    return VizSeries(label: try string(required(s, "label")), values: try numbers(required(s, "values")), tone: try tone(s))
                }
                var area = false
                if let a = f.map["area"] {
                    guard case .bool(let b) = a.value else { throw typeError(a, "a boolean") }
                    area = b
                }
                content = .line(xLabels: try strings(required(f, "x_labels")), series: series, area: area,
                                yLabel: try optional(f, "y_label", string), unit: try optional(f, "unit", string))
            case "donut":
                content = .donut(parts: try list(required(f, "parts")).map { try mark($0, "DonutPart", value: "value") },
                                 unit: try optional(f, "unit", string))
            case "stats":
                content = .stats(tiles: try list(required(f, "tiles")).map { n in
                    let t = try fields(n, "struct StatTile", ["label", "value", "delta", "trend", "tone"])
                    return VizTile(label: try string(required(t, "label")), value: try string(required(t, "value")),
                                   delta: try optional(t, "delta", string), trend: try optional(t, "trend", numbers), tone: try tone(t))
                })
            case "table":
                content = .table(columns: try strings(required(f, "columns")), rows: try list(required(f, "rows")).map(row))
            case "timeline":
                content = .timeline(events: try list(required(f, "events")).map(event), ticks: try optional(f, "ticks", strings),
                                    unit: try optional(f, "unit", string))
            case "flow":
                let nodes = try list(required(f, "nodes")).map { n -> VizNode in
                    let g = try fields(n, "struct FlowNode", ["id", "label", "detail", "tone", "group"])
                    return VizNode(id: try string(required(g, "id")), label: try string(required(g, "label")),
                                   detail: try optional(g, "detail", string), tone: try tone(g), group: try optional(g, "group", string))
                }
                let edges = try (f.map["edges"].map(list) ?? []).map { n -> VizEdge in
                    let e = try fields(n, "struct FlowEdge", ["from", "to", "label"])
                    return VizEdge(from: try string(required(e, "from")), to: try string(required(e, "to")), label: try optional(e, "label", string))
                }
                content = .flow(nodes: nodes, edges: edges)
            default:
                content = .layers(layers: try list(required(f, "layers")).map { n in
                    let l = try fields(n, "struct Layer", ["label", "detail", "items", "tone"])
                    return VizLayer(label: try string(required(l, "label")), detail: try optional(l, "detail", string),
                                    items: try l.map["items"].map(strings) ?? [], tone: try tone(l))
                })
            }
            if version != VizSchema.version { throw VizError.version(version) }
            return Visualization(title: title, summary: summary, content: content, json: json)
        }

        func cell(_ n: Node) throws -> VizCell {
            switch n.value {
            case .number(let v, _, _): return .number(v)
            case .string(let s): return .text(s)
            default: throw fail("data did not match any variant of untagged enum TableCell", n.end)
            }
        }

        func row(_ n: Node) throws -> VizRow {
            if case .array(let cells) = n.value, let cells = try? cells.map(cell) { return VizRow(cells: cells, tone: nil) }
            if case .object = n.value, let f = try? fields(n, "struct TonedRow", ["cells", "tone"]),
               let cells = try? list(required(f, "cells")).map(cell), case .success(let t) = Result(catching: { try tone(f) }) {
                return VizRow(cells: cells, tone: t)
            }
            throw fail("data did not match any variant of untagged enum TableRow", n.end)
        }

        func time(_ n: Node) throws -> VizTime {
            switch n.value {
            case .number(let v, _, _): return .number(v)
            case .string(let s): return .tick(s)
            default: throw fail("data did not match any variant of untagged enum TimePoint", n.end)
            }
        }

        func event(_ n: Node) throws -> VizEvent {
            let f = try fields(n, "struct TimelineEvent", ["label", "start", "end", "lane", "tone"])
            return VizEvent(label: try string(required(f, "label")), start: try time(required(f, "start")),
                            end: try optional(f, "end", time), lane: try optional(f, "lane", string), tone: try tone(f))
        }

        func mockNode(_ n: Node) throws -> MockNode {
            let f = try fields(n, "struct MockNode", ["kind", "text", "value", "tone", "device", "children"])
            return MockNode(kind: try variant(required(f, "kind"), MockKind.self), text: try optional(f, "text", string),
                            value: try optional(f, "value", string), tone: try tone(f),
                            device: try optional(f, "device") { try variant($0, MockDevice.self) },
                            children: try (f.map["children"].map(list) ?? []).map(mockNode))
        }
    }

    // MARK: Validation

    private static func invalid(_ message: String) -> VizError { .invalid(message) }

    private static func validate(_ viz: Visualization) throws {
        try text("title", viz.title, required: true)
        try text("summary", viz.summary, required: true)
        switch viz.content {
        case .bar(let bars, let x, let y):
            try dataset("bar", bars.count)
            try optionalText("x_label", x)
            try optionalText("y_label", y)
            for (i, bar) in bars.enumerated() {
                try text("bars[\(i)].label", bar.label, required: true)
                try number("bars[\(i)].value", bar.value)
            }
        case .heatmap(let xs, let ys, let values):
            if xs.isEmpty || ys.isEmpty { throw invalid("heatmap axes must be nonempty") }
            if xs.count * ys.count > VizSchema.maxMarks { throw invalid("heatmap has more than \(VizSchema.maxMarks) cells") }
            if values.count != ys.count || values.contains(where: { $0.count != xs.count }) {
                throw invalid("heatmap values must have one row per y label and one value per x label")
            }
            for (axis, labels) in [("x_labels", xs), ("y_labels", ys)] {
                for (i, l) in labels.enumerated() { try text("\(axis)[\(i)]", l, required: true) }
            }
            for (r, row) in values.enumerated() {
                for (c, v) in row.enumerated() { try number("values[\(r)][\(c)]", v) }
            }
        case .treemap(let items):
            try dataset("treemap", items.count)
            for (i, item) in items.enumerated() {
                try text("items[\(i)].label", item.label, required: true)
                try number("items[\(i)].weight", item.value)
                if item.value <= 0 { throw invalid("items[\(i)].weight must be positive") }
            }
        case .mockup(let nodes):
            if nodes.isEmpty { throw invalid("mockup nodes must be nonempty") }
            var count = 0
            for (i, node) in nodes.enumerated() { try validateNode(node, depth: 1, inFrame: false, count: &count, path: "nodes[\(i)]") }
        case .line(let xs, let series, _, let y, let unit):
            if xs.count < 2 { throw invalid("line needs at least two x labels") }
            try dataset("line x_labels", xs.count)
            try labels("x_labels", xs, brief: false)
            try optionalText("y_label", y)
            try shortOptional("unit", unit)
            if series.isEmpty || series.count > VizSchema.maxSeries { throw invalid("line needs 1 to \(VizSchema.maxSeries) series") }
            for (i, s) in series.enumerated() {
                try text("series[\(i)].label", s.label, required: true)
                if s.values.count != xs.count { throw invalid("series[\(i)] must have one value per x label") }
                for (p, v) in s.values.enumerated() { try number("series[\(i)].values[\(p)]", v) }
            }
        case .donut(let parts, let unit):
            if parts.isEmpty || parts.count > VizSchema.maxParts { throw invalid("donut needs 1 to \(VizSchema.maxParts) parts") }
            try shortOptional("unit", unit)
            for (i, p) in parts.enumerated() {
                try text("parts[\(i)].label", p.label, required: true)
                try number("parts[\(i)].value", p.value)
                if p.value <= 0 { throw invalid("parts[\(i)].value must be positive") }
            }
        case .stats(let tiles):
            if tiles.isEmpty || tiles.count > VizSchema.maxTiles { throw invalid("stats needs 1 to \(VizSchema.maxTiles) tiles") }
            for (i, t) in tiles.enumerated() {
                try text("tiles[\(i)].label", t.label, required: true)
                try short("tiles[\(i)].value", t.value)
                try shortOptional("tiles[\(i)].delta", t.delta)
                if let trend = t.trend {
                    if trend.count < 2 || trend.count > VizSchema.maxTrend {
                        throw invalid("tiles[\(i)].trend needs 2 to \(VizSchema.maxTrend) values")
                    }
                    for (p, v) in trend.enumerated() { try number("tiles[\(i)].trend[\(p)]", v) }
                }
            }
        case .table(let columns, let rows):
            if columns.isEmpty || columns.count > VizSchema.maxColumns { throw invalid("table needs 1 to \(VizSchema.maxColumns) columns") }
            try labels("columns", columns, brief: false)
            if rows.isEmpty || rows.count > VizSchema.maxRows { throw invalid("table needs 1 to \(VizSchema.maxRows) rows") }
            if rows.count * columns.count > VizSchema.maxTableCells { throw invalid("table has more than \(VizSchema.maxTableCells) cells") }
            for (i, row) in rows.enumerated() {
                if row.cells.count != columns.count { throw invalid("rows[\(i)] must have one cell per column") }
                for (c, cell) in row.cells.enumerated() {
                    switch cell {
                    case .number(let n): try number("rows[\(i)][\(c)]", n)
                    case .text(let s): try text("rows[\(i)][\(c)]", s, required: false)
                    }
                }
            }
        case .timeline(let events, let ticks, let unit):
            if events.isEmpty || events.count > VizSchema.maxEvents { throw invalid("timeline needs 1 to \(VizSchema.maxEvents) events") }
            try shortOptional("unit", unit)
            if let ticks {
                if ticks.isEmpty || ticks.count > VizSchema.maxMarks { throw invalid("timeline ticks need 1 to \(VizSchema.maxMarks) labels") }
                try labels("ticks", ticks, brief: true)
                for (i, t) in ticks.enumerated() where ticks[..<i].contains(t) { throw invalid("ticks[\(i)] repeats `\(t)`") }
            }
            var lanes: [String] = []
            for (i, e) in events.enumerated() {
                try text("events[\(i)].label", e.label, required: true)
                if let lane = e.lane {
                    try short("events[\(i)].lane", lane)
                    if !lanes.contains(lane) { lanes.append(lane) }
                }
                let start = try timePoint("events[\(i)].start", e.start, ticks)
                if let end = e.end {
                    let end = try timePoint("events[\(i)].end", end, ticks)
                    if end < start { throw invalid("events[\(i)] ends before it starts") }
                }
            }
            if lanes.count > VizSchema.maxLayers { throw invalid("timeline has more than \(VizSchema.maxLayers) lanes") }
        case .flow(let nodes, let edges):
            if nodes.isEmpty || nodes.count > VizSchema.maxFlowNodes { throw invalid("flow needs 1 to \(VizSchema.maxFlowNodes) nodes") }
            if edges.count > VizSchema.maxFlowEdges { throw invalid("flow has more than \(VizSchema.maxFlowEdges) edges") }
            var groups: [String] = []
            for (i, n) in nodes.enumerated() {
                let path = "nodes[\(i)]"
                try identifier("\(path).id", n.id)
                if nodes[..<i].contains(where: { $0.id == n.id }) { throw invalid("\(path).id `\(n.id)` is not unique") }
                try text("\(path).label", n.label, required: true)
                try optionalText("\(path).detail", n.detail)
                if let g = n.group {
                    try short("\(path).group", g)
                    if !groups.contains(g) { groups.append(g) }
                }
            }
            if groups.count > VizSchema.maxFlowGroups { throw invalid("flow has more than \(VizSchema.maxFlowGroups) groups") }
            for (i, e) in edges.enumerated() {
                for (end, id) in [("from", e.from), ("to", e.to)] where !nodes.contains(where: { $0.id == id }) {
                    throw invalid("edges[\(i)].\(end) names no node `\(id)`")
                }
                if e.from == e.to { throw invalid("edges[\(i)] connects `\(e.from)` to itself") }
                if edges[..<i].contains(where: { $0.from == e.from && $0.to == e.to }) {
                    throw invalid("edges[\(i)] repeats `\(e.from)` -> `\(e.to)`")
                }
                try shortOptional("edges[\(i)].label", e.label)
            }
        case .layers(let layers):
            if layers.isEmpty || layers.count > VizSchema.maxLayers { throw invalid("layers needs 1 to \(VizSchema.maxLayers) layers") }
            for (i, l) in layers.enumerated() {
                try text("layers[\(i)].label", l.label, required: true)
                try optionalText("layers[\(i)].detail", l.detail)
                if l.items.count > VizSchema.maxLayerItems { throw invalid("layers[\(i)] has more than \(VizSchema.maxLayerItems) items") }
                try labels("layers[\(i)].items", l.items, brief: true)
            }
        }
    }

    private static func validateNode(_ node: MockNode, depth: Int, inFrame: Bool, count: inout Int, path: String) throws {
        if depth > VizSchema.maxMockDepth { throw invalid("\(path) exceeds maximum mockup depth \(VizSchema.maxMockDepth)") }
        count += 1
        if count > VizSchema.maxMarks { throw invalid("mockup has more than \(VizSchema.maxMarks) nodes") }
        try optionalText("\(path).text", node.text)
        try optionalText("\(path).value", node.value)
        if !node.kind.isContainer, !node.children.isEmpty { throw invalid("\(path) leaf node cannot have children") }
        if (node.device != nil) != (node.kind == .frame) {
            throw invalid("\(path) `device` belongs on a frame, and every frame needs one")
        }
        func noValue(_ what: String) throws {
            if node.value != nil { throw invalid("\(path) \(what) cannot have a value") }
        }
        func needs(_ v: String?, _ field: String) throws {
            if v.map({ $0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }) ?? true { throw invalid("\(field) is required") }
        }
        switch node.kind {
        case .row, .column:
            if node.text != nil || node.value != nil { throw invalid("\(path) row/column cannot have text or value") }
        case .card, .list, .nav: try noValue("container")
        case .frame:
            try noValue("frame")
            if inFrame { throw invalid("\(path) frames cannot nest") }
        case .tabs:
            if node.text != nil { throw invalid("\(path) tabs take their labels from text children") }
            if node.children.isEmpty || node.children.count > VizSchema.maxTabs {
                throw invalid("\(path) tabs need 1 to \(VizSchema.maxTabs) text children")
            }
            if node.children.contains(where: { $0.kind != .text }) { throw invalid("\(path) tabs children must be text nodes") }
            if let active = node.value, !node.children.contains(where: { $0.text == active }) {
                throw invalid("\(path).value must name one of its tabs")
            }
        case .text, .button, .badge: try needs(node.text, "\(path).text")
        case .metric:
            try needs(node.text, "\(path).text")
            try needs(node.value, "\(path).value")
        case .input: try needs(node.text, "\(path).text")
        case .toggle:
            try needs(node.text, "\(path).text")
            if let v = node.value, v != "on", v != "off" { throw invalid("\(path).value must be `on` or `off`") }
        case .progress:
            if node.value.flatMap(VizSchema.progressFraction) == nil { throw invalid("\(path).value must be a percentage from 0 to 100") }
        case .avatar:
            try needs(node.text, "\(path).text")
            try noValue("avatar")
        case .image:
            if let v = node.value, !VizSchema.imageShapes.contains(v) {
                throw invalid("\(path).value must be one of \(VizSchema.imageShapes.joined(separator: ", "))")
            }
        case .divider:
            if node.text != nil || node.value != nil { throw invalid("\(path) divider cannot have text or value") }
        }
        for (i, child) in node.children.enumerated() {
            try validateNode(child, depth: depth + 1, inFrame: inFrame || node.kind == .frame, count: &count, path: "\(path).children[\(i)]")
        }
    }

    private static func dataset(_ name: String, _ count: Int) throws {
        if count == 0 { throw invalid("\(name) dataset must be nonempty") }
        if count > VizSchema.maxMarks { throw invalid("\(name) dataset has \(count) marks; maximum is \(VizSchema.maxMarks)") }
    }

    private static func text(_ field: String, _ value: String, required: Bool) throws {
        let length = value.unicodeScalars.count
        if required, value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { throw invalid("\(field) must be nonempty") }
        if length > VizSchema.maxText { throw invalid("\(field) is \(length) characters; maximum is \(VizSchema.maxText)") }
        if value.unicodeScalars.contains(where: { $0.properties.generalCategory == .control }) {
            throw invalid("\(field) contains a control character")
        }
    }

    private static func optionalText(_ field: String, _ value: String?) throws {
        if let value { try text(field, value, required: true) }
    }

    private static func short(_ field: String, _ value: String) throws {
        try text(field, value, required: true)
        let length = value.unicodeScalars.count
        if length > VizSchema.maxShort { throw invalid("\(field) is \(length) characters; maximum is \(VizSchema.maxShort)") }
    }

    private static func shortOptional(_ field: String, _ value: String?) throws {
        if let value { try short(field, value) }
    }

    private static func labels(_ field: String, _ values: [String], brief: Bool) throws {
        for (i, v) in values.enumerated() {
            if brief { try short("\(field)[\(i)]", v) } else { try text("\(field)[\(i)]", v, required: true) }
        }
    }

    private static func identifier(_ field: String, _ value: String) throws {
        try short(field, value)
        if !value.unicodeScalars.allSatisfy({ ($0.isASCII && ($0.properties.isAlphabetic || ("0"..."9").contains($0))) || $0 == "-" || $0 == "_" }) {
            throw invalid("\(field) may use only letters, digits, `-` and `_`")
        }
    }

    private static func timePoint(_ field: String, _ point: VizTime, _ ticks: [String]?) throws -> Double {
        switch (point, ticks) {
        case (.number(let v), nil):
            try number(field, v)
            return v
        case (.tick(let t), let ticks?):
            guard let i = ticks.firstIndex(of: t) else { throw invalid("\(field) `\(t)` is not one of the ticks") }
            return Double(i)
        case (.number, _?): throw invalid("\(field) must name a tick when `ticks` is set")
        case (.tick, nil): throw invalid("\(field) is a string, so the timeline needs `ticks`")
        }
    }

    private static func number(_ field: String, _ value: Double) throws {
        if !value.isFinite || abs(value) > VizSchema.maxAbs {
            throw invalid("\(field) must be finite and between -1000000000000 and 1000000000000")
        }
    }

    // MARK: A block still being written

    /// What can be read from a block still arriving: its type and title once each is written in
    /// full, and the visualization as far as it's written in whole marks (the text cut back to the
    /// last complete element of an open array and closed, when that validates). `complete`: the
    /// top-level object has closed, so only the fence is missing.
    struct Partial: Equatable, Sendable {
        var kind: String?
        var title: String?
        var drawable: Visualization?
        var complete = false
    }

    private struct Open {
        var array: Bool
        /// Just past its last complete member (just past the bracket while it has none).
        var good: Int
        /// An object is past a key's `:` and so reading a value.
        var value: Bool
    }

    /// `trek_core::visualization::parse_partial`: one linear scan, then a few bounded parses. Marks
    /// still being written are left out rather than guessed at, so `12` never shows on its way to `128`.
    static func partial(_ payload: String) -> Partial {
        let bytes = Array(payload.utf8)
        var out = Partial()
        var stack: [Open] = []
        var inString = false, escaped = false, stringStart = 0
        var key: (Int, Int)?
        var end: Int?
        func text(_ from: Int, _ to: Int) -> String { String(decoding: bytes[from..<to], as: UTF8.self) }
        scan: for (i, c) in bytes.enumerated() {
            if inString {
                if escaped {
                    escaped = false
                } else if c == UInt8(ascii: "\\") {
                    escaped = true
                } else if c == UInt8(ascii: "\"") {
                    inString = false
                    guard !stack.isEmpty else { break scan }
                    let isValue = stack[stack.count - 1].array || stack[stack.count - 1].value
                    if isValue { stack[stack.count - 1].good = i + 1 }
                    if stack.count == 1 {
                        if !isValue {
                            key = (stringStart, i + 1)
                        } else if let (ks, ke) = key {
                            key = nil
                            let value = { () -> String? in
                                if case .string(let s)? = try? JSONReader.parse(text(stringStart, i + 1)) { return s }
                                return nil
                            }
                            switch text(ks, ke) {
                            case "\"type\"": out.kind = value()
                            case "\"title\"": out.title = value()
                            default: break
                            }
                        }
                    }
                }
                continue
            }
            switch c {
            case UInt8(ascii: "\"") where !stack.isEmpty:
                inString = true
                stringStart = i
            case UInt8(ascii: "{"), UInt8(ascii: "["):
                if stack.count == 1 { key = nil }
                stack.append(Open(array: c == UInt8(ascii: "["), good: i + 1, value: false))
            case UInt8(ascii: "}"), UInt8(ascii: "]"):
                if stack.isEmpty { break scan }
                stack.removeLast()
                if stack.isEmpty {
                    end = i + 1
                    break scan
                }
                stack[stack.count - 1].good = i + 1
            case UInt8(ascii: ":"):
                if !stack.isEmpty { stack[stack.count - 1].value = true }
            case UInt8(ascii: ","):
                if !stack.isEmpty {
                    stack[stack.count - 1].good = i
                    stack[stack.count - 1].value = false
                }
                if stack.count == 1 { key = nil }
            default:
                if stack.isEmpty, !(c == 0x20 || c == 0x0A || c == 0x0D || c == 0x09 || c == 0x0C) { break scan }
            }
        }
        if bytes.count > VizSchema.maxPayloadBytes { return out }
        if let end {
            out.complete = true
            out.drawable = try? parse(text(0, end))
            return out
        }
        // Where to cut: each open array, innermost first, then the top-level object itself.
        let cuts = (0..<stack.count).reversed().filter { $0 == 0 || stack[$0].array }.prefix(4)
        for k in cuts {
            var s = text(0, stack[k].good)
            for open in stack[...k].reversed() { s.append(open.array ? "]" : "}") }
            if let viz = try? parse(s) {
                out.drawable = viz
                break
            }
        }
        return out
    }
}

// MARK: Blocks in Markdown

/// A `trek-viz` fence as the transcript draws it.
nonisolated enum VizBlock: Equatable, Sendable {
    /// Closed and valid.
    case drawn(Visualization)
    /// Its fence hasn't closed: what's been written so far.
    case arriving(VizParse.Partial, source: String)
    /// Closed, but not something Trek can draw: why, and the JSON as written.
    case invalid(reason: String, source: String)

    private static let cache = Mutex<[String: VizBlock]>([:])

    /// The block for `payload`, the JSON between the fences; `closed`: its closing fence is written.
    /// Parsed once per payload, so an answer streaming below a finished chart doesn't reparse it.
    static func make(payload: String, closed: Bool) -> VizBlock {
        let key = (closed ? "c" : "o") + payload
        if let hit = cache.withLock({ $0[key] }) { return hit }
        let block: VizBlock
        if closed {
            do {
                block = .drawn(try VizParse.parse(payload))
            } catch {
                block = .invalid(reason: error.reason, source: payload)
            }
        } else {
            block = .arriving(VizParse.partial(payload), source: payload)
        }
        cache.withLock { c in
            if c.count > 512 { c.removeAll(keepingCapacity: true) }
            c[key] = block
        }
        return block
    }
}
