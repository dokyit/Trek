import XCTest

/// The driver's HTTP parsing, without a phone: `xcodebuild test` on a simulator runs these and
/// skips `Drive.testRun`.
final class HTTPTests: XCTestCase {
    func testTheMacsAddressesOverWiFiCompareByTheirBytes() {
        // Trek passes the Mac's link-local address with or without its interface.
        XCTAssertEqual(Server.raw("fe80::1c71:8f4d:be86:59f1%en0"), Server.raw("fe80::1c71:8f4d:be86:59f1"))
        XCTAssertEqual(Server.raw("192.168.1.20")?.count, 4)
        // An IPv4 connection on the dual-stack listener can show as IPv4 inside IPv6.
        XCTAssertEqual(Server.raw("::ffff:192.168.1.20"), Server.raw("192.168.1.20"))
        XCTAssertNil(Server.raw("not an address"))
    }

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

    func testAPathsTimesComeFromItsFirstPointAndRise() {
        let json = """
        {"points":[{"x":100,"y":200,"t_ms":5000},{"x":100,"y":200,"t_ms":5592},\
        {"x":120,"y":200,"t_ms":5600},{"x":120,"y":200,"t_ms":5600}]}
        """
        let request = Request(method: "POST", path: "/path", headers: [:], body: Data(json.utf8))
        guard case .spots(let spots) = Synth.spots(request.json["points"]) else {
            return XCTFail("not taken")
        }
        XCTAssertEqual(spots.count, 4)
        XCTAssertEqual(spots[0], Synth.Spot(at: CGPoint(x: 100, y: 200), offset: 0))
        XCTAssertEqual(spots[1].offset, 0.592, accuracy: 1e-9)
        XCTAssertEqual(spots[2].at, CGPoint(x: 120, y: 200))
        // The same time twice: a millisecond apart, so the synthesizer sees them rising.
        XCTAssertEqual(spots[3].offset - spots[2].offset, 0.001, accuracy: 1e-9)
    }

    func testRefusesAPathItCantPlay() {
        let point: (Double) -> [String: Any] = { ["x": 1, "y": 2, "t_ms": $0] }
        let bad: [Any?] = [
            nil,
            [] as [Any],
            [["x": 1, "y": 2]],
            [point(100), point(50)],
            [point(0), point(31_000)],
            Array(repeating: point(0), count: Synth.maxSpots + 1),
        ]
        for points in bad {
            guard case .bad = Synth.spots(points) else {
                return XCTFail("took \(String(describing: points))")
            }
        }
        // One point is a press held there.
        guard case .spots(let one) = Synth.spots([["x": "3", "y": 4, "t_ms": 0]]) else {
            return XCTFail("one point")
        }
        XCTAssertEqual(one, [Synth.Spot(at: CGPoint(x: 3, y: 4), offset: 0)])
    }

    func testAPathIsThinnedToWhatTheSynthesizerTakes() {
        // A mouse's pace (every 16 ms) after a rest of 0.6 s, then a rest at the end.
        var spots = [Synth.Spot(at: CGPoint(x: 10, y: 10), offset: 0),
                     Synth.Spot(at: CGPoint(x: 10, y: 10), offset: 0.592)]
        for i in 0..<10 {
            spots.append(Synth.Spot(at: CGPoint(x: 10, y: 20 + Double(i) * 5), offset: 0.6 + Double(i) * 0.016))
        }
        spots.append(Synth.Spot(at: CGPoint(x: 10, y: 65), offset: 0.75))
        let thin = Synth.thinned(spots)
        XCTAssertEqual(thin.first, spots[0])
        XCTAssertEqual(thin[1], spots[1], "the rest's end stays")
        XCTAssertEqual(thin.last, spots.last, "the lift stays")
        for (a, b) in zip(thin, thin.dropFirst()) {
            XCTAssertGreaterThanOrEqual(b.offset - a.offset, Synth.minGap - 1e-9)
        }
        XCTAssertLessThan(thin.count, spots.count)
        // Two points close together: both go (a flick).
        let flick = [Synth.Spot(at: .zero, offset: 0), Synth.Spot(at: CGPoint(x: 0, y: 50), offset: 0.01)]
        XCTAssertEqual(Synth.thinned(flick), flick)
    }

    func testKeysTheKeyboardHas() {
        XCTAssertEqual(Actions.keyText("enter"), "\n")
        XCTAssertEqual(Actions.keyText("Backspace"), XCUIKeyboardKey.delete.rawValue)
        XCTAssertEqual(Actions.keyText("up"), XCUIKeyboardKey.upArrow.rawValue)
        XCTAssertNil(Actions.keyText("forwarddelete"))
        XCTAssertTrue(Actions.hardwareKeys.contains("left"))
        XCTAssertEqual(Actions.keyText("A"), "A")
        XCTAssertNil(Actions.keyText("hyper"))
        XCTAssertEqual(Actions.version, 3)
        for feature in ["path", "keys", "stream", "elements"] {
            XCTAssertTrue(Actions.features.contains(feature), feature)
        }
    }

    func testNumbersFromJSONOrStrings() {
        XCTAssertEqual(number(NSNumber(value: 3.5)), 3.5)
        XCTAssertEqual(number("12"), 12)
        XCTAssertNil(number(nil))
        XCTAssertNil(number(["x"]))
    }
}
