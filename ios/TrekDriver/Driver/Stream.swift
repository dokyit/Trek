import Foundation
import Network

/// The screen, pushed: `GET /stream?w=&q=&fps=` answers with frames for as long as the Mac keeps
/// the connection (`multipart/x-mixed-replace`, as an IP camera's MJPEG), instead of the Mac
/// asking for each one. One loop on a thread of its own takes the screen at most `fps` times a
/// second and sends a frame only when the screen changed; nothing is taken while nobody
/// watches. Gestures never wait for it: it touches neither the main thread nor the queue the
/// server answers on, and a watcher that reads slowly is owed only the newest frame.
final class ScreenStream {
    /// The most connections watching at once: the Mac's panel, and one to spare.
    static let maxClients = 2
    static let boundary = "trekframe"

    /// What a watcher asked for in its query.
    struct Options: Equatable {
        /// While the screen moves, frames are at most this many pixels wide (0, the default: the
        /// screen's own size). They get there by halving (`Capture.halving`), so a 1206-wide
        /// screen asked for at 900 comes 603 wide.
        var width = 0
        /// The JPEG's quality, 1–100.
        var quality = 70
        /// Frames a second at most.
        var fps = 30
        /// When the screen comes to rest after smaller frames, one at its own size follows.
        var sharp = true

        init() {}

        init(query: String) {
            for pair in query.split(separator: "&") {
                let kv = pair.split(separator: "=", maxSplits: 1)
                guard kv.count == 2, let n = Int(kv[1]) else { continue }
                switch kv[0] {
                case "w": width = n <= 0 ? 0 : min(max(n, 120), 4000)
                case "q": quality = min(max(n, 1), 100)
                case "fps": fps = min(max(n, 1), 60)
                case "sharp": sharp = n != 0
                default: break
                }
            }
        }
    }

    /// What one watcher is owed. Frames go to it one at a time; while one is on its way only
    /// the newest waits, so a slow link shows the screen as it is now rather than every frame
    /// late, and still gets the last frame before the screen went still.
    struct Outbox {
        private(set) var sending = false
        private(set) var waiting: Data?

        /// A new frame: what to send now, when nothing is on its way.
        mutating func offer(_ frame: Data) -> Data? {
            if sending {
                waiting = frame
                return nil
            }
            sending = true
            return frame
        }

        /// The one on its way went: the next to send, when one waited.
        mutating func sent() -> Data? {
            guard let next = waiting else {
                sending = false
                return nil
            }
            waiting = nil
            return next
        }
    }

    /// How often the screen is looked at: `fps` times a second while it changes or is touched,
    /// and a few times a second once it has been still a while (each look has the phone encode
    /// a picture, changed or not).
    struct Pace: Equatable {
        var fps: Int
        /// When the screen last changed, a touch or key last came, or a watcher joined.
        var lively = Date.distantPast

        static let calmAfter: TimeInterval = 2
        static let calmFPS = 4

        func interval(at now: Date) -> TimeInterval {
            let fast = 1 / Double(max(fps, 1))
            return now.timeIntervalSince(lively) < Pace.calmAfter ? fast : max(fast, 1 / Double(Pace.calmFPS))
        }
    }

    /// How long a watcher may hear nothing before it's told the screen is still (so a dead
    /// connection and a still screen don't look alike).
    static let stillEvery: TimeInterval = 1

    /// The answer's headers; the frames follow for as long as the connection lasts.
    static let head = Data(("HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=\(boundary)\r\n"
        + "Cache-Control: no-cache\r\nConnection: close\r\n\r\n").utf8)

    /// One frame as a part of the answer.
    static func part(_ jpeg: Data) -> Data {
        var out = Data("--\(boundary)\r\nContent-Type: image/jpeg\r\nContent-Length: \(jpeg.count)\r\n\r\n".utf8)
        out.append(jpeg)
        out.append(Data("\r\n".utf8))
        return out
    }

    /// An empty part: the screen hasn't changed.
    static let still = Data("--\(boundary)\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n\r\n".utf8)

    private final class Watcher {
        let connection: NWConnection
        var outbox = Outbox()
        var heard = Date()

        init(_ connection: NWConnection) {
            self.connection = connection
        }
    }

    /// Guards everything below, and wakes the loop from its wait.
    private let state = NSCondition()
    private var watchers: [Watcher] = []
    private var looping = false
    private var options = Options()
    /// A watcher joined: the next frame goes even when the screen didn't change.
    private var fresh = false
    private var pace = Pace(fps: 30)
    private var via = Capture.Via.daemon
    private var frames = 0
    private var same = 0
    private var failed = 0
    /// How long the last frames took to take (and make smaller), in seconds.
    private var took: [TimeInterval] = []
    private let capture = DispatchQueue(label: "dev.trek.driver.stream", qos: .userInitiated)

    /// Called about once a second while somebody watches (the driver isn't idle then).
    var onActivity: (() -> Void)?

    /// For `/status`: who watches and how the frames come.
    var stats: [String: Any] {
        state.lock()
        defer { state.unlock() }
        let mean = took.isEmpty ? 0 : took.reduce(0, +) / Double(took.count)
        return [
            "clients": watchers.count,
            "via": via.rawValue,
            "frames": frames,
            "same": same,
            "failed": failed,
            "capture_ms": (mean * 10_000).rounded() / 10,
            "width": options.width,
            "quality": options.quality,
            "fps": options.fps,
        ]
    }

