import XCTest

/// The driver itself: one UI test that serves taps, swipes, keys and text over HTTP on the
/// iPhone's loopback until Trek stops it (`/stop`, or ending `xcodebuild`), or nothing has asked
/// for a while. Run only by Trek, which sets `TREK_DRIVER=1`, the port and a fresh token in the
/// .xctestrun it starts.
final class Drive: XCTestCase {
    private var actions: Actions?

    func testRun() throws {
        let env = ProcessInfo.processInfo.environment
        guard env["TREK_DRIVER"] == "1" else { throw XCTSkip("Trek starts the driver itself.") }
        continueAfterFailure = true
        skipQuiescence()
        let port = UInt16(env["TREK_DRIVER_PORT"] ?? "") ?? 8117
        let idle = TimeInterval(env["TREK_DRIVER_IDLE"] ?? "") ?? 900
        let actions = Actions()
        self.actions = actions
        let server = Server(port: port, token: env["TREK_DRIVER_TOKEN"] ?? "", handle: actions.route)
        try server.start()
        XCTContext.runActivity(named: "TrekDriver: serving on 127.0.0.1:\(port)") { _ in }
        while !actions.stopped, Date().timeIntervalSince(actions.lastRequest) < idle {
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.25))
        }
        print(actions.stopped ? "TrekDriver: stopped by Trek" : "TrekDriver: idle for \(Int(idle)) s, stopping")
        server.stop()
    }

    /// A failed XCTest call (a tap XCTest couldn't make, text with nowhere to go) answers that
    /// request with an error instead of failing the driver.
    override func record(_ issue: XCTIssue) {
        guard let actions else { return super.record(issue) }
        NSLog("TrekDriver: \(issue.compactDescription)")
        actions.issue = issue.compactDescription
    }
}

/// XCTest waits for an app to go idle (no animation, an empty main queue) before and after each
/// action, up to a minute: an app that never idles (a spinner, a socket retrying) would make every
/// tap that slow. The driver acts at once, as a finger does, so the wait is skipped (what
/// WebDriverAgent's `waitForQuiescence` setting does).
func skipQuiescence() {
    guard let cls = NSClassFromString("XCUIApplicationProcess") else { return }
    var count: UInt32 = 0
    guard let methods = class_copyMethodList(cls, &count) else { return }
    defer { free(methods) }
    let skip: @convention(block) (AnyObject) -> Void = { _ in }
    let imp = imp_implementationWithBlock(skip)
    for i in 0..<Int(count) {
        let method = methods[i]
        let name = NSStringFromSelector(method_getName(method))
        // Only the void waits: anything returning a value keeps its own.
        guard name.hasPrefix("waitForQuiescence"),
              let types = method_getTypeEncoding(method), types.pointee == UInt8(ascii: "v")
        else { continue }
        method_setImplementation(method, imp)
    }
}
