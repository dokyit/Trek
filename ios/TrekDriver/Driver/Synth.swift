import UIKit
import XCTest

/// Touches made directly as XCTest's event synthesizer takes them (what WebDriverAgent's W3C
/// actions use): a pointer path of screen points, sent in one go, without first resolving an
/// element. About ten times faster than going through `XCUICoordinate` (no accessibility
/// snapshot first). Not public API: each call says whether it worked, and the caller falls back to
/// `XCUICoordinate` when it didn't.
enum Synth {
    /// A tap (or a press held `hold` seconds) at `point`.
    static func tap(_ point: CGPoint, hold: Double) -> Bool {
        guard let path = path(at: point) else { return false }
        guard call(path, "liftUpAtOffset:", Double(max(hold, 0.05))) else { return false }
        return send(path, name: "tap")
    }

    /// A drag from `from` to `to` over `seconds`, held a moment at each end so it scrolls without
    /// flinging further than asked.
    static func drag(from: CGPoint, to: CGPoint, seconds: Double) -> Bool {
        guard let path = path(at: from) else { return false }
        let start = 0.05
        guard move(path, to, at: start + seconds),
              call(path, "liftUpAtOffset:", start + seconds + 0.05)
        else { return false }
        return send(path, name: "drag")
    }

    /// One point of a path: where, and how long after the touch-down (seconds).
    struct Spot: Equatable {
        var at: CGPoint
        var offset: Double
    }

    enum Spots: Equatable {
        case spots([Spot])
        /// Not a path the driver takes, and why.
        case bad(String)
    }

    /// The most points a path takes, and how long it may run.
    static let maxSpots = 5000
    static let maxSeconds = 30.0

    /// A path from `/path`'s `points` (`[{x, y, t_ms}]`): times from the first point, each later
    /// than the one before (equal ones a millisecond apart: the synthesizer wants them rising).
    static func spots(_ any: Any?) -> Spots {
        guard let items = any as? [Any], !items.isEmpty else {
            return .bad("path takes points: [{x, y, t_ms}]")
        }
        guard items.count <= maxSpots else { return .bad("at most \(maxSpots) points") }
        var out: [Spot] = []
        var start: Double?
        for item in items {
            guard let p = item as? [String: Any], let x = number(p["x"]), let y = number(p["y"]),
                  let t = number(p["t_ms"]), x.isFinite, y.isFinite, t.isFinite
            else { return .bad("each point takes x and y in points, and t_ms") }
            let base = start ?? t
            start = base
            var offset = (t - base) / 1000
            if let last = out.last {
                guard offset >= last.offset else { return .bad("t_ms must rise from point to point") }
                offset = max(offset, last.offset + 0.001)
            }
            guard offset <= maxSeconds else { return .bad("a path runs \(Int(maxSeconds)) s at most") }
            out.append(Spot(at: CGPoint(x: x, y: y), offset: offset))
        }
        return .spots(out)
    }

    /// How close together the synthesizer takes a path's points. Closer ones break it: on iOS 27
    /// a drag sampled every 16 ms (a mouse's pace) moved nothing at all and ended early, every
    /// 20–25 ms it played slower than drawn, and from about 33 ms on as drawn (it moves smoothly
    /// between points by itself).
    static let minGap = 0.034

    /// `spots` with those closer than `minGap` to the one kept before dropped: the first and last
    /// always stay (the last in place of the one before it when they're too close), and so does
    /// the end of a rest, which comes long after its start.
    static func thinned(_ spots: [Spot]) -> [Spot] {
        var out: [Spot] = []
        for (i, spot) in spots.enumerated() {
            guard let kept = out.last, spot.offset - kept.offset < minGap else {
                out.append(spot)
                continue
            }
            if i == spots.count - 1 {
                if out.count > 1 { out.removeLast() }
                out.append(spot)
            }
        }
        return out
    }

    /// A touch down at the first spot that follows the rest at their offsets and lifts at the
    /// last. A rest is two spots at one place: it stays still between them.
    static func path(_ spots: [Spot]) -> Bool {
        let spots = thinned(spots)
        guard let first = spots.first, let last = spots.last, let path = path(at: first.at) else {
            return false
        }
        for spot in spots.dropFirst() {
            guard move(path, spot.at, at: spot.offset) else { return false }
        }
        guard call(path, "liftUpAtOffset:", max(last.offset, 0.05)) else { return false }
        return send(path, name: "path")
    }

