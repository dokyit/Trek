import UIKit
import XCTest

/// What each endpoint does, in screen points of the whole device. Taps and swipes go through
/// SpringBoard's coordinate space (it fills the screen in every app), so they land wherever they
/// point, in any app or on the Home Screen.
final class Actions {
    /// The driver's version, for `/status` (the Mac talks to this one and older ones). 2 added
    /// `/path`, and the arrows and `count` in `/key`; 3 `/stream` and `/elements`.
    static let version = 3
    /// What it does beyond version 1's endpoints, for `/status`.
    static let features = ["path", "keys", "stream", "elements"]

    /// The screen pushed to whoever watches (`GET /stream`, which the server hands it).
    let stream = ScreenStream()

    private static let springboardID = "com.apple.springboard"
    private let springboard = XCUIApplication(bundleIdentifier: Actions.springboardID)
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

    /// The stream has a watcher: that counts as use, as a request does.
    func watched() {
        lastRequest = Date()
    }

    func route(_ request: Request) -> Response {
        if request.method == "GET", request.path == "/screenshot" {
            // Taken off the main thread (`Server`), beside a gesture that may be playing: it
            // leaves the gesture's state (`issue`) alone, and notes its time where that's kept.
            DispatchQueue.main.async { self.lastRequest = Date() }
            return screenshot(request.query)
        }
        lastRequest = Date()
        issue = nil
        // A touch or a key is about to change the screen: the stream looks at full pace again.
        if request.method == "POST" { stream.wake() }
        let response: Response
        switch (request.method, request.path) {
        case ("GET", "/status"): response = status()
        case ("GET", "/elements"): response = elements(request.query)
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
             (_, "/open"), (_, "/screenshot"), (_, "/stop"), (_, "/elements"), (_, "/stream"):
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
    /// quality (Trek's iPhone tool shows the screen this way from a driver without `/stream`, so
    /// the cable's capture, which makes iOS show 9:41, stays off). `via=daemon` takes it the way
    /// the stream does (always a JPEG), to compare the two.
    private func screenshot(_ query: String) -> Response {
        var width = 0
        var quality = 0
        var via = Capture.Via.xctest
        for pair in query.split(separator: "&") {
            let kv = pair.split(separator: "=", maxSplits: 1)
            guard kv.count == 2 else { continue }
            if kv[0] == "via" {
                guard let named = Capture.Via(rawValue: String(kv[1])) else {
                    return .error("via is daemon or xctest", status: 400)
                }
                via = named
            }
            guard let n = Int(kv[1]) else { continue }
            if kv[0] == "w" { width = n } else if kv[0] == "q" { quality = n }
        }
        if via == .daemon {
            let quality = quality > 0 ? quality : 70
            guard let raw = Capture.daemonJPEG(quality: width > 0 ? max(quality, Capture.sourceQuality) : quality),
                  let jpeg = Capture.scaled(raw, width: width, quality: quality)
            else { return .error("this iOS doesn't give the screen as a JPEG directly", status: 501) }
            return Response(status: 200, type: "image/jpeg", body: jpeg)
        }
        guard let (type, data) = Capture.xctest(width: width, quality: quality) else {
            return .error("couldn't encode the screen", status: 500)
        }
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
            "active": activeBundles().first ?? "",
            "stream": stream.stats,
        ])
    }

    /// What's on the screen, as a flat list (`Elements`): each app XCTest counts as active, the
    /// one in front first. That is SpringBoard on the Home Screen, and SpringBoard alone under a
    /// system alert (a permission asked over an app). `max=<n>` and `ms=<n>` bound the answer;
    /// `more` counts what was left out, and `partial` says the time ran out.
    private func elements(_ query: String) -> Response {
        let started = Date()
        let limits = Elements.Limits(query: query)
        let screen = CGRect(origin: .zero, size: size)
        var flat = Elements.Flat(limit: limits.count, deadline: started.addingTimeInterval(limits.seconds))
        var bundles = activeBundles()
        if bundles.isEmpty {
            // This XCTest doesn't say which apps are active: the app last opened here while
            // it's in front, else SpringBoard.
            let opened = lastOpened.flatMap {
                XCUIApplication(bundleIdentifier: $0).state == .runningForeground ? $0 : nil
            }
            bundles = [opened ?? Actions.springboardID]
        }
        var read: [String] = []
        for bundle in bundles {
            guard Date() < flat.deadline, !flat.partial else {
                flat.partial = true
                break
            }
            let left = max(flat.deadline.timeIntervalSinceNow, 0.2)
            let app = XCUIApplication(bundleIdentifier: bundle)
            guard let root = Elements.bounded(seconds: left, { try? app.snapshot() }) else { continue }
            read.append(bundle)
            // The app itself, by its bundle id, then what it shows.
            let name = root.label.trimmingCharacters(in: .whitespacesAndNewlines)
            if flat.elements.count < flat.limit {
                flat.elements.append(Elements.Element(
                    type: Elements.name(.application), label: Elements.clip(name), identifier: bundle,
                    frame: root.frame))
            }
            for kid in root.children {
                Elements.flatten(Elements.Live(snapshot: kid), screen: screen, depth: 1, into: &flat)
            }
        }
        guard !read.isEmpty else {
            return Date() >= flat.deadline
                ? .error("what's on the iPhone's screen took too long to read", status: 504)
                : .error("couldn't read what's on the iPhone's screen", status: 500)
        }
        return .json([
            "ok": true,
            "app": read[0],
            "width": size.width,
            "height": size.height,
            "elements": flat.elements.map(\.json),
            "more": flat.more,
            "partial": flat.partial,
            "ms": Int((Date().timeIntervalSince(started) * 1000).rounded()),
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
        activeBundles().first.map { XCUIApplication(bundleIdentifier: $0) }
    }

    /// The apps on the screen as XCTest's accessibility client has them, by bundle id: the one
    /// in front first, and SpringBoard (which is among them on the Home Screen) last.
    private func activeBundles() -> [String] {
        let device = XCUIDevice.shared as NSObject
        let interface = NSSelectorFromString("accessibilityInterface")
        let monitorSel = NSSelectorFromString("applicationMonitor")
        guard device.responds(to: interface), device.responds(to: monitorSel),
              let client = device.value(forKey: "accessibilityInterface") as? NSObject,
              let monitor = device.value(forKey: "applicationMonitor") as? NSObject
        else { return [] }
        let active = NSSelectorFromString("activeApplications")
        guard client.responds(to: active),
              let elements = client.perform(active)?.takeUnretainedValue() as? [NSObject]
        else { return [] }
        var bundles: [String] = []
        for element in elements {
            guard let pid = (element.value(forKey: "processIdentifier") as? NSNumber)?.int32Value,
                  let bundle = Actions.bundle(pid: pid, monitor: monitor), !bundles.contains(bundle)
            else { continue }
            bundles.append(bundle)
        }
        return bundles.filter { $0 != Actions.springboardID } + bundles.filter { $0 == Actions.springboardID }
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
}
