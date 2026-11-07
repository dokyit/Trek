import UIKit
import XCTest

/// What each endpoint does, in screen points of the whole device. Taps and swipes go through
/// SpringBoard's coordinate space (it fills the screen in every app), so they land wherever they
/// point, in any app or on the Home Screen.
final class Actions {
    /// The driver's version, for `/status` (the Mac talks to this one and older ones). 2 added
    /// `/path`, and the arrows and `count` in `/key`.
    static let version = 2
    /// What it does beyond version 1's endpoints, for `/status`.
    static let features = ["path", "keys"]

    private let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
    /// The app last opened through `/open`: where text goes when nothing else says.
    private var lastOpened: String?
    /// Set by `/stop`: the test returns and the runner goes away.
    private(set) var stopped = false
    /// When the last request came (an idle driver stops itself, so a Mac that went away doesn't
    /// leave it running).
    private(set) var lastRequest = Date()
    /// The failure XCTest recorded during the current request, if any (the test reports it here
    /// instead of failing).
    var issue: String?

    func route(_ request: Request) -> Response {
        lastRequest = Date()
        issue = nil
        let response: Response
        switch (request.method, request.path) {
        case ("GET", "/status"): response = status()
        case ("POST", "/tap"): response = tap(request.json)
        case ("POST", "/swipe"): response = swipe(request.json)
        case ("POST", "/path"): response = path(request.json)
        case ("POST", "/type"): response = type(request.json)
        case ("POST", "/key"): response = key(request.json)
        case ("POST", "/open"): response = open(request.json)
        case ("GET", "/screenshot"): response = screenshot(request.query)
        case ("POST", "/stop"):
            stopped = true
            response = .json(["ok": true])
        case (_, "/status"), (_, "/tap"), (_, "/swipe"), (_, "/path"), (_, "/type"), (_, "/key"),
             (_, "/open"), (_, "/screenshot"), (_, "/stop"):
            response = .error("\(request.method) isn't taken here", status: 405)
        default:
            response = .error("no \(request.path)", status: 404)
        }
        if let issue, response.status == 200, response.type == "application/json" {
            return .error(issue, status: 500)
        }
        return response
    }

    /// The screen in points (SpringBoard fills it), read once.
    private lazy var size: CGSize = springboard.frame.size

    /// The screen, at most `w=<n>` pixels wide: a PNG, or with `q=<1-100>` a JPEG of that
    /// quality (Trek's iPhone tool shows the screen this way while the driver runs, so the
    /// cable's capture, which makes iOS show 9:41, stays off).
    private func screenshot(_ query: String) -> Response {
        var width = 0
        var quality = 0
        for pair in query.split(separator: "&") {
            let kv = pair.split(separator: "=", maxSplits: 1)
            guard kv.count == 2, let n = Int(kv[1]) else { continue }
            if kv[0] == "w" { width = n } else if kv[0] == "q" { quality = n }
        }
        let shot = XCUIScreen.main.screenshot()
        var image = shot.image
        let pixels = image.size.width * image.scale
        guard quality > 0 || (width > 0 && pixels > CGFloat(width)) else {
            return Response(status: 200, type: "image/png", body: shot.pngRepresentation)
        }
        if width > 0, pixels > CGFloat(width) {
            let factor = CGFloat(width) / pixels
            let size = CGSize(width: (image.size.width * image.scale * factor).rounded(),
                              height: (image.size.height * image.scale * factor).rounded())
            let format = UIGraphicsImageRendererFormat()
            format.scale = 1
            image = UIGraphicsImageRenderer(size: size, format: format).image { _ in
                image.draw(in: CGRect(origin: .zero, size: size))
            }
        }
        let (type, data) = quality > 0
            ? ("image/jpeg", image.jpegData(compressionQuality: CGFloat(min(quality, 100)) / 100))
            : ("image/png", image.pngData())
        guard let data else { return .error("couldn't encode the screen", status: 500) }
        return Response(status: 200, type: type, body: data)
    }

    private func status() -> Response {
        return .json([
            "ok": true,
            "version": Actions.version,
            "features": Actions.features,
            "width": size.width,
            "height": size.height,
            "scale": UITraitCollection.current.displayScale,
            "active": activeApp().flatMap(bundleID) ?? "",
        ])
    }

