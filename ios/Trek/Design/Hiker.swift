import SwiftUI

/// What Trek says an agent is doing while it works, instead of "Thinking" (the Mac's
/// `mascot::WORDS` and `mascot::word`, the same words in the same order for the same thread).
nonisolated enum TrailWord {
    static let words = [
        "Breaking trail", "Reading the map", "Route-finding", "Scouting ahead", "Checking the compass",
        "Marking the trail", "Following the cairns", "Gaining elevation", "Crossing the ridge",
        "Charting a course", "Stacking cairns", "Traversing", "Wayfinding", "Trailblazing",
    ]

    /// How long each word stays before the next.
    static let every: UInt64 = 6

    /// The word for a turn that has run `secs` seconds: it changes every `every` seconds, and
    /// `seed` (the thread id) keeps threads from moving in lockstep. Each thread strides through
    /// the list from its own start by a stride coprime with its length, so every word comes round
    /// once a lap and never twice in a row.
    static func word(_ seed: String, _ secs: UInt64) -> String {
        let n = UInt64(words.count)
        var h: UInt64 = 0xcbf2_9ce4_8422_2325
        for b in seed.utf8 { h = (h ^ UInt64(b)) &* 0x100_0000_01b3 }
        func gcd(_ a: UInt64, _ b: UInt64) -> UInt64 {
            var (a, b) = (a, b)
            while b != 0 { (a, b) = (b, a % b) }
            return a
        }
        var stride: UInt64 = 1
        for k in 1..<n {
            let s = ((h >> 7) &+ k) % n
            if s > 1 && gcd(s, n) == 1 { stride = s; break }
        }
        let i = (h % n + (secs / every) % n * stride) % n
        return words[Int(i)]
    }

    /// "Breaking trail…"
    static func line(_ seed: String, since: Int64?, now: Date) -> String {
        let secs = since.map { UInt64(max(0, now.timeIntervalSince1970 - Double($0) / 1000)) } ?? 0
        return word(seed, secs) + "…"
    }
}

/// The Mac's pixel hiker (`trek-mascot`): hat, ember pack, a four-pose walk that bobs a pixel on
/// each contact, and a standing pose. Eight colours, each with a dark and a light value.
enum HikerSprite {
    static let width = 12
    static let height = 16
    /// The standing pose; the walk is the four after it.
    static let stand = 0

    static let poses: [[String]] = [
        ["....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....", "EEEEKKKK....",
         "EEEEKKKKK...", "EeEEKKKKs...", "EeEEKKKK....", ".EEEKKKK....", "....KKKK....", "....KKK.....",
         "....KKK.....", "....KKK.....", "....KKK.....", "....bbbb...."],
        ["............", "....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....",
         "EEEEKKKK....", "EEEEKKKK....", "EeEEKKKK....", "EeEEsKKK....", ".EEEKKKK....", "....KKKK....",
         "...kkKKKK...", "..kk...KK...", ".kk.....KK..", ".b......bbb."],
        ["....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....", "EEEEKKKK....",
         "EEEEKKKKK...", "EeEEKKKKs...", "EeEEKKKK....", ".EEEKKKK....", "....KKKK....", "....KKkk....",
         "....KK.kk...", "....KK..k...", "....KK..bb..", "....bbb....."],
        ["............", "....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....",
         "EEEEKKKK....", "EEEEKKKKK...", "EeEEKKKKKs..", "EeEEKKKK....", ".EEEKKKK....", "....KKKK....",
         "...KKkkkk...", "..KK...kk...", ".KK.....kk..", ".b......bbb."],
        ["....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....", "EEEEKKKK....",
         "EEEEKKKKK...", "EeEEKKKKs...", "EeEEKKKK....", ".EEEKKKK....", "....kkKK....", "....kkKK....",
         "....kk.KK...", "....kk..K...", "....kk..bb..", "....bbb....."],
    ]

    /// `h` hat, `s` skin, `o` eye, `K` near side, `k` far leg, `b` boots, `E` the ember pack and
    /// `e` its shade: (dark, light).
    static func color(_ c: Character, dark: Bool) -> Color? {
        let pair: (UInt32, UInt32)
        switch c {
        case "h": pair = (0xC9A47C, 0xB58A5C)
        case "s": pair = (0xEBC6A0, 0xE9BF95)
        case "o": pair = (0x1A1A1C, 0x1A1A1C)
        case "K": pair = (0x9A9AA2, 0x55555B)
        case "k": pair = (0x5C5C63, 0x9C9A95)
        case "b": pair = (0x8A5A2E, 0x6B4423)
        case "E": pair = (0xFF7A3D, 0xE85D1F)
        case "e": pair = (0xC9551F, 0xB8461A)
        default: return nil
        }
        return Color(hex: dark ? pair.0 : pair.1)
    }

