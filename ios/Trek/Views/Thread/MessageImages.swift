import SwiftUI
import UIKit

/// A message's images as thumbnails in its bubble, as the Mac shows them; a tap opens them full
/// screen, like the Mac's preview, and a long press offers to save, copy or share one.
struct MessageImages: View {
    var threadID: String
    var itemID: String
    var count: Int
    @State private var open: Int?

    /// Up to four are drawn; a fourth tile says how many more.
    private var shown: Int { min(count, 4) }

    /// A tile's side: one large, a few side by side, and four that still fit the bubble (at 76 they
    /// ran it past the transcript's margin, and its buttons towards the screen's edge).
    nonisolated static func side(count: Int) -> CGFloat {
        count == 1 ? 168 : count == 2 || count == 3 ? 76 : 66
    }

    var body: some View {
        HStack(spacing: 6) {
            ForEach(0..<shown, id: \.self) { i in
                Button { open = i } label: {
                    Thumbnail(threadID: threadID, itemID: itemID, index: i, side: Self.side(count: count))
                        .overlay {
                            if i == shown - 1, count > shown {
                                Text("+\(count - shown + 1)")
                                    .font(.ui(.headline).monospacedDigit())
                                    .foregroundStyle(.white)
                                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                                    .background(.black.opacity(0.45))
                            }
                        }
                        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                }
                .buttonStyle(.plain)
                // What lifts under a long press is the tile, corners and all.
                .contentShape(.contextMenuPreview, RoundedRectangle(cornerRadius: 10, style: .continuous))
                .pictureMenu(.message(threadID: threadID, itemID: itemID, index: i))
                .accessibilityLabel("Image \(i + 1) of \(count)")
                .accessibilityHint("Opens it full screen")
            }
        }
        .onAppear { if Launch.sheet == "image", open == nil { open = 0 } }
        .fullScreenCover(item: Binding(get: { open.map(Opened.init) }, set: { open = $0?.index })) { o in
            ImageViewer(threadID: threadID, itemID: itemID, count: count, start: o.index)
        }
    }

    private struct Opened: Identifiable {
        var index: Int
        var id: Int { index }
    }
}

/// One image, square, loaded from the Mac.
private struct Thumbnail: View {
    var threadID: String
    var itemID: String
    var index: Int
    var side: CGFloat
    @Environment(AppModel.self) private var model
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        ZStack {
            Trek.foreground.opacity(0.06)
            if let image {
                Image(uiImage: image).resizable().scaledToFill()
                    .transition(.opacity)
            } else if failed {
                Image(systemName: "photo").foregroundStyle(Trek.muted)
            } else {
                ProgressView().controlSize(.small)
            }
        }
        .frame(width: side, height: side)
        .clipped()
        .task(id: "\(threadID)/\(itemID)/\(index)") {
            let loaded = await model.attachment(threadID, item: itemID, index: index, maxPx: 480)
            withAnimation(.easeOut(duration: 0.2)) {
                image = loaded
                failed = loaded == nil
            }
        }
    }
}

/// An image the full-screen viewer shows: one of a message's, one of a note's (`attachments/…`
/// on the Mac), a picture on the web a note links, or a photo picked for a message not yet sent.
enum ViewerImage: Hashable {
    case message(threadID: String, itemID: String, index: Int)
    case note(noteID: String, path: String)
    case web(URL)
    /// A picture on the Mac a thread names (an answer's `![](path)`, a watched video's frame).
    case media(threadID: String, path: String)
    /// Picked in the composer: it's on the phone already.
    case picked(UIImage)

    /// Its thumbnail, if it's been drawn already: shown blurred while the full image comes.
    var thumbnail: UIImage? {
        switch self {
        case .message(let t, let i, let n): Attachments.cache.object(forKey: "\(t)/\(i)/\(n)@480" as NSString)
        case .note(let n, let p): Attachments.cache.object(forKey: "note:\(n)/\(p)@\(NoteImageView.thumbPx)" as NSString)
        case .web(let url): Attachments.cache.object(forKey: url.absoluteString as NSString)
        case .media(let t, let p):
            Attachments.cache.object(forKey: "media:\(t)/\(p)@\(AnswerImage.thumbPx)" as NSString)
                ?? Attachments.cache.object(forKey: "media:\(t)/\(p)@\(WatchedVideoView.framePx)" as NSString)
        case .picked(let image): image
        }
    }

