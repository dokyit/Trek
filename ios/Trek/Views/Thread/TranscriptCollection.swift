import SwiftUI
import UIKit
import os

private let scrollLog = Logger(subsystem: "dev.trek.TrekMobile", category: "scroll")

// MARK: The transcript's list, in UIKit
//
// A SwiftUI `LazyVStack` guesses the height of every row it hasn't drawn and corrects the scroll
// offset as rows come into view and turn out taller or shorter. With rows as uneven as a real
// transcript's (one-line notes, answers several screens long, code, tables), the corrections
// showed as the transcript jumping a page or more, or to near the end with blank space under the
// last message. Here the list is a `UICollectionView` with a layout of its own: each row's height
// is measured once (by id, at the list's width) and kept; a row measured above the screen moves
// the offset by as much as it grew, so what's on screen stays where it is; a page of earlier rows
// keeps the row being read in place; at the latest, the bottom stays at the latest.

/// What the rows see of the SwiftUI environment around the transcript (a hosting controller in a
/// cell starts with an environment of its own).
struct TranscriptEnvironment {
    var model: AppModel
    var projectHue: Int?
    var liveThreadID: String?
    var calm: Bool
    var compact: Bool
    var openThread: OpenThreadAction?
    var dynamicTypeSize: DynamicTypeSize

    func apply(_ view: some View) -> some View {
        view
            .environment(model)
            .environment(\.projectHue, projectHue)
            .environment(\.liveThreadID, liveThreadID)
            .environment(\.trekCalm, calm)
            .environment(\.compactRows, compact)
            .environment(\.openThread, openThread)
            .dynamicTypeSize(dynamicTypeSize)
    }

    /// What changes every row's height.
    var sizing: DynamicTypeSize { dynamicTypeSize }
}

/// Where the reader is, for the views around the list.
struct TranscriptEdges: Equatable {
    /// Within a couple of screens of the top.
    var nearTop = false
    /// More than a screen above the latest.
    var awayFromBottom = false
}

/// How the SwiftUI side asks the list to move.
@MainActor
final class TranscriptScroller {
    fileprivate weak var controller: TranscriptController?
    /// Back to the latest (the jump-down button).
    func scrollToLatest() { controller?.scrollToLatest(animated: true) }
}

struct TranscriptCollection: UIViewControllerRepresentable {
    var rows: [TranscriptRow]
    /// Bumped when the transcript was replaced whole: drawn afresh, at the latest.
    var generation: Int
    var environment: TranscriptEnvironment
    var scroller: TranscriptScroller
    var loadEarlier: () -> Void
    var edges: (TranscriptEdges) -> Void
    /// What covers the list at its top and bottom (the bars, the composer, the keyboard): the
    /// rows scroll under them, and stop clear of them at either end.
    var insets: EdgeInsets

    func makeUIViewController(context: Context) -> TranscriptController {
        TranscriptController()
    }

    func updateUIViewController(_ c: TranscriptController, context: Context) {
        scroller.controller = c
        c.loadEarlier = loadEarlier
        c.edgesChanged = edges
        c.setEnvironment(environment)
        c.setInsets(top: insets.top, bottom: insets.bottom)
        // A change made with an animation (a group opened) grows or shrinks its row in step.
        let t = context.transaction
        c.update(rows: rows, generation: generation, animated: t.animation != nil && !t.disablesAnimations)
    }
}

// MARK: Controller

final class TranscriptController: UIViewController, UICollectionViewDataSource, UICollectionViewDelegate {
    var loadEarlier: () -> Void = {}
    var edgesChanged: (TranscriptEdges) -> Void = { _ in }

    private(set) var rows: [TranscriptRow] = []
    /// Each row's height as laid out (measured, or a guess until it is), by position.
    fileprivate(set) var heights: [CGFloat] = []
    /// The space above each row (when it isn't empty).
    fileprivate(set) var gaps: [CGFloat] = []
    /// Heights measured so far, by row id, at `measuredWidth`.
    private var measured: [String: CGFloat] = [:]
    private var measuredWidth: CGFloat = 0
    private var index: [String: Int] = [:]
    private var generation: Int?
    private var environment: TranscriptEnvironment?

