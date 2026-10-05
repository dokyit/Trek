import os
import QuartzCore
import SwiftUI

/// Timing for the slow paths (opening a thread, typing, switching tabs), on with the
/// `-TrekPerf YES` launch argument. Each measure runs from an event to the first display refresh
/// after it: the main thread is free again and the result is on screen. Lines go to stdout and
/// the unified log (`dev.trek.TrekMobile`, category `perf`); intervals show in Instruments as
/// signposts.
enum Perf {
    static let enabled = UserDefaults.standard.bool(forKey: "TrekPerf")
    static let log = Logger(subsystem: "dev.trek.TrekMobile", category: "perf")
    static let signposter = OSSignposter(subsystem: "dev.trek.TrekMobile", category: .pointsOfInterest)

    static var now: Double { CACurrentMediaTime() }

    static func report(_ line: String) {
        guard enabled else { return }
        print("[perf] \(line)")
        log.info("\(line, privacy: .public)")
    }

    static func ms(_ seconds: Double) -> String { String(format: "%.1f ms", seconds * 1000) }

    /// Runs `f` and reports how long it took, when `enabled`.
    @discardableResult
    static func measure<T>(_ label: StaticString, _ detail: @autoclosure () -> String = "", _ f: () throws -> T) rethrows -> T {
        guard enabled else { return try f() }
        let state = signposter.beginInterval(label)
        let start = now
        defer {
            signposter.endInterval(label, state)
            report("\(label)\(detail().isEmpty ? "" : " (\(detail()))"): \(ms(now - start))")
        }
        return try f()
    }

    /// Reports `label` with the time from `start` until the main thread has committed the frame
    /// with the change in it (the end of the run loop pass SwiftUI updates and lays out in), and
    /// the display refresh after that.
    static func untilFrame(_ label: String, from start: Double = now) {
        guard enabled else { return }
        // Last of the run loop's before-waiting observers: after SwiftUI's update and Core
        // Animation's commit.
        let observer = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.beforeWaiting.rawValue, false, CFIndex.max) { _, _ in
            MainActor.assumeIsolated {
                let committed = now
                FrameWatch.shared.next { at in
                    report("\(label): \(ms(committed - start)) (drawn \(ms(at - start)))")
                }
            }
        }
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
    }
}

/// Calls back on the first display refresh after a request: when the main thread got there, the
/// frame with the work in it is drawn.
@MainActor
final class FrameWatch: NSObject {
    static let shared = FrameWatch()
    private var link: CADisplayLink?
    private var waiting: [(Double) -> Void] = []

    func next(_ f: @escaping (Double) -> Void) {
        waiting.append(f)
        if link == nil {
            let l = CADisplayLink(target: self, selector: #selector(tick))
            l.add(to: .main, forMode: .common)
            link = l
        }
    }

    @objc private func tick() {
        let at = Perf.now
        let calls = waiting
        waiting = []
        calls.forEach { $0(at) }
        if waiting.isEmpty {
            link?.invalidate()
            link = nil
        }
    }
}

/// A binding that reports how long each change takes to reach the screen (typing, tab switches).
extension Binding {
    func timed(_ label: String) -> Binding<Value> {
        guard Perf.enabled else { return self }
        return Binding(get: { wrappedValue }, set: { value, transaction in
            let start = Perf.now
            self.transaction(transaction).wrappedValue = value
            Perf.untilFrame(label, from: start)
        })
    }
}