    /// Each pose's cells, precomputed for both looks: (column, row, colour).
    private static func cells(dark: Bool) -> [[(Int, Int, Color)]] {
        poses.map { pose in
            var cells: [(Int, Int, Color)] = []
            for (y, row) in pose.enumerated() {
                for (x, ch) in row.enumerated() {
                    if let c = color(ch, dark: dark) { cells.append((x, y, c)) }
                }
            }
            return cells
        }
    }
    private static let darkCells = cells(dark: true)
    private static let lightCells = cells(dark: false)

    static func draw(_ ctx: GraphicsContext, left: CGFloat, ground: CGFloat, cell: CGFloat, pose: Int, right: Bool, dark: Bool) {
        let top = ground - CGFloat(height) * cell
        for (x, y, color) in (dark ? darkCells : lightCells)[pose] {
            let col = right ? x : width - 1 - x
            ctx.fill(Path(CGRect(x: left + CGFloat(col) * cell, y: top + CGFloat(y) * cell, width: cell, height: cell)), with: .color(color))
        }
    }
}

/// The hiker's own clock per thread: seconds walked. It moves only while the hiker is on screen
/// and walking, so after a pause (another screen, Reduce Motion) it carries on from where it
/// stood rather than jumping to where the turn's clock would put it.
@MainActor
final class WalkClock {
    private static var clocks: [String: WalkClock] = [:]
    static func of(_ id: String) -> WalkClock {
        if let c = clocks[id] { return c }
        let c = WalkClock()
        clocks[id] = c
        return c
    }

    private var walked: Double = 0
    private var last: Date?

    /// Steps the clock to `now` (at most a few frames' worth at once) and returns seconds walked.
    func step(_ now: Date) -> Double {
        let dt = last.map { min(max(now.timeIntervalSince($0), 0), 0.2) } ?? 0
        last = now
        walked += dt
        return walked
    }

    func pause() { last = nil }
}

/// A dotted trail the width of its parent with the hiker walking it, there and back over 16
/// seconds, slowing to turn at each end; 16 frames a second, a pose every 4 art pixels it covers
/// so its feet stay planted, standing while it turns. Still, it stands near the start facing
/// ahead.
struct TrailView: View {
    var id: String
    var still: Bool

    /// One lap there and back.
    static let lap = 16.0
    static let fps = 16.0
    /// Art pixels covered per pose.
    static let step = 4.0

    @Environment(\.displayScale) private var scale
    @Environment(\.colorScheme) private var scheme

    /// A sprite pixel snapped to whole device pixels (1.5 pt on the Mac; 5 px on a 3x screen).
    private var cell: CGFloat { (1.5 * scale).rounded(.up) / scale }
    private var height: CGFloat { CGFloat(HikerSprite.height) * cell + 4 }

    var body: some View {
        Group {
            if still {
                canvas(pos: 0.06, walking: false, right: true)
            } else {
                TimelineView(.animation(minimumInterval: 1 / Self.fps)) { ctx in
                    let (pos, walking, right) = Self.pose(WalkClock.of(id).step(ctx.date))
                    canvas(pos: pos, walking: walking, right: right)
                }
                .onDisappear { WalkClock.of(id).pause() }
            }
        }
        .frame(height: height)
        .accessibilityHidden(true)
    }

    /// Where the hiker is (0…1 along the trail), whether it's in stride, and which way it faces.
    static func pose(_ clock: Double) -> (Double, Bool, Bool) {
        let t = clock.truncatingRemainder(dividingBy: lap) / lap
        // Triangle wave: right for half the lap, back left for the other half.
        let (raw, right) = t < 0.5 ? (t * 2, true) : (2 - t * 2, false)
        // Eased at the ends, so it slows to a stop, turns and sets off again.
        let pos = 0.5 - 0.5 * cos(.pi * raw)
        let speed = sin(.pi * raw)
        return (pos, speed >= 0.15, right)
    }

    private func canvas(pos: Double, walking: Bool, right: Bool) -> some View {
        Canvas { ctx, size in
            let ground = (size.height - 2).rounded()
            var x: CGFloat = 2
            while x < size.width - 2 {
                ctx.fill(Path(roundedRect: CGRect(x: x, y: ground - 1, width: 2, height: 2), cornerRadius: 1),
                         with: .color(Trek.foreground.opacity(0.16)))
                x += 7
            }
            let spriteW = CGFloat(HikerSprite.width) * cell
            let travel = max(size.width - spriteW - 8, 0)
            let left = ((4 + travel * pos) / cell).rounded() * cell
            // A pose for every few pixels covered: the legs keep pace with the ground.
            let pose = walking ? 1 + Int(travel * pos / cell / Self.step) % 4 : HikerSprite.stand
            HikerSprite.draw(ctx, left: left, ground: ground - 1, cell: cell, pose: pose, right: right, dark: scheme == .dark)
        }
    }
}
