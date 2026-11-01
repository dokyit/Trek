import UIKit
import XCTest

/// What each endpoint does, in screen points of the whole device. Taps and swipes go through
/// SpringBoard's coordinate space (it fills the screen in every app), so they land wherever they
/// point, in any app or on the Home Screen.
final class Actions {
    /// The driver's version, for `/status` (the Mac checks it matches what it expects).
    static let version = 1

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
        case ("POST", "/type"): response = type(request.json)
        case ("POST", "/key"): response = key(request.json)
        case ("POST", "/open"): response = open(request.json)
        case ("GET", "/screenshot"):
            let png = XCUIScreen.main.screenshot().pngRepresentation
            response = Response(status: 200, type: "image/png", body: png)
        case ("POST", "/stop"):
            stopped = true
            response = .json(["ok": true])
        case (_, "/status"), (_, "/tap"), (_, "/swipe"), (_, "/type"), (_, "/key"), (_, "/open"),
             (_, "/screenshot"), (_, "/stop"):
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

    private func status() -> Response {
        return .json([
            "ok": true,
            "version": Actions.version,
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
        let text: String
        switch name {
        case "enter", "return": text = "\n"
        case "backspace", "delete": text = XCUIKeyboardKey.delete.rawValue
        case "tab": text = "\t"
        case "space": text = " "
        case "escape", "esc": text = XCUIKeyboardKey.escape.rawValue
        default:
            guard raw.count == 1 else {
                return .error("\(raw) isn't a key the iPhone's keyboard has over USB", status: 400)
            }
            text = raw
        }
        guard let app = focusedApp(hint: body["bundle"] as? String) else {
            return .error("Nothing has the keyboard on the iPhone: tap a text field first.", status: 409)
        }
        app.typeText(text)
        return .json(["ok": true])
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
