import Photos
import Testing
import UIKit
@testable import Trek

/// A Photos library that answers as told, and counts what it was asked.
private nonisolated final class FakeShelf: PhotoShelf, @unchecked Sendable {
    var access: PHAuthorizationStatus
    /// What the user says when asked.
    var answer: PHAuthorizationStatus
    var asked = 0
    var added = 0
    var failure: (any Error)?

    init(_ access: PHAuthorizationStatus, answer: PHAuthorizationStatus = .denied) {
        self.access = access
        self.answer = answer
    }

    func askAccess() async -> PHAuthorizationStatus {
        asked += 1
        access = answer
        return answer
    }

    func add(_ image: UIImage) async throws {
        if let failure { throw failure }
        added += 1
    }
}

private struct Full: LocalizedError {
    var errorDescription: String? { "The iPhone is out of space." }
}

/// Saving a long-pressed picture to Photos: asked once, added only on a yes, and what's said when
/// it isn't.
struct PictureKeeperTests {
    @Test func theFirstSaveAsksAndAYesAddsThePicture() async {
        let shelf = FakeShelf(.notDetermined, answer: .authorized)
        #expect(await PictureKeeper.save(UIImage(), to: shelf) == .saved)
        #expect(shelf.asked == 1)
        #expect(shelf.added == 1)
        // Allowed now: the next one isn't asked about.
        #expect(await PictureKeeper.save(UIImage(), to: shelf) == .saved)
        #expect(shelf.asked == 1)
        #expect(shelf.added == 2)
    }

    @Test func aNoAddsNothingAndIsNeverAskedAgain() async {
        let shelf = FakeShelf(.notDetermined, answer: .denied)
        #expect(await PictureKeeper.save(UIImage(), to: shelf) == .refused)
        #expect(shelf.added == 0)
        // Refused before: iOS won't ask twice, and neither does Trek.
        #expect(await PictureKeeper.save(UIImage(), to: shelf) == .refused)
        #expect(shelf.asked == 1)
        #expect(shelf.added == 0)
    }

    @Test func aRefusalPointsAtSettingsAndARestrictionDoesnt() async {
        let refused = await PictureKeeper.save(UIImage(), to: FakeShelf(.denied))
        #expect(refused == .refused)
        #expect(refused.needsSettings)
        #expect(refused.message.contains("Settings"))
        #expect(refused.message.contains("never looks at your library"), "it says what the access is for")
        // Screen Time or a profile: nothing in Settings the user can turn on.
        let restricted = await PictureKeeper.save(UIImage(), to: FakeShelf(.restricted))
        #expect(restricted == .restricted)
        #expect(!restricted.needsSettings)
        #expect(!PictureSave.saved.needsSettings)
    }

    @Test func whatPhotosCouldntKeepIsSaidInItsWords() async {
        let shelf = FakeShelf(.authorized)
        shelf.failure = Full()
        let outcome = await PictureKeeper.save(UIImage(), to: shelf)
        #expect(outcome == .failed("The iPhone is out of space."))
        #expect(outcome.message == "Photos couldn't keep that picture: The iPhone is out of space.")
        #expect(PictureSave.failed("").message == "Photos couldn't keep that picture.")
        #expect(PictureSave.saved.message == "Saved to Photos")
    }

    @Test func aLongPressOffersSaveCopyAndShare() {
        #expect(PictureAct.allCases.map(\.title) == ["Save to Photos", "Copy", "Share…"])
        #expect(PictureAct.allCases.allSatisfy { UIImage(systemName: $0.symbol) != nil }, "each has its symbol")
    }

    /// Add-only: the app says why it adds to Photos, and asks for nothing that reads the library.
    @Test func theAppAsksOnlyToAddToPhotos() {
        let why = Bundle.main.object(forInfoDictionaryKey: "NSPhotoLibraryAddUsageDescription") as? String
        #expect(why?.isEmpty == false)
        #expect(Bundle.main.object(forInfoDictionaryKey: "NSPhotoLibraryUsageDescription") == nil)
    }
}

/// The viewer's long press: what lifts, and when it's offered at all.
struct ViewerMenuTests {
    private let screen = CGRect(x: 0, y: 0, width: 390, height: 844)

    @Test func whatLiftsIsThePictureNotTheScreen() {
        // Wide: the screen's width, centred between the bars.
        let wide = ZoomableImage.Coordinator.fitted(CGSize(width: 1600, height: 900), in: screen)
        #expect(abs(wide.width - 390) < 0.01)
        #expect(abs(wide.height - 390 * 900 / 1600) < 0.01)
        #expect(abs(wide.midY - 422) < 0.01)
        // Tall: the screen's height, centred between its sides.
        let tall = ZoomableImage.Coordinator.fitted(CGSize(width: 300, height: 1688), in: screen)
        #expect(abs(tall.height - 844) < 0.01)
        #expect(abs(tall.midX - 195) < 0.01)
        // Nothing to fit: the whole view, never a rectangle of no size.
        #expect(ZoomableImage.Coordinator.fitted(.zero, in: screen) == screen)
    }

    /// The pull-down to close keeps its gesture: no menu part-way through a pull, nor once the
    /// picture is on its way out. And none for a press on the black beside the picture.
    @Test func noMenuDuringAPullOrOnTheWayOut() {
        #expect(ZoomableImage.Coordinator.offersMenu(leaving: false, pulled: 0))
        #expect(!ZoomableImage.Coordinator.offersMenu(leaving: false, pulled: 0.2))
        #expect(!ZoomableImage.Coordinator.offersMenu(leaving: true, pulled: 0))
        #expect(!ZoomableImage.Coordinator.offersMenu(leaving: false, pulled: 0, onPicture: false))
    }

    /// The menu is on the image only while nothing else has hold of it: a pull, a pan, a pinch or
    /// a swipe to the next image each take it off until they let go.
    @Test func aGestureWithHoldOfTheImageKeepsTheMenuOff() {
        typealias C = ZoomableImage.Coordinator
        #expect(C.menuOffered(holds: [], leaving: false))
        for hold in [C.Hold.pull, .page, .pan, .zoom] {
            #expect(!C.menuOffered(holds: [hold], leaving: false))
        }
        #expect(!C.menuOffered(holds: [.pull, .zoom], leaving: false))
        #expect(!C.menuOffered(holds: [], leaving: true), "on its way off the screen")
    }
}