    private var collectionView: TranscriptCollectionView!
    private var layout: TranscriptLayout!
    /// At the latest: what's added or grows keeps the bottom at the latest.
    private var pinned = true
    /// The reader has scrolled since the thread opened (before that it settles at the bottom).
    private var userScrolled = false
    private var edges = TranscriptEdges()
    private var lastOffset: CGFloat = 0

    override func viewDidLoad() {
        super.viewDidLoad()
        layout = TranscriptLayout()
        layout.source = self
        let cv = TranscriptCollectionView(frame: view.bounds, collectionViewLayout: layout)
        cv.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        cv.backgroundColor = .clear
        cv.dataSource = self
        cv.delegate = self
        cv.register(TranscriptCell.self, forCellWithReuseIdentifier: TranscriptCell.reuse)
        cv.keyboardDismissMode = .interactive
        cv.alwaysBounceVertical = true
        cv.contentInsetAdjustmentBehavior = .never
        cv.selfSizingInvalidation = .enabled
        // The list keeps its rows' heights itself: no prefetching of cells it would measure twice.
        cv.isPrefetchingEnabled = false
        cv.didLayout = { [weak self] in self?.afterLayout() }
        cv.insetsChanged = { [weak self] in self?.afterInsetsChange() }
        view.addSubview(cv)
        collectionView = cv
        if ScrollProbe.on { CADisplayLink(target: self, selector: #selector(probeTick)).add(to: .main, forMode: .common) }
        #if DEBUG
        if ScrollProbe.autoScroll > 0 {
            DispatchQueue.main.asyncAfter(deadline: .now() + 4) { [weak self] in self?.startDriving() }
        }
        #endif
        view.backgroundColor = .clear
    }

    func setInsets(top: CGFloat, bottom: CGFloat) {
        guard isViewLoaded else { return }
        let insets = UIEdgeInsets(top: top, left: 0, bottom: bottom, right: 0)
        guard collectionView.contentInset != insets else { return }
        // Scrolled up, the row at the top stays put as the bars or the keyboard change.
        let anchor = pinned ? nil : captureAnchor()
        collectionView.contentInset = insets
        collectionView.verticalScrollIndicatorInsets = insets
        settle(anchor)
    }

    func setEnvironment(_ env: TranscriptEnvironment) {
        let resize = environment.map { $0.sizing != env.sizing } ?? false
        environment = env
        guard resize, isViewLoaded else { return }
        // Every row's height changes with the text size: measure again, keeping the reader's row.
        let anchor = pinned ? nil : captureAnchor()
        measured.removeAll()
        estimates.removeAll()
        heights = rows.map { estimate($0) }
        collectionView.reloadData()
        collectionView.layoutIfNeeded()
        settle(anchor)
    }

    // MARK: Rows in

    func update(rows new: [TranscriptRow], generation g: Int, animated: Bool = false) {
        guard isViewLoaded else { return }
        if g != generation {
            // A new thread, or one the host rewrote: drawn afresh at the latest.
            let first = generation == nil
            generation = g
            rows = new
            reindex()
            heights = new.map { measured[$0.id] ?? estimate($0) }
            pinned = true
            if !first { userScrolled = false }
            collectionView.reloadData()
            collectionView.layoutIfNeeded()
            scrollToBottom()
            ScrollProbe.note("reset: \(new.count) rows")
            return
        }
        let old = rows
        if old.count == new.count, zip(old, new).allSatisfy({ $0.id == $1.id }) {
            // The same rows: redraw the ones that changed (an answer being written, a group opened).
            let changed = (0..<new.count).filter { old[$0] != new[$0] }
            rows = new
            reconfigure(changed)
            if animated, !changed.isEmpty { animateResize() }
            return
        }
        // Rows went in or out: the rows both lists start with and end with stay; what's between
        // them is replaced. A page of earlier rows is all "between" at the start, new rows at the
        // end; a turn's rows rebuilt at the end likewise.
        var p = 0
        while p < old.count, p < new.count, old[p].id == new[p].id { p += 1 }
        var s = 0
        while s < old.count - p, s < new.count - p, old[old.count - 1 - s].id == new[new.count - 1 - s].id { s += 1 }
        let anchor = pinned ? nil : captureAnchor()
        rows = new
        reindex()
        heights = new.map { measured[$0.id] ?? estimate($0) }
        let deleted = (p..<(old.count - s)).map { IndexPath(item: $0, section: 0) }
        let inserted = (p..<(new.count - s)).map { IndexPath(item: $0, section: 0) }
        if old.isEmpty || deleted.count + inserted.count > 3000 {
            collectionView.reloadData()
        } else {
            UIView.performWithoutAnimation {
                collectionView.performBatchUpdates {
                    collectionView.deleteItems(at: deleted)
                    collectionView.insertItems(at: inserted)
                }
            }
            // Rows kept at either end whose content changed.
            var changed: [Int] = []
            for i in 0..<p where old[i] != new[i] { changed.append(i) }
            for k in 0..<s where old[old.count - 1 - k] != new[new.count - 1 - k] { changed.append(new.count - 1 - k) }
            reconfigure(changed)
        }
        collectionView.layoutIfNeeded()
        settle(anchor)
        // The rows at the new offset drawn now, not at the next pass.
        collectionView.layoutIfNeeded()
        if inserted.count > 0, p <= 2 { ScrollProbe.note("\(inserted.count) rows in above, kept \(anchor?.id ?? "the bottom")") }
        checkEarlier()
    }

    private func reindex() {
        index.removeAll(keepingCapacity: true)
        for (i, r) in rows.enumerated() { index[r.id] = i }
        gaps = rows.map(\.gapBefore)
    }

    /// Redraw rows whose content changed: the ones on screen now (the others are drawn, and
    /// measured, when they come into view).
    private func reconfigure(_ changed: [Int]) {
        guard !changed.isEmpty else { return }
        let visible = Set(collectionView.indexPathsForVisibleItems.map(\.item))
        let paths = changed.filter { visible.contains($0) }.map { IndexPath(item: $0, section: 0) }
        guard !paths.isEmpty else { return }
        UIView.performWithoutAnimation { collectionView.reconfigureItems(at: paths) }
    }

    /// The rows just redrawn take their new heights, and the rows under them move, over a moment
    /// (the redrawn rows clip what they don't have room for yet).
    private func animateResize() {
        let cells = collectionView.visibleCells
        cells.forEach { $0.clipsToBounds = true }
        UIView.animate(withDuration: 0.24, delay: 0, options: [.curveEaseInOut, .allowUserInteraction, .beginFromCurrentState]) {
            self.collectionView.layoutIfNeeded()
        } completion: { _ in
            cells.forEach { $0.clipsToBounds = false }
        }
    }

    // MARK: Heights

    /// Guesses made so far, by row id: a page landing guesses only its own rows (a guess reads
    /// the row's text).
    private var estimates: [String: CGFloat] = [:]

    private func estimate(_ row: TranscriptRow) -> CGFloat {
        if let e = estimates[row.id] { return e }
        let e = row.estimatedHeight(width: max(200, collectionView?.bounds.width ?? 400))
        estimates[row.id] = e
        return e
    }

    /// A row's measured height: kept, and, for a row that starts above the screen's top, the
    /// offset to move by so what's on screen doesn't move (nil: none).
    fileprivate func measured(_ h: CGFloat, at i: Int) -> CGFloat? {
        guard i < rows.count else { return nil }
        let old = heights[i]
        heights[i] = h
        measured[rows[i].id] = h
        guard !pinned, let cv = collectionView else { return nil }
        var delta = h - old
        // A row going from empty to drawn (or back) takes (or gives back) the gap before it too.
        if (old == 0) != (h == 0) { delta += h > 0 ? gaps[i] : -gaps[i] }
        let top = cv.contentOffset.y + cv.adjustedContentInset.top
        let above = layout.origin(of: i) < top - 0.5
        if ScrollProbe.on, abs(delta) > 0.5 { ScrollProbe.note("measured \(rows[i].id): \(Int(old))→\(Int(h))\(above ? ", above the screen: offset \(Int(delta))" : "")") }
        return above ? delta : nil
    }

    // MARK: Keeping the reader's place

    private struct Anchor {
        var id: String
        /// Its top's distance from the content offset.
        var offset: CGFloat
    }

    /// The first row of the transcript proper on screen (not the marker above it).
    private func captureAnchor() -> Anchor? {
        guard let cv = collectionView, !rows.isEmpty else { return nil }
        let top = cv.contentOffset.y + cv.adjustedContentInset.top
        var i = layout.firstIndex(endingBelow: top)
        while i < rows.count, rows[i].isMarker || heights[i] == 0 { i += 1 }
        guard i < rows.count else { return nil }
        return Anchor(id: rows[i].id, offset: layout.origin(of: i) - cv.contentOffset.y)
    }

    /// After rows changed: the anchor row back where it was, or the bottom at the latest.
    private func settle(_ anchor: Anchor?) {
        if pinned { return scrollToBottom() }
        guard let anchor, let i = index[anchor.id], let cv = collectionView else { return }
        let y = layout.origin(of: i) - anchor.offset
        let clamped = min(max(y, minOffset), max(minOffset, maxOffset))
        if abs(cv.contentOffset.y - clamped) > 0.25 { cv.contentOffset.y = clamped }
    }

    private var minOffset: CGFloat { -collectionView.adjustedContentInset.top }
    private var maxOffset: CGFloat {
        collectionView.contentSize.height + collectionView.adjustedContentInset.bottom - collectionView.bounds.height
    }

    private func scrollToBottom() {
        guard let cv = collectionView else { return }
        let y = max(minOffset, maxOffset)
        if abs(cv.contentOffset.y - y) > 0.25 { cv.contentOffset.y = y }
    }

    func scrollToLatest(animated: Bool) {
        guard let cv = collectionView else { return }
        pinned = true
        // From far up, most of the way in one step (rows on the way needn't be drawn), then glide.
        if cv.contentOffset.y < maxOffset - 3 * cv.bounds.height {
            cv.contentOffset.y = maxOffset - cv.bounds.height
            cv.layoutIfNeeded()
        }
        cv.setContentOffset(CGPoint(x: cv.contentOffset.x, y: max(minOffset, maxOffset)), animated: animated)
    }

    private var touching: Bool {
        collectionView.isTracking || collectionView.isDragging || collectionView.isDecelerating || driving
    }

    /// `-TrekAutoScroll <n>` (debug builds): scrolls up by itself, as a reader would, where no
    /// touches can be scripted (a real iPhone).
    private var driving = false
    #if DEBUG
    private var drive: (link: CADisplayLink, frame: Int, left: Int)?

    private func startDriving() {
        let link = CADisplayLink(target: self, selector: #selector(driveTick))
        link.add(to: .main, forMode: .common)
        drive = (link, 0, ScrollProbe.autoScroll)
        driving = true
        userScrolled = true
        ScrollProbe.note("auto-scroll: \(ScrollProbe.autoScroll) swipes")
    }

    /// One swipe: up for 45 frames at a speed that eases in and out (a page and a half), then a
    /// rest of 30 frames. Each frame moves from where the list is now, so the list's own
    /// corrections (a row measured above, a page landing) stand, as they do under a finger.
    @objc private func driveTick() {
        guard var d = drive, let cv = collectionView else { return }
        let f = d.frame % 75
        if f < 45 {
            let t = Double(f) / 45
            let speed = 70 * sin(t * .pi)
            cv.contentOffset.y = max(minOffset, cv.contentOffset.y - speed)
            pinned = false
        }
        d.frame += 1
        if d.frame % 75 == 0 { d.left -= 1 }
        if d.left <= 0 || cv.contentOffset.y <= minOffset + 0.5 && !edgesNearTopLoading {
            d.link.invalidate()
            drive = nil
            driving = false
            ScrollProbe.note("auto-scroll done")
            return
        }
        drive = d
        updateEdges()
    }

    /// At the top with earlier rows still on their way: wait for them rather than stop.
    private var edgesNearTopLoading: Bool { rows.contains { if case .earlier = $0 { true } else { false } } }
    #endif

    private func afterLayout() {
        if layout.width != measuredWidth {
            // Another width: every height changes. Measure again, keeping the reader's row.
            let first = measuredWidth == 0
            measuredWidth = layout.width
            if !first {
                let anchor = pinned ? nil : captureAnchor()
                measured.removeAll()
                estimates.removeAll()
                heights = rows.map { estimate($0) }
                layout.invalidateLayout()
                collectionView.layoutIfNeeded()
                settle(anchor)
            }
        }
        if pinned, !touching { scrollToBottom() }
        updateEdges()
    }

    private func afterInsetsChange() {
        // The composer grew or the keyboard came up: at the latest, stay there.
        if pinned { scrollToBottom() }
    }

    private func updateEdges() {
        guard let cv = collectionView else { return }
        let offset = cv.contentOffset.y + cv.adjustedContentInset.top
        let fromBottom = cv.contentSize.height + cv.adjustedContentInset.bottom - cv.contentOffset.y - cv.bounds.height
        let e = TranscriptEdges(nearTop: offset < 1400, awayFromBottom: fromBottom > 520)
        if e != edges {
            edges = e
            edgesChanged(e)
        }
        checkEarlier()
    }

    /// Earlier items load as the reader scrolls up towards them, or straight away while what's
    /// loaded doesn't fill the screen; not while the list settles at the bottom.
    private func checkEarlier() {
        guard let cv = collectionView, !rows.isEmpty else { return }
        let short = cv.contentSize.height < cv.bounds.height - cv.adjustedContentInset.top - cv.adjustedContentInset.bottom
        if short || (edges.nearTop && userScrolled) { loadEarlier() }
    }

    // MARK: Data source and delegate

    func numberOfSections(in collectionView: UICollectionView) -> Int { 1 }

    func collectionView(_ collectionView: UICollectionView, numberOfItemsInSection section: Int) -> Int { rows.count }

    func collectionView(_ collectionView: UICollectionView, cellForItemAt indexPath: IndexPath) -> UICollectionViewCell {
        let cell = collectionView.dequeueReusableCell(withReuseIdentifier: TranscriptCell.reuse, for: indexPath) as! TranscriptCell
        let row = rows[indexPath.item]
        // The row says when it grows or shrinks by itself (the working footer's sub-agents card
        // coming in, a card's state moving on): measured again, so it never draws over the rows
        // around it or under the composer.
        let content = row.padding(.horizontal, 20)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { [weak cell] h in cell?.contentHeight(h) }
            .id(row.id)
        let view = environment.map { AnyView($0.apply(content)) } ?? AnyView(content)
        cell.configure(view, id: row.id, parent: self)
        return cell
    }

    func scrollViewWillBeginDragging(_ scrollView: UIScrollView) {
        if !userScrolled {
            userScrolled = true
            checkEarlier()
        }
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        let cv = collectionView!
        let y = cv.contentOffset.y
        if touching {
            // The reader moved it: at the latest (or within a few points of it), it stays there.
            pinned = maxOffset - y < 32
        } else if abs(y - lastOffset) > 700 {
            scrollLog.info("transcript moved \(Int(y - self.lastOffset)) pt by itself, content \(Int(cv.contentSize.height)), pinned \(self.pinned), \(self.rows.count) rows")
        }
        lastOffset = y
        updateEdges()
    }

    // MARK: Probe

    private var probeLast: (id: String, y: CGFloat)?

    /// Once a frame, what's on screen (`-TrekScrollProbe YES`): the row at the top, where it is,
    /// and how far the row that was at the top last frame moved on screen since (`motion`).
    @objc private func probeTick() {
        guard let cv = collectionView, isViewLoaded, view.window != nil else { return }
        let top = cv.contentOffset.y + cv.adjustedContentInset.top
        var i = layout.firstIndex(endingBelow: top)
        while i < rows.count, heights[i] == 0 { i += 1 }
        // Rows with something drawn on screen (a blank screen has none).
        let bottom = cv.contentOffset.y + cv.bounds.height - cv.adjustedContentInset.bottom
        let visible = cv.indexPathsForVisibleItems.filter { ip in
            let i = ip.item
            return i < heights.count && heights[i] > 0 && layout.origin(of: i) < bottom && layout.origin(of: i) + heights[i] > top
        }.count
        var motion: CGFloat?
        if let last = probeLast, let j = index[last.id] { motion = layout.origin(of: j) - top - last.y }
        probeLast = i < rows.count ? (rows[i].id, layout.origin(of: i) - top) : nil
        ScrollProbe.frame(offset: cv.contentOffset.y, contentHeight: cv.contentSize.height, top: i < rows.count ? rows[i].id : nil,
                          index: i < rows.count ? i : nil, topY: i < rows.count ? layout.origin(of: i) - top : nil, visible: visible,
                          touching: touching, rows: rows.count, motion: motion)
    }
}

// MARK: Collection view

final class TranscriptCollectionView: UICollectionView {
    var didLayout: () -> Void = {}
    var insetsChanged: () -> Void = {}

    override func layoutSubviews() {
        super.layoutSubviews()
        didLayout()
    }

    override func adjustedContentInsetDidChange() {
        super.adjustedContentInsetDidChange()
        insetsChanged()
    }
}

// MARK: Layout

/// Rows one under another, `spacing` apart (or as close as the row asks), each as tall as it
/// measured (a guess until then).
final class TranscriptLayout: UICollectionViewLayout {
    static let spacing: CGFloat = 18
    static let top: CGFloat = 12
    static let bottom: CGFloat = 16

    weak var source: TranscriptController?
    private var ys: [CGFloat] = []
    private var hs: [CGFloat] = []
    private var height: CGFloat = 0
    private(set) var width: CGFloat = 0

    override func prepare() {
        super.prepare()
        guard let cv = collectionView, let source else { return }
        width = cv.bounds.width
        hs = source.heights
        let gaps = source.gaps
        ys = [CGFloat](repeating: 0, count: hs.count)
        var y = Self.top
        var placed = false
        for i in 0..<hs.count {
            if hs[i] > 0 {
                if placed { y += i < gaps.count ? gaps[i] : Self.spacing }
                ys[i] = y
                y += hs[i]
                placed = true
            } else {
                ys[i] = y
            }
        }
        height = y + Self.bottom
    }

    override var collectionViewContentSize: CGSize { CGSize(width: width, height: height) }

    func origin(of i: Int) -> CGFloat { i < ys.count ? ys[i] : height }

    /// The first row whose bottom is below `y`.
    func firstIndex(endingBelow y: CGFloat) -> Int {
        var lo = 0, hi = ys.count
        while lo < hi {
            let mid = (lo + hi) / 2
            if ys[mid] + hs[mid] <= y { lo = mid + 1 } else { hi = mid }
        }
        return lo
    }

    private func attributes(_ i: Int) -> UICollectionViewLayoutAttributes {
        let a = UICollectionViewLayoutAttributes(forCellWith: IndexPath(item: i, section: 0))
        a.frame = CGRect(x: 0, y: ys[i], width: width, height: hs[i])
        return a
    }

    override func layoutAttributesForElements(in rect: CGRect) -> [UICollectionViewLayoutAttributes]? {
        var out: [UICollectionViewLayoutAttributes] = []
        var i = firstIndex(endingBelow: rect.minY)
        // A row not measured yet is drawn (and so measured) even when it's empty so far.
        if i > 0, hs[i - 1] == 0 { i -= 1 }
        while i < ys.count, ys[i] <= rect.maxY {
            out.append(attributes(i))
            i += 1
        }
        return out
    }

    override func layoutAttributesForItem(at indexPath: IndexPath) -> UICollectionViewLayoutAttributes? {
        indexPath.item < ys.count ? attributes(indexPath.item) : nil
    }

    override func shouldInvalidateLayout(forBoundsChange newBounds: CGRect) -> Bool { newBounds.width != width }

    override func shouldInvalidateLayout(forPreferredLayoutAttributes preferred: UICollectionViewLayoutAttributes,
                                         withOriginalAttributes original: UICollectionViewLayoutAttributes) -> Bool {
        let i = original.indexPath.item
        guard let source, i < source.heights.count else { return false }
        return abs(preferred.size.height - source.heights[i]) > 0.5
    }

    override func invalidationContext(forPreferredLayoutAttributes preferred: UICollectionViewLayoutAttributes,
                                      withOriginalAttributes original: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutInvalidationContext {
        let ctx = super.invalidationContext(forPreferredLayoutAttributes: preferred, withOriginalAttributes: original)
        if let dy = source?.measured(preferred.size.height, at: original.indexPath.item) {
            ctx.contentOffsetAdjustment = CGPoint(x: 0, y: dy)
        }
        return ctx
    }
}

// MARK: Cell

/// A row's SwiftUI view in a hosting controller of its own (so a row's sheets and menus present
/// as they do anywhere else), measured at the list's width once per content: laying out a long
/// answer is the costliest thing the list does, and the list asks for a cell's size often.
final class TranscriptCell: UICollectionViewCell {
    static let reuse = "row"
    private var host: UIHostingController<AnyView>?
    private var container: HostContainer?
    private(set) var rowID: String?
    /// The height measured for the content as it is, and the width it was measured at.
    private var size: (width: CGFloat, height: CGFloat)?

    func configure(_ view: AnyView, id: String, parent: UIViewController) {
        rowID = id
        size = nil
        if let host {
            host.rootView = view
            // Measured again (self-sizing) now its content is new.
            invalidateIntrinsicContentSize()
            return
        }
        let hc = UIHostingController(rootView: view)
        // The hosting view says when its content's size changes (a group opened inside, an image
        // came in): the container hears of it and the row is measured again.
        hc.sizingOptions = .intrinsicContentSize
        hc.safeAreaRegions = []
        hc.view.backgroundColor = .clear
        let box = HostContainer(frame: contentView.bounds)
        box.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        box.hosted = hc.view
        box.changed = { [weak self] in
            self?.size = nil
            self?.invalidateIntrinsicContentSize()
        }
        contentView.addSubview(box)
        parent.addChild(hc)
        hc.view.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(hc.view)
        let bottom = hc.view.bottomAnchor.constraint(equalTo: box.bottomAnchor)
        bottom.priority = .defaultHigh
        NSLayoutConstraint.activate([
            hc.view.topAnchor.constraint(equalTo: box.topAnchor),
            hc.view.leadingAnchor.constraint(equalTo: box.leadingAnchor),
            hc.view.trailingAnchor.constraint(equalTo: box.trailingAnchor),
            bottom,
        ])
        hc.didMove(toParent: parent)
        host = hc
        container = box
    }

    /// The row's content laid out at `height`: when that isn't the height the cell was given (it
    /// grew or shrank by itself), it's measured again.
    func contentHeight(_ height: CGFloat) {
        guard host != nil, abs(height - bounds.height) > 0.5 else { return }
        if let size, abs(size.height - ceil(height)) <= 0.5 { return }
        size = nil
        invalidateIntrinsicContentSize()
    }

    override func preferredLayoutAttributesFitting(_ attributes: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        guard let host else { return attributes }
        let width = attributes.size.width
        let height: CGFloat
        if let size, size.width == width {
            height = size.height
        } else {
            height = max(0, ceil(host.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude)).height))
            size = (width, height)
        }
        guard abs(attributes.size.height - height) > 0.25 else { return attributes }
        let out = attributes.copy() as! UICollectionViewLayoutAttributes
        out.size.height = height
        return out
    }
}

/// Holds a row's hosting view and says when the size the content wants changes.
private final class HostContainer: UIView {
    weak var hosted: UIView?
    var changed: () -> Void = {}
    private var ideal: CGSize?

    override func layoutSubviews() {
        super.layoutSubviews()
        guard let hosted else { return }
        let now = hosted.intrinsicContentSize
        if let ideal, ideal != now { changed() }
        ideal = now
    }
}
