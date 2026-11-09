import UIKit
import XCTest

/// The stream's and the element list's own rules, without a phone or a connection: `xcodebuild
/// test` on a simulator runs these and skips `Drive.testRun`.
final class StreamTests: XCTestCase {
    // MARK: The stream

    func testWhatAWatcherAsksForIsKeptWithinBounds() {
        let asked = ScreenStream.Options(query: "w=900&q=70&fps=30")
        XCTAssertEqual([asked.width, asked.quality, asked.fps], [900, 70, 30])
        XCTAssertTrue(asked.sharp)
        // Nothing asked: the screen's own size, 30 times a second at most.
        XCTAssertEqual(ScreenStream.Options(query: ""), ScreenStream.Options())
        XCTAssertEqual(ScreenStream.Options().width, 0)
        let wild = ScreenStream.Options(query: "w=50&q=500&fps=1000&sharp=0&other=1&w")
        XCTAssertEqual([wild.width, wild.quality, wild.fps], [120, 100, 60])
        XCTAssertFalse(wild.sharp)
        XCTAssertEqual(ScreenStream.Options(query: "w=-3&q=0&fps=0").width, 0)
        XCTAssertEqual(ScreenStream.Options(query: "q=0&fps=0").quality, 1)
        XCTAssertEqual(ScreenStream.Options(query: "q=0&fps=0").fps, 1)
    }

    func testASlowWatcherIsOwedOnlyTheNewestFrame() {
        var outbox = ScreenStream.Outbox()
        let (a, b, c, d) = (Data([1]), Data([2]), Data([3]), Data([4]))
        XCTAssertEqual(outbox.offer(a), a, "nothing on its way: it goes at once")
        XCTAssertNil(outbox.offer(b), "one is on its way: this one waits")
        XCTAssertNil(outbox.offer(c), "and is replaced by a newer one")
        XCTAssertEqual(outbox.sent(), c, "the newest follows, never the one in between")
        XCTAssertTrue(outbox.sending)
        XCTAssertNil(outbox.sent())
        XCTAssertFalse(outbox.sending)
        XCTAssertEqual(outbox.offer(d), d)
    }

    func testAStillScreenIsLookedAtLessOften() {
        let now = Date()
        var pace = ScreenStream.Pace(fps: 30)
        // Still for ever: a few looks a second.
        XCTAssertEqual(pace.interval(at: now), 1 / Double(ScreenStream.Pace.calmFPS), accuracy: 1e-9)
        // It changed, or a touch came: at full pace, for a while after.
        pace.lively = now
        XCTAssertEqual(pace.interval(at: now), 1.0 / 30, accuracy: 1e-9)
        XCTAssertEqual(pace.interval(at: now.addingTimeInterval(1.9)), 1.0 / 30, accuracy: 1e-9)
        XCTAssertEqual(pace.interval(at: now.addingTimeInterval(2.1)), 0.25, accuracy: 1e-9)
        // A watcher that asked for less than the calm pace gets no more than it asked.
        XCTAssertEqual(ScreenStream.Pace(fps: 2).interval(at: now), 0.5, accuracy: 1e-9)
    }

    func testAFrameIsOnePartWithItsLength() {
        let jpeg = Data([0xFF, 0xD8, 0x01, 0xFF, 0xD9])
        let part = ScreenStream.part(jpeg)
        let head = "--trekframe\r\nContent-Type: image/jpeg\r\nContent-Length: 5\r\n\r\n"
        XCTAssertEqual(part.prefix(head.utf8.count), Data(head.utf8))
        XCTAssertEqual(part.dropFirst(head.utf8.count).prefix(5), jpeg)
        XCTAssertEqual(part.suffix(2), Data("\r\n".utf8))
        XCTAssertEqual(part.count, head.utf8.count + 5 + 2)
        // A still screen: a part with nothing in it.
        let still = String(data: ScreenStream.still, encoding: .utf8)!
        XCTAssertTrue(still.hasPrefix("--trekframe\r\n"))
        XCTAssertTrue(still.contains("Content-Length: 0\r\n\r\n"))
        let answer = String(data: ScreenStream.head, encoding: .utf8)!
        XCTAssertTrue(answer.hasPrefix("HTTP/1.1 200 OK\r\n"))
        XCTAssertTrue(answer.contains("multipart/x-mixed-replace; boundary=trekframe\r\n"))
        XCTAssertTrue(answer.hasSuffix("\r\n\r\n"))
    }

