import Foundation

/// One HTTP/1.1 request, as much of it as the driver needs: the method, the path (query dropped),
/// lower-cased headers and the body (by Content-Length; no chunked bodies).
struct Request: Equatable {
    var method: String
    var path: String
    var headers: [String: String]
    var body: Data

    /// The body as a JSON object (empty when there is none or it isn't one).
    var json: [String: Any] {
        guard !body.isEmpty,
              let any = try? JSONSerialization.jsonObject(with: body),
              let dict = any as? [String: Any]
        else { return [:] }
        return dict
    }

    enum Parse: Equatable {
        /// More bytes are needed.
        case incomplete
        /// A whole request, and how many bytes of the buffer it took.
        case done(Request, Int)
        /// Not a request this server takes; answer 400 and close.
        case bad(String)
    }

    /// The most a request may be: the driver takes small JSON bodies only.
    static let limit = 1 << 20

    /// Parse the start of `buffer`.
    static func parse(_ buffer: Data) -> Parse {
        let crlf2 = Data("\r\n\r\n".utf8)
        guard let end = buffer.range(of: crlf2) else {
            return buffer.count > limit ? .bad("headers too long") : .incomplete
        }
        guard let head = String(data: buffer[buffer.startIndex..<end.lowerBound], encoding: .utf8)
        else { return .bad("headers aren't UTF-8") }
        var lines = head.components(separatedBy: "\r\n")
        let start = lines.removeFirst().split(separator: " ", omittingEmptySubsequences: true)
        guard start.count == 3, start[2].hasPrefix("HTTP/1.") else {
            return .bad("not an HTTP/1 request line")
        }
        var headers: [String: String] = [:]
        for line in lines where !line.isEmpty {
            guard let colon = line.firstIndex(of: ":") else { return .bad("bad header") }
            let name = line[..<colon].trimmingCharacters(in: .whitespaces).lowercased()
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            headers[name] = value
        }
        if headers["transfer-encoding"] != nil { return .bad("chunked bodies aren't taken") }
        let length: Int
        if let raw = headers["content-length"] {
            guard let n = Int(raw), n >= 0 else { return .bad("bad Content-Length") }
            length = n
        } else {
            length = 0
        }
        if length > limit { return .bad("body too long") }
        let bodyStart = end.upperBound
        let available = buffer.endIndex - bodyStart
        if available < length { return .incomplete }
        let body = Data(buffer[bodyStart..<(bodyStart + length)])
        var path = String(start[1])
        if let q = path.firstIndex(of: "?") { path = String(path[..<q]) }
        let request = Request(method: String(start[0]).uppercased(), path: path, headers: headers, body: body)
        return .done(request, bodyStart + length - buffer.startIndex)
    }

    static func == (a: Request, b: Request) -> Bool {
        a.method == b.method && a.path == b.path && a.headers == b.headers && a.body == b.body
    }
}

/// An answer: a status, a content type and the bytes. Always `Connection: close`.
struct Response {
    var status: Int
    var type: String
    var body: Data

    static func json(_ object: [String: Any], status: Int = 200) -> Response {
        let body = (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])) ?? Data("{}".utf8)
        return Response(status: status, type: "application/json", body: body)
    }

    static func error(_ message: String, status: Int) -> Response {
        json(["ok": false, "error": message], status: status)
    }

    var encoded: Data {
        let reason: String
        switch status {
        case 200: reason = "OK"
        case 400: reason = "Bad Request"
        case 401: reason = "Unauthorized"
        case 404: reason = "Not Found"
        case 405: reason = "Method Not Allowed"
        default: reason = "Error"
        }
        var head = "HTTP/1.1 \(status) \(reason)\r\n"
        head += "Content-Type: \(type)\r\n"
        head += "Content-Length: \(body.count)\r\n"
        head += "Connection: close\r\n\r\n"
        return Data(head.utf8) + body
    }
}

/// A number from a JSON body: JSON numbers and numeric strings.
func number(_ any: Any?) -> Double? {
    switch any {
    case let n as NSNumber: return n.doubleValue
    case let s as String: return Double(s)
    default: return nil
    }
}
