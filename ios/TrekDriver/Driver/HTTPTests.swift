import XCTest

/// The driver's HTTP parsing, without a phone: `xcodebuild test` on a simulator runs these and
/// skips `Drive.testRun`.
final class HTTPTests: XCTestCase {
    func testAWholeRequestWithABody() {
        let raw = Data("POST /tap?x=1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 15\r\nX-Trek-Token: abc\r\n\r\n{\"x\":10,\"y\":20}".utf8)
        guard case .done(let r, let used) = Request.parse(raw) else { return XCTFail("not parsed") }
        XCTAssertEqual(r.method, "POST")
        XCTAssertEqual(r.path, "/tap")
        XCTAssertEqual(r.query, "x=1")
        XCTAssertEqual(r.headers["x-trek-token"], "abc")
        XCTAssertEqual(number(r.json["x"]), 10)
        XCTAssertEqual(number(r.json["y"]), 20)
        XCTAssertEqual(used, raw.count)
    }

    func testWaitsForTheRestOfTheHeadersAndBody() {
        XCTAssertEqual(Request.parse(Data("GET /status HTTP/1.1\r\nHost: x".utf8)), .incomplete)
        XCTAssertEqual(Request.parse(Data("POST /type HTTP/1.1\r\nContent-Length: 10\r\n\r\n{\"te".utf8)), .incomplete)
    }

    func testARequestWithoutABody() {
        guard case .done(let r, _) = Request.parse(Data("GET /status HTTP/1.1\r\n\r\n".utf8)) else {
            return XCTFail("not parsed")
        }
        XCTAssertEqual(r.path, "/status")
        XCTAssertTrue(r.body.isEmpty)
        XCTAssertTrue(r.json.isEmpty)
    }

    func testRefusesWhatItDoesntTake() {
        guard case .bad = Request.parse(Data("HELLO\r\n\r\n".utf8)) else { return XCTFail("took a bad line") }
        guard case .bad = Request.parse(Data("POST /t HTTP/1.1\r\nContent-Length: x\r\n\r\n".utf8)) else {
            return XCTFail("took a bad length")
        }
        guard case .bad = Request.parse(Data("POST /t HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".utf8)) else {
            return XCTFail("took a chunked body")
        }
    }

    func testAnswersCloseTheConnection() {
        let text = String(data: Response.json(["ok": true]).encoded, encoding: .utf8)!
        XCTAssertTrue(text.hasPrefix("HTTP/1.1 200 OK\r\n"))
        XCTAssertTrue(text.contains("Content-Length: 11\r\n"))
        XCTAssertTrue(text.contains("Connection: close\r\n"))
        XCTAssertTrue(text.hasSuffix("\r\n\r\n{\"ok\":true}"))
    }

    func testNumbersFromJSONOrStrings() {
        XCTAssertEqual(number(NSNumber(value: 3.5)), 3.5)
        XCTAssertEqual(number("12"), 12)
        XCTAssertNil(number(nil))
        XCTAssertNil(number(["x"]))
    }
}