    private func point(_ x: Double, _ y: Double) -> XCUICoordinate {
        springboard.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0))
            .withOffset(CGVector(dx: x, dy: y))
    }

    private func tap(_ body: [String: Any]) -> Response {
        guard let x = number(body["x"]), let y = number(body["y"]) else {
            return .error("tap takes x and y in points", status: 400)
        }
        let hold = (number(body["hold_ms"]) ?? 0) / 1000
        if Synth.tap(CGPoint(x: x, y: y), hold: hold) { return .json(["ok": true, "via": "synth"]) }
        let at = point(x, y)
        if hold > 0.15 { at.press(forDuration: hold) } else { at.tap() }
        return .json(["ok": true, "via": "coordinate"])
    }

    private func swipe(_ body: [String: Any]) -> Response {
        guard let x1 = number(body["x1"]), let y1 = number(body["y1"]),
              let x2 = number(body["x2"]), let y2 = number(body["y2"])
        else { return .error("swipe takes x1, y1, x2 and y2 in points", status: 400) }
        let seconds = max((number(body["duration_ms"]) ?? 300) / 1000, 0.05)
        if Synth.drag(from: CGPoint(x: x1, y: y1), to: CGPoint(x: x2, y: y2), seconds: seconds) {
            return .json(["ok": true, "via": "synth"])
        }
        let distance = hypot(x2 - x1, y2 - y1)
        let velocity = XCUIGestureVelocity(CGFloat(max(distance / seconds, 50)))
        point(x1, y1).press(forDuration: 0.05, thenDragTo: point(x2, y2),
                            withVelocity: velocity, thenHoldForDuration: 0.05)
        return .json(["ok": true])
    }

    /// A touch that follows `points` (`[{x, y, t_ms}]`, `t_ms` from the touch-down and rising):
    /// held where it rests, moved as it moves, lifted at the last point. What Trek's iPhone tool
    /// sends for the user's own drag, a press held before it moves included. `via: "coordinate"`
    /// plays it through XCTest's own press-then-drag instead (a straight line), to compare.
    private func path(_ body: [String: Any]) -> Response {
        let spots: [Synth.Spot]
        switch Synth.spots(body["points"]) {
        case .spots(let s): spots = s
        case .bad(let why): return .error(why, status: 400)
        }
        if body["via"] as? String != "coordinate", Synth.path(spots) {
            return .json(["ok": true, "via": "synth"])
        }
        // XCTest's own calls: pressed as long as the path rests first, dragged straight to its end.
        guard let first = spots.first, let last = spots.last else { return .error("no points", status: 400) }
        let rest = spots.prefix { $0.at == first.at }.last?.offset ?? 0
        let moving = max(last.offset - rest, 0.05)
        let distance = hypot(last.at.x - first.at.x, last.at.y - first.at.y)
        let velocity = XCUIGestureVelocity(CGFloat(max(distance / moving, 50)))
        point(first.at.x, first.at.y).press(
            forDuration: max(rest, 0.05), thenDragTo: point(last.at.x, last.at.y),
            withVelocity: velocity, thenHoldForDuration: 0.05)
        return .json(["ok": true, "via": "coordinate"])
    }

    private func type(_ body: [String: Any]) -> Response {
        guard let text = body["text"] as? String, !text.isEmpty else {
            return .error("type takes text", status: 400)
        }
        guard let app = focusedApp(hint: body["bundle"] as? String) else {
            return .error("Nothing has the keyboard on the iPhone: tap a text field first.", status: 409)
        }
        app.typeText(text)
        return .json(["ok": true])
    }

    private func key(_ body: [String: Any]) -> Response {
        guard let raw = body["key"] as? String, !raw.isEmpty else {
            return .error("key takes key", status: 400)
        }
        let name = raw.lowercased()
        let device = XCUIDevice.shared
        switch name {
        case "home":
            device.press(.home)
            return .json(["ok": true])
        case "lock", "power", "side":
            let lock = NSSelectorFromString("pressLockButton")
            guard device.responds(to: lock) else { return .error("can't press the side button", status: 501) }
            device.perform(lock)
            return .json(["ok": true])
        case "volume_up", "volumeup", "volume_down", "volumedown":
            #if targetEnvironment(simulator)
            return .error("no volume buttons in the simulator", status: 501)
            #else
            device.press(name.hasSuffix("up") ? .volumeUp : .volumeDown)
            return .json(["ok": true])
            #endif
        case "spotlight", "cmd+space":
            device.press(.home)
            point(size.width / 2, size.height * 0.35).press(
                forDuration: 0.05, thenDragTo: point(size.width / 2, size.height * 0.6),
                withVelocity: .fast, thenHoldForDuration: 0)
            return .json(["ok": true])
        default:
            break
        }
        guard let text = Actions.keyText(raw) else {
            return .error("\(raw) isn't a key the iPhone's keyboard has over USB", status: 400)
        }
        // `count`: a held key (Delete) in one go.
        let count = min(max(Int(number(body["count"]) ?? 1), 1), 100)
        // The arrows go as a hardware keyboard's keys: typed, they'd land in the field as
        // characters. (Forward delete, sent so, does nothing in iOS 27's fields: it isn't taken.)
        if Actions.hardwareKeys.contains(name) {
            for _ in 0..<count where !Synth.key(text) {
                return .error("can't press \(raw) on this iPhone", status: 501)
            }
            return .json(["ok": true])
        }
        guard let app = focusedApp(hint: body["bundle"] as? String) else {
            return .error("Nothing has the keyboard on the iPhone: tap a text field first.", status: 409)
        }
        app.typeText(String(repeating: text, count: count))
        return .json(["ok": true])
    }

    /// Keys pressed as a hardware keyboard's rather than typed.
    static let hardwareKeys: Set = [
        "up", "arrowup", "down", "arrowdown", "left", "arrowleft", "right", "arrowright",
    ]

    /// What typing `key` is (a key the keyboard has, or one character); nil for one it hasn't.
    static func keyText(_ key: String) -> String? {
        switch key.lowercased() {
        case "enter", "return": return "\n"
        case "backspace", "delete": return XCUIKeyboardKey.delete.rawValue
        case "tab": return "\t"
        case "space": return " "
        case "escape", "esc": return XCUIKeyboardKey.escape.rawValue
        case "up", "arrowup": return XCUIKeyboardKey.upArrow.rawValue
        case "down", "arrowdown": return XCUIKeyboardKey.downArrow.rawValue
        case "left", "arrowleft": return XCUIKeyboardKey.leftArrow.rawValue
        case "right", "arrowright": return XCUIKeyboardKey.rightArrow.rawValue
        default: return key.count == 1 ? key : nil
        }
    }

    private func open(_ body: [String: Any]) -> Response {
        guard let bundle = body["bundle"] as? String, !bundle.isEmpty else {
            return .error("open takes bundle", status: 400)
        }
        let app = XCUIApplication(bundleIdentifier: bundle)
        app.activate()
        lastOpened = bundle
        return .json(["ok": true, "state": app.state.rawValue])
    }

    // MARK: Which app is in front

    /// The app with the keyboard: the one in front when XCTest says (a private call WebDriverAgent
    /// uses too), else the hinted or last opened one if it's in front; nil when none shows a
    /// keyboard.
    private func focusedApp(hint: String?) -> XCUIApplication? {
        var candidates: [XCUIApplication] = []
        if let hint, !hint.isEmpty { candidates.append(XCUIApplication(bundleIdentifier: hint)) }
        if let active = activeApp() { candidates.append(active) }
        if let lastOpened { candidates.append(XCUIApplication(bundleIdentifier: lastOpened)) }
        candidates.append(springboard)
        for app in candidates where app.keyboards.count > 0 {
            return app
        }
        return nil
    }

    /// The app in front, through XCTest's own accessibility client (not public API; nil when this
    /// Xcode's XCTest doesn't have it).
    private func activeApp() -> XCUIApplication? {
        let device = XCUIDevice.shared as NSObject
        let interface = NSSelectorFromString("accessibilityInterface")
        let monitorSel = NSSelectorFromString("applicationMonitor")
        guard device.responds(to: interface), device.responds(to: monitorSel),
              let client = device.value(forKey: "accessibilityInterface") as? NSObject,
              let monitor = device.value(forKey: "applicationMonitor") as? NSObject
        else { return nil }
        let active = NSSelectorFromString("activeApplications")
        guard client.responds(to: active),
              let elements = client.perform(active)?.takeUnretainedValue() as? [NSObject]
        else { return nil }
        let bundles = elements.compactMap { element -> String? in
            guard let pid = (element.value(forKey: "processIdentifier") as? NSNumber)?.int32Value
            else { return nil }
            return Actions.bundle(pid: pid, monitor: monitor)
        }
        let bundle = bundles.first { $0 != "com.apple.springboard" } ?? bundles.first
        return bundle.map { XCUIApplication(bundleIdentifier: $0) }
    }

    /// The bundle id of the app running as `pid`, from XCTest's application monitor.
    private static func bundle(pid: Int32, monitor: NSObject) -> String? {
        let sel = NSSelectorFromString("monitoredApplicationWithProcessIdentifier:")
        guard let method = class_getInstanceMethod(Swift.type(of: monitor), sel) else { return nil }
        typealias Find = @convention(c) (AnyObject, Selector, Int32) -> Unmanaged<AnyObject>?
        let find = unsafeBitCast(method_getImplementation(method), to: Find.self)
        guard let found = find(monitor, sel, pid)?.takeUnretainedValue() as? NSObject,
              found.responds(to: NSSelectorFromString("bundleID"))
        else { return nil }
        return found.value(forKey: "bundleID") as? String
    }

    private func bundleID(_ app: XCUIApplication) -> String? {
        let sel = NSSelectorFromString("bundleID")
        guard app.responds(to: sel) else { return nil }
        return app.perform(sel)?.takeUnretainedValue() as? String
    }
}