    /// A connection that asked for `/stream` (its token and address already checked): from here
    /// on it's the stream's. Called on `io`, the queue the connection was started on.
    func add(_ connection: NWConnection, _ request: Request) {
        state.lock()
        guard watchers.count < ScreenStream.maxClients else {
            state.unlock()
            let refusal = Response.error("\(ScreenStream.maxClients) are watching the screen already", status: 503)
            connection.send(content: refusal.encoded, contentContext: .finalMessage, isComplete: true,
                            completion: .contentProcessed { _ in connection.cancel() })
            return
        }
        let watcher = Watcher(connection)
        if watchers.isEmpty {
            // The stream is the first watcher's; one joining later sees the same frames.
            options = Options(query: request.query)
            pace.fps = options.fps
        }
        watchers.append(watcher)
        fresh = true
        pace.lively = Date()
        let start = !looping
        looping = true
        state.signal()
        state.unlock()
        connection.send(content: ScreenStream.head, completion: .contentProcessed { [weak self] error in
            if error != nil { self?.remove(watcher) }
        })
        hear(watcher)
        if start { capture.async { [weak self] in self?.loop() } }
    }

    /// A touch or a key came: the screen is about to change, so it's looked at at full pace
    /// again, now.
    func wake() {
        state.lock()
        pace.lively = Date()
        state.signal()
        state.unlock()
    }

    /// Everyone watching is let go (the driver is stopping).
    func stop() {
        state.lock()
        let all = watchers
        watchers = []
        state.signal()
        state.unlock()
        for watcher in all { watcher.connection.cancel() }
    }

    /// The Mac closing its end is how it stops watching: nothing else comes from it.
    private func hear(_ watcher: Watcher) {
        watcher.connection.receive(minimumIncompleteLength: 1, maximumLength: 1024) { [weak self] _, _, done, error in
            guard let self else { return }
            if done || error != nil { self.remove(watcher) } else { self.hear(watcher) }
        }
    }

    private func remove(_ watcher: Watcher) {
        state.lock()
        watchers.removeAll { $0 === watcher }
        state.signal()
        state.unlock()
        watcher.connection.cancel()
    }

    private func push(_ part: Data, to watcher: Watcher) {
        state.lock()
        let now = watcher.outbox.offer(part)
        state.unlock()
        if let now { transmit(now, to: watcher) }
    }

    private func transmit(_ part: Data, to watcher: Watcher) {
        watcher.connection.send(content: part, completion: .contentProcessed { [weak self] error in
            guard let self else { return }
            if error != nil { return self.remove(watcher) }
            self.state.lock()
            watcher.heard = Date()
            let next = watcher.outbox.sent()
            self.state.unlock()
            if let next { self.transmit(next, to: watcher) }
        })
    }

    /// Take the screen, send it if it changed, wait for the next turn; until nobody watches.
    private func loop() {
        // The screen as last taken, before it was made smaller: two looks at a still screen
        // give the same bytes.
        var last: Data?
        // The screen's own width in pixels, once a frame has said.
        var native: Int?
        // The last frame sent was smaller than the screen.
        var soft = false
        var misses = 0
        var noted = Date.distantPast
        while true {
            let started = Date()
            state.lock()
            guard !watchers.isEmpty else {
                looping = false
                state.unlock()
                return
            }
            let (options, watching, via) = (self.options, self.watchers, self.via)
            let fresh = self.fresh
            self.fresh = false
            state.unlock()
            if started.timeIntervalSince(noted) >= 1 {
                noted = started
                onActivity?()
            }
            autoreleasepool {
                let by = via == .daemon ? Capture.halving(native: native ?? .max, width: options.width) : 1
                let taken: Data?
                switch via {
                case .daemon:
                    taken = Capture.daemonJPEG(
                        quality: by > 1 ? max(options.quality, Capture.sourceQuality) : options.quality)
                case .xctest:
                    taken = Capture.xctest(width: options.width, quality: options.quality)?.data
                }
                guard let raw = taken else {
                    misses += 1
                    state.lock()
                    failed += 1
                    // The daemon's JPEG isn't to be had on this iOS: the public screenshot from here on.
                    if via == .daemon, misses >= 3 {
                        self.via = .xctest
                        misses = 0
                    }
                    state.unlock()
                    return
                }
                misses = 0
                if native == nil, via == .daemon { native = Capture.size(raw)?.width }
                let jpeg: Data?
                let changed = fresh || raw != last
                if changed {
                    last = raw
                    let by = via == .daemon ? Capture.halving(native: native ?? 0, width: options.width) : 1
                    jpeg = Capture.halved(raw, by: by, quality: options.quality)
                    soft = by > 1
                } else if soft, options.sharp {
                    // It came to rest: once more, as sharp as the screen.
                    jpeg = raw
                    soft = false
                } else {
                    state.lock()
                    same += 1
                    let quiet = watching.filter {
                        !$0.outbox.sending && started.timeIntervalSince($0.heard) >= ScreenStream.stillEvery
                    }
                    state.unlock()
                    for watcher in quiet { push(ScreenStream.still, to: watcher) }
                    return
                }
                guard let jpeg else { return }
                let part = ScreenStream.part(jpeg)
                for watcher in watching { push(part, to: watcher) }
                state.lock()
                frames += 1
                if changed { pace.lively = max(pace.lively, started) }
                took.append(Date().timeIntervalSince(started))
                if took.count > 30 { took.removeFirst(took.count - 30) }
                state.unlock()
            }
            // The next look at the pace of the moment: a touch during a calm wait ends it.
            state.lock()
            while !watchers.isEmpty {
                let now = Date()
                let until = started.addingTimeInterval(pace.interval(at: now))
                if now >= until || !state.wait(until: until) { break }
            }
            state.unlock()
        }
    }
}