    /// The image itself, as big as the Mac sends; nil when it couldn't be had.
    @MainActor func load(_ model: AppModel, maxPx: Int = 2048) async -> UIImage? {
        switch self {
        case .message(let t, let i, let n): return await model.attachment(t, item: i, index: n, maxPx: maxPx)
        case .note(let n, let p): return await model.noteAsset(n, path: p, maxPx: maxPx)
        case .media(let t, let p): return await model.mediaImage(t, path: p, maxPx: maxPx)
        case .picked(let image): return image
        case .web(let url):
            let key = url.absoluteString as NSString
            if let image = Attachments.cache.object(forKey: key) { return image }
            guard let (data, _) = try? await URLSession.shared.data(from: url), data.count < 20 << 20,
                  let image = await Attachments.decode(data) else { return nil }
            Attachments.cache.setObject(image, forKey: key, cost: data.count)
            return image
        }
    }
}

/// How far an image has been pulled down to close the viewer, 0 to 1: what the black behind it and
/// the buttons over it follow, read only by them (a drag doesn't redraw the pages).
@Observable
final class ViewerDrag {
    var progress: CGFloat = 0
}

/// Images full screen: a message's, or a note's pictures. Swipe between them, pinch or
/// double-tap to zoom, pull one down to close: what's behind shows through as it goes. A long
/// press on one offers to save, copy or share it.
struct ImageViewer: View {
    var images: [ViewerImage]
    var start: Int
    @Environment(\.dismiss) private var dismiss
    @State private var page = 0
    @State private var drag = ViewerDrag()
    /// What a long press on an image ends in. The viewer's own, not each page's: a sheet asked
    /// for from inside a page of the pager didn't come up the first time.
    @State private var runner = PictureRunner()

    init(images: [ViewerImage], start: Int) {
        self.images = images
        self.start = start
    }

    /// A message's images.
    init(threadID: String, itemID: String, count: Int, start: Int) {
        self.init(images: (0..<count).map { .message(threadID: threadID, itemID: itemID, index: $0) }, start: start)
    }

    private var count: Int { images.count }

    var body: some View {
        ZStack(alignment: .top) {
            Backdrop(drag: drag)
            TabView(selection: $page) {
                ForEach(Array(images.enumerated()), id: \.offset) { i, image in
                    FullImage(image: image, drag: drag, runner: runner).tag(i)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: count > 1 ? .automatic : .never))
            .ignoresSafeArea()
            Chrome(drag: drag) {
                Button { dismiss() } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 15, weight: .semibold))
                        .frame(width: 40, height: 40)
                }
                .glassEffect(.regular.interactive(), in: .circle)
                .accessibilityLabel("Close")
                Spacer()
                if count > 1 {
                    Text("\(page + 1) of \(count)")
                        .font(.ui(.subheadline).monospacedDigit().weight(.medium))
                        .padding(.horizontal, 12)
                        .frame(height: 32)
                        .glassEffect(.regular, in: .capsule)
                }
            }
            // "Saved to Photos", "Copied": the app's own toast is under this cover. Below the
            // buttons, and felt once (the one under the cover feels what goes wrong).
            ToastOverlay(feels: false)
                .padding(.top, 52)
        }
        .modifier(PictureOutcomes(runner: runner))
        .preferredColorScheme(.dark)
        .statusBarHidden()
        // Clear, so the thread shows through as the black behind the image fades with a pull.
        .presentationBackground(.clear)
        .onAppear { page = start }
    }

    /// The black behind the image: it fades as the image is pulled down, all gone by half way.
    private struct Backdrop: View {
        let drag: ViewerDrag

        var body: some View {
            Color.black.opacity(1 - drag.progress).ignoresSafeArea()
        }
    }

    /// The buttons over the image: out of the way as soon as it's pulled.
    private struct Chrome<Content: View>: View {
        let drag: ViewerDrag
        @ViewBuilder var content: Content

        var body: some View {
            HStack { content }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .opacity(max(0, 1 - drag.progress * 3))
        }
    }
}