    // MARK: The picture

    func testAPictureGetsSmallerByHalving() {
        XCTAssertEqual(Capture.halving(native: 1206, width: 0), 1, "0: as it is")
        XCTAssertEqual(Capture.halving(native: 1206, width: 1206), 1)
        XCTAssertEqual(Capture.halving(native: 1206, width: 2000), 1)
        XCTAssertEqual(Capture.halving(native: 1206, width: 900), 2)
        XCTAssertEqual(Capture.halving(native: 1320, width: 600), 4)
        XCTAssertEqual(Capture.halving(native: 2064, width: 500), 8)
        XCTAssertEqual(Capture.halving(native: 10_000, width: 120), 8, "never by more than 8")
    }

    func testAJPEGIsMadeSmallerWithoutLosingItsShape() throws {
        let size = CGSize(width: 400, height: 800)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let image = UIGraphicsImageRenderer(size: size, format: format).image { context in
            UIColor.systemTeal.setFill()
            context.fill(CGRect(origin: .zero, size: size))
            UIColor.white.setFill()
            context.fill(CGRect(x: 40, y: 80, width: 120, height: 300))
        }
        let jpeg = try XCTUnwrap(image.jpegData(compressionQuality: 0.9))
        XCTAssertTrue(Capture.isJPEG(jpeg))
        XCTAssertFalse(Capture.isJPEG(Data("{\"ok\":true}".utf8)))
        let own = try XCTUnwrap(Capture.size(jpeg))
        XCTAssertEqual([own.width, own.height], [400, 800])
        let half = try XCTUnwrap(Capture.halved(jpeg, by: 2, quality: 70))
        XCTAssertTrue(Capture.isJPEG(half))
        let halfSize = try XCTUnwrap(Capture.size(half))
        XCTAssertEqual([halfSize.width, halfSize.height], [200, 400])
        let narrow = try XCTUnwrap(Capture.scaled(jpeg, width: 100, quality: 70))
        let narrowSize = try XCTUnwrap(Capture.size(narrow))
        XCTAssertEqual([narrowSize.width, narrowSize.height], [100, 200])
        // No smaller than asked, and never larger than it is: the same bytes.
        XCTAssertEqual(Capture.halved(jpeg, by: 1, quality: 70), jpeg)
        XCTAssertEqual(Capture.scaled(jpeg, width: 0, quality: 70), jpeg)
        XCTAssertEqual(Capture.scaled(jpeg, width: 1000, quality: 70), jpeg)
        XCTAssertNil(Capture.scaled(Data("not a picture".utf8), width: 100, quality: 70))
    }

    func testAPrivateCallIsMadeOnlyWhenItStillTakesWhatItsGiven() {
        XCTAssertEqual(Capture.shape("@32@0:8@16d24"), "@@:@d")
        XCTAssertEqual(Capture.shape("@64@0:8q16{CGRect={CGPoint=dd}{CGSize=dd}}24@56"),
                       "@@:q{CGRect={CGPoint=dd}{CGSize=dd}}@")
        XCTAssertEqual(Capture.Via(rawValue: "daemon"), .daemon)
        XCTAssertNil(Capture.Via(rawValue: "magic"))
    }

    // MARK: Elements

    private struct Fake: Elements.Node {
        var elementType: XCUIElement.ElementType
        var label = ""
        var identifier = ""
        var text: String?
        var frame = CGRect(x: 20, y: 100, width: 200, height: 44)
        var isEnabled = true
        var isSelected = false
        var isFocused = false
        var kids: [Elements.Node] = []
    }

