import CoreGraphics
import Testing
@testable import Trek

// The reader's place in the transcript when a row's height changes: only what's above what is
// being read may move the offset.
struct TranscriptPlaceTests {
    @Test func aRowWhollyAboveTheScreenTakesTheOffsetWithIt() {
        #expect(TranscriptController.movesWhatIsRead(origin: 100, height: 200, top: 400, first: false))
        #expect(TranscriptController.movesWhatIsRead(origin: 100, height: 300, top: 400, first: false), "its foot at the top exactly")
    }

    @Test func theAnswerBeingReadGrowsAtItsFootAndStaysPut() {
        // It starts above the screen's top and runs on below it: each line that arrives makes
        // it taller at the end, under what's being read. The offset followed every one of them.
        #expect(!TranscriptController.movesWhatIsRead(origin: 100, height: 900, top: 400, first: false))
        #expect(!TranscriptController.movesWhatIsRead(origin: 399, height: 20, top: 400, first: false))
    }

    @Test func aRowComingInFromAboveAtItsEstimateKeepsWhatFollowsItStill() {
        // Scrolled up into: drawn for the first time across the top, its real height in place
        // of the estimate. What's on screen is its foot and the rows after it.
        #expect(TranscriptController.movesWhatIsRead(origin: 100, height: 900, top: 400, first: true))
    }

    @Test func aRowOnOrBelowTheScreensTopMovesNothingAboveIt() {
        #expect(!TranscriptController.movesWhatIsRead(origin: 400, height: 100, top: 400, first: true))
        #expect(!TranscriptController.movesWhatIsRead(origin: 800, height: 100, top: 400, first: false))
    }
}