private struct FullImage: View {
    var image: ViewerImage
    let drag: ViewerDrag
    let runner: PictureRunner
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @MotionAllowed private var motion
    @State private var loaded: UIImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let loaded {
                ZoomableImage(image: loaded, drag: drag, motion: motion, onAct: { [runner, model] act in
                    Task { await runner.run(act, image: loaded, model: model) }
                }) {
                    // It has gone off the screen already: the cover goes without sliding again.
                    var t = Transaction()
                    t.disablesAnimations = true
                    withTransaction(t) { dismiss() }
                }
                .transition(.opacity)
            } else if failed {
                ContentUnavailableView("Couldn't load this image", systemImage: "photo",
                                       description: Text("It may not be on the Mac any more."))
            } else {
                // The thumbnail, if it's here, while the full image comes.
                ZStack {
                    if let small = image.thumbnail {
                        Image(uiImage: small).resizable().scaledToFit().blur(radius: 6)
                    }
                    ProgressView().tint(.white)
                }
                .transition(.opacity)
            }
        }
        .task {
            let got = await image.load(model)
            // The sharp image over the blurred one, not in its place at once.
            withAnimation(.easeOut(duration: 0.2)) {
                loaded = got
                failed = got == nil
            }
        }
    }
}

/// An image that pinches and double-taps to zoom, pans when zoomed, and closes on a pull down
/// when it isn't: it follows the finger, shrinking a little as it goes, and, let go, either
/// carries on off the screen at the finger's speed or springs back. Held still, it lifts and
/// offers Save to Photos, Copy and Share… (the system's own menu, taken off the image for as long
/// as a pull, a pan, a pinch or a swipe to the next image has hold of it).
struct ZoomableImage: UIViewRepresentable {
    var image: UIImage
    let drag: ViewerDrag
    /// Movement welcome (not Reduce Motion): let go, it flies off or springs back with a little
    /// give; without, it fades out or settles back with none.
    var motion: Bool
    /// Save, copy or share was picked from the long press's menu.
    var onAct: (PictureAct) -> Void
    /// It has left the screen.
    var onClose: () -> Void

