import SwiftUI

/// What Trek says an agent is doing while it works, instead of "Thinking" (the Mac's
/// `mascot::WORDS` and `mascot::word`, the same words in the same order for the same thread).
nonisolated enum TrailWord {
    static let words = [
        "Trailblazing", "Switchbacking", "Summiting", "Scrambling", "Bushwhacking", "Route-finding",
        "Stacking cairns", "Reading the map", "Checking the compass", "Fording the creek", "Gaining elevation",
        "Traversing", "Acclimatizing", "Scouting ahead", "Marking the trail", "Crossing the ridge",
        "Breaking trail", "Boulder-hopping", "Topping out", "Following the cairns", "Charting a course",
        "Wayfinding", "Lighting the beacon", "Refilling canteens", "Taking the scenic route", "Contouring",
        "Peak-bagging", "Setting up base camp", "Lacing boots", "Checking the forecast", "Glissading",
        "Hiking it out",
    ]

    /// How long each word stays before the next.
    static let every: UInt64 = 4

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

/// The Mac's pixel hiker (`mascot.rs`): hat, ember backpack with a bedroll, walking pole, a
/// four-frame walk.
enum HikerSprite {
    static let width = 14
    static let height = 16

    static let top = [
        "......hhh.....",
        ".....hhhhh....",
        "....HHHHHHHH..",
        ".....ssses....",
        ".....sssss....",
        "..rr..ss......",
        ".bbbbcccc.....",
        ".bBbbccccC....",
        ".bBbbcccCsL...",
        ".bBbbcccC.....",
        "..bbbcccC.....",
        "....pppp......",
    ]
    /// Near leg forward, passing, far leg forward, passing.
    static let legs = [
        ["....PP.pp.....", "...PP...pp....", "...P.....p....", "..kk.....kk..."],
        ["....Pppp......", ".....Pp.......", ".....Pp.......", ".....kkk......"],
        ["....pp.PP.....", "...pp...PP....", "...p.....P....", "..kk.....kk..."],
        ["....pPPP......", ".....pP.......", ".....pP.......", ".....kkk......"],
    ]
    /// The pole from under the grip to the ground: planted ahead on contact frames.
    static let pole: [(Double, Double, Double, Double)] = [(10, 9, 13, 15), (10, 9, 11, 15), (10, 9, 13, 15), (10, 9, 11, 15)]

    static func color(_ c: Character) -> Color? {
        let hex: UInt32
        switch c {
        case "h": hex = 0xB07A45
        case "H": hex = 0x8A5A2E
        case "s": hex = 0xE9C39A
        case "e": hex = 0x2A2A2E
        case "c": hex = 0x5E8C7B
        case "C": hex = 0x4A7262
        case "b": hex = 0xFF7A3D
        case "B": hex = 0xC9551F
        case "r": hex = 0xF2E3C6
        case "p": hex = 0x55606F
        case "P": hex = 0x3A424E
        case "k": hex = 0x2B2B30
        case "l": hex = 0xA8A29E
        case "L": hex = 0x6B6763
        default: return nil
        }
        return Color(hex: hex)
    }

    /// The sprite's cells for a frame, precomputed: (column, row, colour).
    static let frames: [[(Int, Int, Color)]] = (0..<4).map { f in
        var cells: [(Int, Int, Color)] = []
        for (y, row) in (top + legs[f]).enumerated() {
            for (x, ch) in row.enumerated() {
                if let c = color(ch) { cells.append((x, y, c)) }
            }
        }
        let (x0, y0, x1, y1) = pole[f]
        let n = Int(y1 - y0)
        for i in 0...n {
            let t = Double(i) / Double(n)
            cells.append((Int((x0 + (x1 - x0) * t).rounded()), Int(y0) + i, Color(hex: 0xA8A29E)))
        }
        return cells
    }

    static func draw(_ ctx: GraphicsContext, left: CGFloat, ground: CGFloat, cell: CGFloat, frame: Int, right: Bool) {
        let top = ground - CGFloat(height) * cell
        for (x, y, color) in frames[frame] {
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
/// seconds, slowing to turn at each end; 16 frames a second, 8 steps a second. Still, it stands
/// near the start facing ahead.
struct TrailView: View {
    var id: String
    var still: Bool

    /// One lap there and back.
    static let lap = 16.0
    static let fps = 16.0
    static let steps = 8.0

    @Environment(\.displayScale) private var scale

    /// A sprite pixel snapped to whole device pixels (1.5 pt on the Mac; 5 px on a 3x screen).
    private var cell: CGFloat { (1.5 * scale).rounded(.up) / scale }
    private var height: CGFloat { CGFloat(HikerSprite.height) * cell + 4 }

    var body: some View {
        Group {
            if still {
                canvas(pos: 0.06, frame: 1, right: true)
            } else {
                TimelineView(.animation(minimumInterval: 1 / Self.fps)) { ctx in
                    let (pos, frame, right) = Self.pose(WalkClock.of(id).step(ctx.date))
                    canvas(pos: pos, frame: frame, right: right)
                }
                .onDisappear { WalkClock.of(id).pause() }
            }
        }
        .frame(height: height)
        .accessibilityHidden(true)
    }

    /// Where the hiker is (0…1 along the trail), which leg frame, and which way it faces.
    static func pose(_ clock: Double) -> (Double, Int, Bool) {
        let t = clock.truncatingRemainder(dividingBy: lap) / lap
        // Triangle wave: right for half the lap, back left for the other half.
        let (raw, right) = t < 0.5 ? (t * 2, true) : (2 - t * 2, false)
        // Eased at the ends, so it slows to a stop, turns and sets off again.
        let pos = 0.5 - 0.5 * cos(.pi * raw)
        let speed = sin(.pi * raw)
        let frame = speed < 0.15 ? 1 : Int(clock * steps) % 4
        return (pos, frame, right)
    }

    private func canvas(pos: Double, frame: Int, right: Bool) -> some View {
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
            HikerSprite.draw(ctx, left: left, ground: ground - 1, cell: cell, frame: frame, right: right)
        }
    }
}
