import Testing
import UIKit
@testable import Trek

/// Rows going in and out of the transcript list never leave UIKit with counts that don't add up.
/// On 21 November 2026 (0.4.63, a Simulator) the app stopped in `performBatchUpdates`
/// (`_Bug_Detected_In_Client_Of_UICollectionView_Invalid_Batch_Updates`). The crash report doesn't
/// say which counts were off, and these cases (a stale layout, let-go data, no window) don't bring
/// it back on the code before the fix either: they hold the list to UIKit's contract (the rows
/// change inside the batch) so a change that breaks it shows here, not on a phone.
@MainActor
struct TranscriptBatchTests {
    private let store = ThreadStore(id: "h:t1", host: "h", summary: nil)
    private let model = AppModel()

    /// What the rows draw with, as the thread view hands it over.
    private func environment() -> TranscriptEnvironment {
        TranscriptEnvironment(model: model, projectHue: nil, liveThreadID: nil, calm: true, compact: false,
                              openThread: nil, dynamicTypeSize: .large)
    }

    private func host() -> (TranscriptController, UIWindow, UICollectionView) {
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 393, height: 800))
        let c = TranscriptController()
        window.rootViewController = c
        window.makeKeyAndVisible()
        c.loadViewIfNeeded()
        c.setEnvironment(environment())
        c.view.frame = window.bounds
        c.view.layoutIfNeeded()
        let cv = c.view.subviews.compactMap { $0 as? UICollectionView }.first!
        return (c, window, cv)
    }

    private var short: [TranscriptRow] { [.header(store), .footer(store)] }
    private var long: [TranscriptRow] { [.header(store), .loading(waiting: nil), .earlier(loading: false), .footer(store)] }

    /// The list's layout isn't up to date when rows come in: the collection view's own data was
    /// let go (`reloadData`) and no layout has run since. Batched against the rows it then
    /// counts, the update must still add up.
    @Test func rowsInWhileTheLayoutIsStale() {
        let (c, window, cv) = host()
        c.update(rows: short, generation: 1)
        cv.reloadData()
        c.update(rows: long, generation: 1)
        cv.layoutIfNeeded()
        #expect(cv.numberOfItems(inSection: 0) == 4)
        cv.reloadData()
        c.update(rows: short, generation: 1)
        cv.layoutIfNeeded()
        #expect(cv.numberOfItems(inSection: 0) == 2)
        _ = window
    }

    /// The same, with the layout invalidated rather than the data, and the swap of rows of the
    /// same count (one out, one in).
    @Test func rowsSwappedWhileTheLayoutIsInvalid() {
        let (c, window, cv) = host()
        c.update(rows: long, generation: 1)
        cv.collectionViewLayout.invalidateLayout()
        cv.setNeedsLayout()
        c.update(rows: [.header(store), .loading(waiting: nil), .footer(store)], generation: 1)
        cv.collectionViewLayout.invalidateLayout()
        c.update(rows: [.header(store), .earlier(loading: true), .footer(store)], generation: 1)
        cv.layoutIfNeeded()
        #expect(cv.numberOfItems(inSection: 0) == 3)
        _ = window
    }

    /// Off screen (the thread pushed away, the app in the background): updates still add up.
    @Test func rowsInWithNoWindow() {
        let c = TranscriptController()
        c.loadViewIfNeeded()
        c.setEnvironment(environment())
        c.update(rows: short, generation: 1)
        c.update(rows: long, generation: 1)
        c.update(rows: short, generation: 1)
        c.update(rows: long, generation: 1)
        let cv = c.view.subviews.compactMap { $0 as? UICollectionView }.first!
        cv.layoutIfNeeded()
        #expect(cv.numberOfItems(inSection: 0) == 4)
    }
}
