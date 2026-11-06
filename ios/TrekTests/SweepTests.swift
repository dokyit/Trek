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