    private let screen = CGRect(x: 0, y: 0, width: 402, height: 874)

    private func flat(_ nodes: [Fake], limit: Int = 300, deadline: Date = .distantFuture) -> Elements.Flat {
        var flat = Elements.Flat(limit: limit, deadline: deadline)
        for node in nodes { Elements.flatten(node, screen: screen, into: &flat) }
        return flat
    }

    private func row(_ y: CGFloat) -> CGRect { CGRect(x: 20, y: y, width: 200, height: 44) }

    func testOnlyWhatSaysSomethingOrIsAControlIsListed() {
        let tree = Fake(elementType: .window, kids: [
            Fake(elementType: .navigationBar, identifier: "Settings", frame: row(60), kids: [
                Fake(elementType: .staticText, label: "Settings", frame: row(64)),
            ]),
            // A cell that says nothing: its button takes its place.
            Fake(elementType: .cell, frame: row(120), kids: [
                Fake(elementType: .button, label: "General", identifier: "general", frame: row(120), kids: [
                    Fake(elementType: .image, frame: row(121)),
                    Fake(elementType: .image, identifier: "chevron.forward", frame: row(122)),
                    Fake(elementType: .image, label: "Gear", frame: row(123)),
                ]),
            ]),
            // Only an identifier and nothing in it; and one with something in it.
            Fake(elementType: .other, identifier: "label-view", frame: row(180), kids: [
                Fake(elementType: .image, frame: row(181)),
            ]),
            Fake(elementType: .other, identifier: "Home screen icons", frame: row(240), kids: [
                Fake(elementType: .icon, label: "Maps", frame: row(240)),
            ]),
            // A control with no name is still a control.
            Fake(elementType: .switch, text: "1", frame: row(300)),
            Fake(elementType: .button, frame: row(360)),
            // Off the screen, and with no size.
            Fake(elementType: .button, label: "Below", frame: row(2000)),
            Fake(elementType: .button, label: "Flat", frame: CGRect(x: 20, y: 400, width: 0, height: 44)),
            Fake(elementType: .statusBar, frame: row(0), kids: [
                Fake(elementType: .staticText, label: "9:41", frame: row(0)),
            ]),
            Fake(elementType: .keyboard, frame: row(600), kids: [
                Fake(elementType: .key, label: "q", frame: row(600)),
            ]),
        ])
        let listed = flat([tree]).elements.map { "\($0.depth) \($0.type) \($0.label)\($0.identifier.isEmpty ? "" : " #\($0.identifier)")" }
        XCTAssertEqual(listed, [
            "0 NavigationBar  #Settings",
            "1 StaticText Settings",
            "0 Button General #general",
            "1 Image Gear",
            "0 Other  #Home screen icons",
            "1 Icon Maps",
            "0 Switch ",
            "0 Button ",
            "0 Keyboard ",
        ])
    }

    func testASecureFieldsValueIsNeverListed() throws {
        let fields = flat([
            Fake(elementType: .textField, label: "Email", text: "ada@example.com", frame: row(100), isFocused: true),
            Fake(elementType: .secureTextField, label: "Password", text: "hunter2", frame: row(160)),
        ]).elements
        XCTAssertEqual(fields[0].value, "ada@example.com")
        XCTAssertTrue(fields[0].focused)
        XCTAssertNil(fields[1].value)
        XCTAssertTrue(fields[1].secure)
        let json = fields[1].json
        XCTAssertNil(json["value"])
        XCTAssertEqual(json["secure"] as? Bool, true)
        XCTAssertEqual(json["type"] as? String, "SecureTextField")
        let text = String(data: try JSONSerialization.data(withJSONObject: fields.map(\.json)), encoding: .utf8)!
        XCTAssertFalse(text.contains("hunter2"))
        // The rest of an element, as it goes out.
        let email = fields[0].json
        XCTAssertEqual(email["frame"] as? [Double], [20, 100, 200, 44])
        XCTAssertEqual(email["value"] as? String, "ada@example.com")
        XCTAssertEqual(email["enabled"] as? Bool, true)
        XCTAssertEqual(email["focused"] as? Bool, true)
        XCTAssertEqual(email["selected"] as? Bool, false)
        XCTAssertEqual(email["depth"] as? Int, 0)
    }

    func testTheListIsBoundedInCountAndTime() {
        let buttons = (0..<50).map { Fake(elementType: .button, label: "B\($0)", frame: row(CGFloat(10 + $0 * 10))) }
        let capped = flat(buttons, limit: 10)
        XCTAssertEqual(capped.elements.count, 10)
        XCTAssertEqual(capped.more, 40)
        XCTAssertFalse(capped.partial)
        XCTAssertEqual(capped.elements.last?.label, "B9")
        // Past the count, what's inside a listed container still counts as more.
        let nested = flat([Fake(elementType: .table, label: "List", kids: buttons)], limit: 5)
        XCTAssertEqual(nested.elements.count, 5)
        XCTAssertEqual(nested.more, 46)
        // Its time is up: it stops walking, and says so.
        let late = flat(buttons, deadline: .distantPast)
        XCTAssertTrue(late.partial)
        XCTAssertLessThan(late.elements.count, 50)
        // A long label is cut.
        let long = flat([Fake(elementType: .staticText, label: String(repeating: "x", count: 1000))]).elements[0]
        XCTAssertEqual(long.label.count, Elements.maxText + 1)
        XCTAssertTrue(long.label.hasSuffix("…"))
    }

    func testARepeatIsListedOnce() {
        let bar = Fake(elementType: .other, label: "Vertical scroll bar, 2 pages", text: "0%", frame: row(200))
        let other = Fake(elementType: .other, label: "Vertical scroll bar, 2 pages", text: "0%", frame: row(500))
        XCTAssertEqual(flat([bar, bar, other, bar]).elements.count, 2, "the same thing in the same place, once")
    }

    func testTheAnswersBoundsComeFromTheQuery() {
        XCTAssertEqual(Elements.Limits(query: ""), Elements.Limits())
        let asked = Elements.Limits(query: "max=40&ms=1500")
        XCTAssertEqual(asked.count, 40)
        XCTAssertEqual(asked.seconds, 1.5, accuracy: 1e-9)
        let wild = Elements.Limits(query: "max=100000&ms=1")
        XCTAssertEqual(wild.count, 1000)
        XCTAssertEqual(wild.seconds, 0.2, accuracy: 1e-9)
        XCTAssertEqual(Elements.name(.secureTextField), "SecureTextField")
        XCTAssertEqual(Elements.name(.staticText), "StaticText")
        XCTAssertEqual(Elements.name(.icon), "Icon")
    }

    func testASnapshotsTimeLimitGoesBackToWhatItWas() {
        // XCTest's own limit, read around a bounded call (skipped where this XCTest hasn't it).
        guard let cls: AnyClass = NSClassFromString("XCTTimeoutControls"),
              let getter = class_getClassMethod(cls, NSSelectorFromString("defaultQueryExecutionTimeout")),
              Capture.shape(getter) == "d@:"
        else { return }
        typealias Get = @convention(c) (AnyClass, Selector) -> Double
        let read = { unsafeBitCast(method_getImplementation(getter), to: Get.self)(cls, NSSelectorFromString("defaultQueryExecutionTimeout")) }
        let before = read()
        let during = Elements.bounded(seconds: 3) { read() }
        XCTAssertEqual(during, min(before, 3), accuracy: 1e-9)
        XCTAssertEqual(read(), before, accuracy: 1e-9)
        // Never lengthened.
        XCTAssertEqual(Elements.bounded(seconds: before + 100) { read() }, before, accuracy: 1e-9)
    }
}
