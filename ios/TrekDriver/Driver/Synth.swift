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