    /// The scroll view sits in a plain view of its own: SwiftUI places the view it's handed (and
    /// puts its transform back as it lays out, which a pull now makes it do every frame), so the
    /// pull moves the one inside.
    func makeUIView(context: Context) -> UIView {
        let box = UIView()
        box.backgroundColor = .clear
        let scroll = UIScrollView(frame: box.bounds)
        scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        box.addSubview(scroll)
        context.coordinator.scroll = scroll
        scroll.delegate = context.coordinator
        scroll.minimumZoomScale = 1
        scroll.maximumZoomScale = 5
        scroll.showsHorizontalScrollIndicator = false
        scroll.showsVerticalScrollIndicator = false
        scroll.contentInsetAdjustmentBehavior = .never
        scroll.backgroundColor = .clear
        let view = UIImageView(image: image)
        view.contentMode = .scaleAspectFit
        view.isAccessibilityElement = true
        view.accessibilityLabel = "Image"
        // The long press's menu, on the scroll view beside its other gestures: the image stays
        // out of touch handling, as it was, so the pinch, the pan and the pull are found as before.
        // The same three are VoiceOver's actions on the image.
        context.coordinator.offerMenu()
        let coordinator = context.coordinator
        view.accessibilityCustomActions = PictureAct.allCases.map { act in
            UIAccessibilityCustomAction(name: act.title) { [weak coordinator] _ in
                coordinator?.onAct(act)
                return true
            }
        }
        scroll.addSubview(view)
        context.coordinator.view = view
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.doubleTap(_:)))
        tap.numberOfTapsRequired = 2
        scroll.addGestureRecognizer(tap)
        let swipe = UIPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.swiped(_:)))
        // One finger: a second (a pinch starting) mustn't move it.
        swipe.maximumNumberOfTouches = 1
        swipe.delegate = context.coordinator
        scroll.addGestureRecognizer(swipe)
        return box
    }

    func updateUIView(_ box: UIView, context: Context) {
        let c = context.coordinator
        c.view?.image = image
        c.onClose = onClose
        c.onAct = onAct
        c.drag = drag
        c.motion = motion
        DispatchQueue.main.async {
            guard let scroll = c.scroll else { return }
            c.layout(scroll)
            c.watchPager(from: scroll)
        }
    }

    static func dismantleUIView(_ box: UIView, coordinator: Coordinator) {
        coordinator.unwatchPager()
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, UIScrollViewDelegate, UIGestureRecognizerDelegate, UIContextMenuInteractionDelegate {
        weak var view: UIImageView?
        weak var scroll: UIScrollView?
        var onClose: () -> Void = {}
        var onAct: (PictureAct) -> Void = { _ in }
        var drag: ViewerDrag?
        var motion = true
        /// Gone: the pull let go of it past the point of no return.
        private var leaving = false

        /// Only straight down, and only with nothing to pan: sideways is the next photo, and a
        /// zoomed image pans.
        func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
            guard let pan = gesture as? UIPanGestureRecognizer,
                  let scroll = pan.view as? UIScrollView else { return true }
            let moved = pan.velocity(in: scroll)
            // Not with the long press's menu up: the finger that brought it up picks from it.
            return !menuUp && scroll.zoomScale == 1 && moved.y > 0 && moved.y > abs(moved.x)
        }

        func gestureRecognizer(_ gesture: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            true
        }

        /// The image `y` points down from its place: moved, and a little smaller the further it is
        /// (85 % by half the screen), with what's behind it following (`ViewerDrag`).
        private func place(_ scroll: UIScrollView, at y: CGFloat, height: CGFloat) {
            let progress = min(1, max(0, y / (height * 0.5)))
            let scale = 1 - 0.15 * progress
            scroll.transform = CGAffineTransform(translationX: 0, y: y).scaledBy(x: scale, y: scale)
            drag?.progress = progress
        }

        /// Where the image was, on screen, as this pull took hold of it: caught springing back, it
        /// carries on from there rather than jumping to its place first.
        private var base: CGFloat = 0

        @objc func swiped(_ pan: UIPanGestureRecognizer) {
            guard let scroll = pan.view as? UIScrollView, !leaving else { return }
            // Measured in the scroll view's parent: the scroll view itself moves with the finger.
            let space = scroll.superview ?? scroll
            let height = max(1, scroll.bounds.height)
            if pan.state == .began {
                base = scroll.layer.presentation()?.affineTransform().ty ?? 0
                scroll.layer.removeAllAnimations()
                hold(.pull, true)
            }
            let pulled = base + pan.translation(in: space).y
            // Down follows the finger one to one; up, past where it started, gives less and less.
            let y = pulled >= 0 ? pulled : Fling.rubberBand(pulled, size: height)
            switch pan.state {
            case .began, .changed:
                place(scroll, at: y, height: height)
            case .ended, .cancelled, .failed:
                let v = pan.velocity(in: space).y
                // Where a flick would carry it, and which way the finger was going as it lifted:
                // a pull that ends going back up stays, however far it went.
                let resting = y + Fling.carry(v)
                if pan.state == .ended, v > -60, y > 110 || resting > height * 0.35 {
                    leave(scroll, from: y, velocity: v, height: height)
                } else {
                    settle(scroll, from: y, velocity: v)
                }
                hold(.pull, false)
            default:
                break
            }
        }

        /// Off the bottom at the finger's speed, the black behind it gone with it; then closed.
        private func leave(_ scroll: UIScrollView, from y: CGFloat, velocity v: CGFloat, height: CGFloat) {
            leaving = true
            let close: (Bool) -> Void = { [weak self] _ in self?.onClose() }
            withAnimation(.easeOut(duration: 0.25)) { drag?.progress = 1 }
            guard motion else {
                UIView.animate(withDuration: 0.2, delay: 0, options: .curveEaseOut, animations: { scroll.alpha = 0 }, completion: close)
                return
            }
            let to = height * 1.05
            UIView.animate(springDuration: 0.35, bounce: 0, initialSpringVelocity: Fling.relative(max(v, 600), over: to - y),
                           options: [.beginFromCurrentState], animations: {
                scroll.transform = CGAffineTransform(translationX: 0, y: to).scaledBy(x: 0.85, y: 0.85)
            }, completion: close)
        }

        /// Back where it was, carrying on from the finger's speed (a touch of give, as a thing let go
        /// of has; none with Reduce Motion).
        private func settle(_ scroll: UIScrollView, from y: CGFloat, velocity v: CGFloat) {
            withAnimation(.snappy(duration: 0.3)) { drag?.progress = 0 }
            UIView.animate(springDuration: 0.4, bounce: motion ? 0.15 : 0, initialSpringVelocity: motion ? Fling.relative(v, over: -y) : 0,
                           options: [.beginFromCurrentState, .allowUserInteraction], animations: {
                scroll.transform = .identity
            })
        }

        func layout(_ scroll: UIScrollView) {
            guard let view, scroll.zoomScale == 1 else { return }
            view.frame = scroll.bounds
            scroll.contentSize = scroll.bounds.size
        }

        func viewForZooming(in scrollView: UIScrollView) -> UIView? { view }

        // MARK: The long press's menu

        /// What has hold of the image, and so keeps the menu off it.
        nonisolated enum Hold: Hashable, Sendable { case pull, page, pan, zoom }
        private var holds: Set<Hold> = []
        private var menu: UIContextMenuInteraction?
        /// The menu is on screen.
        private var menuUp = false
        /// What was picked from the menu, until the menu has left the screen: what it ends in (a
        /// sheet, an alert, a toast) comes once the menu is out of the way.
        private var chosen: PictureAct?
        /// The scroll view that pages between images (the `TabView`'s), watched for its swipe.
        private weak var pager: UIScrollView?

        /// The menu is the scroll view's while nothing else has hold of the image. A gesture that
        /// takes hold takes the menu off for as long as it lasts: the image goes with the finger,
        /// and the menu's own long press, which counts how long a finger has been down more than
        /// how far it went, came up part-way through a pull, a pan or a swipe to the next image.
        func hold(_ reason: Hold, _ on: Bool) {
            if on { holds.insert(reason) } else { holds.remove(reason) }
            guard let scroll else { return }
            if Self.menuOffered(holds: holds, leaving: leaving) {
                guard menu == nil else { return }
                let interaction = UIContextMenuInteraction(delegate: self)
                scroll.addInteraction(interaction)
                menu = interaction
            } else if let interaction = menu {
                scroll.removeInteraction(interaction)
                menu = nil
            }
        }

        nonisolated static func menuOffered(holds: Set<Hold>, leaving: Bool) -> Bool {
            holds.isEmpty && !leaving
        }

        /// Puts the menu on the image (once its scroll view is here).
        func offerMenu() { hold(.pull, false) }

        /// Follows the swipe between images, to keep the menu out of it.
        func watchPager(from scroll: UIScrollView) {
            guard pager == nil else { return }
            var above = scroll.superview
            while let view = above {
                if let found = view as? UIScrollView {
                    pager = found
                    found.panGestureRecognizer.addTarget(self, action: #selector(paged(_:)))
                    return
                }
                above = view.superview
            }
        }

        func unwatchPager() {
            pager?.panGestureRecognizer.removeTarget(self, action: #selector(paged(_:)))
            pager = nil
        }

        @objc func paged(_ pan: UIPanGestureRecognizer) {
            switch pan.state {
            case .began: hold(.page, true)
            case .ended, .cancelled, .failed: hold(.page, false)
            default: break
            }
        }

        func scrollViewWillBeginDragging(_ scrollView: UIScrollView) { hold(.pan, true) }

        func scrollViewDidEndDragging(_ scrollView: UIScrollView, willDecelerate decelerate: Bool) {
            if !decelerate { hold(.pan, false) }
        }

        func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) { hold(.pan, false) }

        func scrollViewWillBeginZooming(_ scrollView: UIScrollView, with view: UIView?) { hold(.zoom, true) }

        func scrollViewDidEndZooming(_ scrollView: UIScrollView, with view: UIView?, atScale scale: CGFloat) { hold(.zoom, false) }

        func contextMenuInteraction(_ interaction: UIContextMenuInteraction, willDisplayMenuFor configuration: UIContextMenuConfiguration,
                                    animator: (any UIContextMenuInteractionAnimating)?) {
            menuUp = true
        }

        func contextMenuInteraction(_ interaction: UIContextMenuInteraction, willEndFor configuration: UIContextMenuConfiguration,
                                    animator: (any UIContextMenuInteractionAnimating)?) {
            menuUp = false
            if let animator {
                animator.addCompletion { [weak self] in self?.carryOut() }
            } else {
                carryOut()
            }
        }

        /// A row of the menu was tapped: done once the menu has gone (now, if it has already).
        private func picked(_ act: PictureAct) {
            chosen = act
            if !menuUp { carryOut() }
        }

        private func carryOut() {
            guard let act = chosen else { return }
            chosen = nil
            onAct(act)
        }

        /// Whether a long press may bring the menu up: on the picture itself (not the black beside
        /// it), and not while the image is on its way out, nor part-way through a pull (`pulled`:
        /// how far, 0 to 1).
        nonisolated static func offersMenu(leaving: Bool, pulled: CGFloat, onPicture: Bool = true) -> Bool {
            !leaving && pulled == 0 && onPicture
        }

        func contextMenuInteraction(_ interaction: UIContextMenuInteraction, configurationForMenuAtLocation location: CGPoint) -> UIContextMenuConfiguration? {
            guard let view, let size = view.image?.size else { return nil }
            let onPicture = Self.fitted(size, in: view.bounds).contains(interaction.location(in: view))
            guard Self.offersMenu(leaving: leaving, pulled: drag?.progress ?? 0, onPicture: onPicture) else { return nil }
            return UIContextMenuConfiguration(identifier: nil, previewProvider: nil) { [weak self] _ in
                UIMenu(children: PictureAct.allCases.map { act in
                    UIAction(title: act.title, image: UIImage(systemName: act.symbol)) { _ in self?.picked(act) }
                })
            }
        }

        func contextMenuInteraction(_ interaction: UIContextMenuInteraction, configuration: UIContextMenuConfiguration,
                                    highlightPreviewForItemWithIdentifier identifier: any NSCopying) -> UITargetedPreview? {
            lifted()
        }

        func contextMenuInteraction(_ interaction: UIContextMenuInteraction, configuration: UIContextMenuConfiguration,
                                    dismissalPreviewForItemWithIdentifier identifier: any NSCopying) -> UITargetedPreview? {
            lifted()
        }

        /// What lifts is the picture, not the screen it's fitted in: the bars above and below it
        /// (or beside it) are no part of it.
        private func lifted() -> UITargetedPreview? {
            guard let view, view.window != nil else { return nil }
            let parameters = UIPreviewParameters()
            parameters.backgroundColor = .clear
            if let size = view.image?.size {
                parameters.visiblePath = UIBezierPath(roundedRect: Self.fitted(size, in: view.bounds), cornerRadius: 14)
            }
            return UITargetedPreview(view: view, parameters: parameters)
        }

        /// Where a picture `size` large is drawn when it's fitted, whole, in `bounds`.
        nonisolated static func fitted(_ size: CGSize, in bounds: CGRect) -> CGRect {
            guard size.width > 0, size.height > 0, bounds.width > 0, bounds.height > 0 else { return bounds }
            let scale = min(bounds.width / size.width, bounds.height / size.height)
            let fit = CGSize(width: size.width * scale, height: size.height * scale)
            return CGRect(x: bounds.midX - fit.width / 2, y: bounds.midY - fit.height / 2, width: fit.width, height: fit.height)
        }

        @objc func doubleTap(_ tap: UITapGestureRecognizer) {
            guard let scroll = tap.view as? UIScrollView else { return }
            if scroll.zoomScale > 1 {
                scroll.setZoomScale(1, animated: true)
            } else {
                let at = tap.location(in: view)
                let size = CGSize(width: scroll.bounds.width / 2.5, height: scroll.bounds.height / 2.5)
                scroll.zoom(to: CGRect(x: at.x - size.width / 2, y: at.y - size.height / 2, width: size.width, height: size.height), animated: true)
            }
        }
    }
}
