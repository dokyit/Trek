import Foundation
import Testing
@testable import Trek

/// The shimmer and a chart's skeleton sweep by the clock (`Sweep`), as the repeating animation
/// they replace did: from the start as they come in, evenly, a pass every 1.6 s.
struct SweepTests {
    private let start = Date(timeIntervalSince1970: 1_800_000_000)

    @Test func aPassBeginsAtTheStartAndRunsEvenly() {
        #expect(Sweep.period == 1.6)
        #expect(Sweep.progress(at: start, since: start) == 0)
        #expect(abs(Sweep.progress(at: start + 0.4, since: start) - 0.25) < 1e-6)
        #expect(abs(Sweep.progress(at: start + 0.8, since: start) - 0.5) < 1e-6)
        #expect(abs(Sweep.progress(at: start + 1.2, since: start) - 0.75) < 1e-6)
    }

    @Test func itComesRoundAgainEachPass() {
        #expect(abs(Sweep.progress(at: start + 3 * 1.6 + 0.4, since: start) - 0.25) < 1e-6)
        for t in stride(from: 0.0, to: 20, by: 0.05) {
            let p = Sweep.progress(at: start + t, since: start)
            #expect(p >= 0 && p < 1, "at \(t) s")
        }
    }

    @Test func aClockBehindTheStartHoldsAtTheBeginning() {
        // A sweep that began a moment from now (its start taken a frame late) waits at the left.
        #expect(Sweep.progress(at: start - 5, since: start) == 0)
    }
}

/// The beacon breathes by the clock (`Breath`), as the repeating animation it replaces did (0.8 s
/// in, 0.8 s out, eased), so its loop can't catch the list it sits in.
struct BreathTests {
    /// A moment a whole number of breaths from the clock's reference date.
    private let start = Date(timeIntervalSinceReferenceDate: 1.6 * 500_000_000)

    @Test func aBreathGoesInAndOutOverOnePeriod() {
        #expect(Breath.period == 1.6)
        #expect(Breath.level(at: start) < 1e-3, "dim as it begins")
        #expect(abs(Breath.level(at: start + 0.8) - 1) < 1e-3, "lit halfway")
        #expect(Breath.level(at: start + 1.6) < 1e-3, "dim again")
        #expect(abs(Breath.level(at: start + 0.4) - 0.5) < 1e-3, "eased: half lit a quarter in")
    }

    @Test func itStaysInRangeAndComesRound() {
        for t in stride(from: 0.0, to: 20, by: 0.05) {
            let l = Breath.level(at: start + t)
            #expect(l >= 0 && l <= 1, "at \(t) s")
            #expect(abs(l - Breath.level(at: start + t + 3 * 1.6)) < 1e-3, "the same three breaths on, at \(t) s")
        }
    }

    @Test func aTurnRunsEvenlyFromNoughtToOne() {
        // At a whole number of turns: at the top, which in floating point is a hair either side.
        let top = Breath.turn(at: start, period: 1.6)
        #expect(min(top, 1 - top) < 1e-3)
        #expect(abs(Breath.turn(at: start + 0.4, period: 1.6) - 0.25) < 1e-3)
        for t in stride(from: 0.0, to: 5, by: 0.07) {
            let p = Breath.turn(at: start + t, period: 0.9)
            #expect(p >= 0 && p < 1, "at \(t) s")
        }
    }
}
