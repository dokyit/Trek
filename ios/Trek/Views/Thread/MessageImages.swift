import SwiftUI
import UIKit

/// A message's images as thumbnails in its bubble, as the Mac shows them; a tap opens them full
/// screen, like the Mac's preview.
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
/// double-tap to zoom, pull one down to close: what's behind shows through as it goes.
struct ImageViewer: View {
    var images: [ViewerImage]
    var start: Int
    @Environment(\.dismiss) private var dismiss
    @State private var page = 0
    @State private var drag = ViewerDrag()

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
                    FullImage(image: image, drag: drag).tag(i)
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
        }
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
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @MotionAllowed private var motion
    @State private var loaded: UIImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let loaded {
                ZoomableImage(image: loaded, drag: drag, motion: motion) {
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
/// carries on off the screen at the finger's speed or springs back.
private struct ZoomableImage: UIViewRepresentable {
    var image: UIImage
    let drag: ViewerDrag
    /// Movement welcome (not Reduce Motion): let go, it flies off or springs back with a little
    /// give; without, it fades out or settles back with none.
    var motion: Bool
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
        c.drag = drag
        c.motion = motion
        DispatchQueue.main.async { if let scroll = c.scroll { c.layout(scroll) } }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, UIScrollViewDelegate, UIGestureRecognizerDelegate {
        weak var view: UIImageView?
        weak var scroll: UIScrollView?
        var onClose: () -> Void = {}
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
            return scroll.zoomScale == 1 && moved.y > 0 && moved.y > abs(moved.x)
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
