import Foundation
import QuartzCore

/// `-TrekScrollProbe YES`: the transcript writes a line per scroll frame to
/// `Library/Caches/scroll-probe.tsv` in the app's container (time, content offset, content
/// height, the row at the top of the screen, its index and its top's distance from the screen's
/// top, the rows on screen, whether a finger is on it), so a scripted scroll can be checked for
/// jumps the swipe doesn't explain and for blank screens (`ios/scripts/scroll-probe.py`).
enum ScrollProbe {
    static let on = UserDefaults.standard.bool(forKey: "TrekScrollProbe")
    nonisolated(unsafe) private static var lines: [String] = []
    nonisolated(unsafe) private static var flushing = false
    static let url: URL = {
        let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let url = dir.appendingPathComponent("scroll-probe.tsv")
        try? FileManager.default.removeItem(at: url)
        return url
    }()

    /// One frame: `top` is the id of the first row whose bottom is below the screen's top, `index`
    /// its place in the transcript, `topY` where its top is on screen (negative: above it).
    static func frame(offset: CGFloat, contentHeight: CGFloat, top: String?, index: Int?, topY: CGFloat?, visible: Int,
                      touching: Bool, rows: Int, motion: CGFloat? = nil) {
        guard on else { return }
        let t = CACurrentMediaTime()
        let line = String(format: "%.4f\t%.1f\t%.1f\t%@\t%d\t%.1f\t%d\t%d\t%d\t%@", t, offset, contentHeight, top ?? "-", index ?? -1,
                          topY ?? 0, visible, touching ? 1 : 0, rows, motion.map { String(format: "%.1f", $0) } ?? "-")
        lines.append(line)
        guard !flushing else { return }
        flushing = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { flush() }
    }

    /// Something else worth seeing beside the frames (a page landing, a reset).
    static func note(_ text: String) {
        guard on else { return }
        lines.append(String(format: "%.4f\t#\t%@", CACurrentMediaTime(), text))
    }

    private static func flush() {
        flushing = false
        guard !lines.isEmpty else { return }
        let chunk = lines.joined(separator: "\n") + "\n"
        lines = []
        if let h = try? FileHandle(forWritingTo: url) {
            h.seekToEndOfFile()
            h.write(Data(chunk.utf8))
            try? h.close()
        } else {
            try? Data(chunk.utf8).write(to: url)
        }
    }
}