    /// A key pressed as a hardware keyboard's, `key` an `XCUIKeyboardKey` (an arrow): typing its
    /// text would put the arrow's character in the field instead. Whether it worked.
    static func key(_ key: String) -> Bool {
        guard let cls = NSClassFromString("XCPointerEventPath") as? NSObject.Type,
              let blank = allocate(cls)
        else { return false }
        let initSel = NSSelectorFromString("initForTextInput")
        guard let initMethod = class_getInstanceMethod(cls, initSel) else { return false }
        typealias Init = @convention(c) (AnyObject, Selector) -> Unmanaged<NSObject>?
        let initialize = unsafeBitCast(method_getImplementation(initMethod), to: Init.self)
        guard let path = initialize(blank, initSel)?.takeRetainedValue() else { return false }
        let typeSel = NSSelectorFromString("typeKey:modifiers:atOffset:")
        guard let method = class_getInstanceMethod(Swift.type(of: path), typeSel) else { return false }
        typealias TypeKey = @convention(c) (AnyObject, Selector, NSString, UInt, Double) -> Void
        unsafeBitCast(method_getImplementation(method), to: TypeKey.self)(path, typeSel, key as NSString, 0, 0)
        return send(path, name: "key")
    }

    private static func path(at point: CGPoint) -> NSObject? {
        guard let cls = NSClassFromString("XCPointerEventPath") as? NSObject.Type else { return nil }
        let sel = NSSelectorFromString("initForTouchAtPoint:offset:")
        guard let method = class_getInstanceMethod(cls, sel),
              let blank = allocate(cls)
        else { return nil }
        typealias Init = @convention(c) (AnyObject, Selector, CGPoint, Double) -> Unmanaged<NSObject>?
        let initialize = unsafeBitCast(method_getImplementation(method), to: Init.self)
        return initialize(blank, sel, point, 0)?.takeRetainedValue()
    }

    private static func allocate(_ cls: NSObject.Type) -> AnyObject? {
        let alloc = NSSelectorFromString("alloc")
        guard let method = class_getClassMethod(cls, alloc) else { return nil }
        typealias Alloc = @convention(c) (AnyClass, Selector) -> Unmanaged<AnyObject>?
        let make = unsafeBitCast(method_getImplementation(method), to: Alloc.self)
        // +1, handed to init, which consumes it.
        return make(cls, alloc)?.takeUnretainedValue()
    }

    private static func call(_ path: NSObject, _ name: String, _ offset: Double) -> Bool {
        let sel = NSSelectorFromString(name)
        guard let method = class_getInstanceMethod(Swift.type(of: path), sel) else { return false }
        typealias Call = @convention(c) (AnyObject, Selector, Double) -> Void
        unsafeBitCast(method_getImplementation(method), to: Call.self)(path, sel, offset)
        return true
    }

    private static func move(_ path: NSObject, _ point: CGPoint, at offset: Double) -> Bool {
        let sel = NSSelectorFromString("moveToPoint:atOffset:")
        guard let method = class_getInstanceMethod(Swift.type(of: path), sel) else { return false }
        typealias Move = @convention(c) (AnyObject, Selector, CGPoint, Double) -> Void
        unsafeBitCast(method_getImplementation(method), to: Move.self)(path, sel, point, offset)
        return true
    }

    /// Wrap `path` in an event record and have the synthesizer play it, waiting (the main run loop
    /// keeps turning) until it says it's done.
    private static func send(_ path: NSObject, name: String) -> Bool {
        guard let cls = NSClassFromString("XCSynthesizedEventRecord") as? NSObject.Type,
              let blank = allocate(cls)
        else { return false }
        let initSel = NSSelectorFromString("initWithName:interfaceOrientation:")
        guard let initMethod = class_getInstanceMethod(cls, initSel) else { return false }
        typealias Init = @convention(c) (AnyObject, Selector, NSString, Int) -> Unmanaged<NSObject>?
        let initialize = unsafeBitCast(method_getImplementation(initMethod), to: Init.self)
        guard let record = initialize(blank, initSel, name as NSString, UIInterfaceOrientation.portrait.rawValue)?
            .takeRetainedValue()
        else { return false }
        let add = NSSelectorFromString("addPointerEventPath:")
        guard record.responds(to: add) else { return false }
        record.perform(add, with: path)

        let device = XCUIDevice.shared as NSObject
        guard device.responds(to: NSSelectorFromString("eventSynthesizer")),
              let synthesizer = device.value(forKey: "eventSynthesizer") as? NSObject
        else { return false }
        let play = NSSelectorFromString("synthesizeEvent:completion:")
        guard let method = class_getInstanceMethod(Swift.type(of: synthesizer), play) else { return false }
        typealias Play = @convention(c) (AnyObject, Selector, AnyObject, @escaping @convention(block) (Bool, NSError?) -> Void) -> Void
        var done = false
        var ok = false
        unsafeBitCast(method_getImplementation(method), to: Play.self)(synthesizer, play, record) { worked, _ in
            ok = worked
            done = true
        }
        let deadline = Date(timeIntervalSinceNow: 15)
        while !done, Date() < deadline {
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
        }
        return done && ok
    }
}
