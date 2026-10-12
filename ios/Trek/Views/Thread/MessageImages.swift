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

    var body: some View {
        HStack(spacing: 6) {
            ForEach(0..<shown, id: \.self) { i in
                Button { open = i } label: {
                    Thumbnail(threadID: threadID, itemID: itemID, index: i, side: count == 1 ? 168 : 76)
                        .overlay {
                            if i == shown - 1, count > shown {
                                Text("+\(count - shown + 1)")
                                    .font(.headline.monospacedDigit())
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

/// A message's images full screen: swipe between them, pinch or double-tap to zoom.
struct ImageViewer: View {
    var threadID: String
    var itemID: String
    var count: Int
    var start: Int
    @Environment(\.dismiss) private var dismiss
    @State private var page = 0

    var body: some View {
        ZStack(alignment: .top) {
            Color.black.ignoresSafeArea()
            TabView(selection: $page) {
                ForEach(0..<count, id: \.self) { i in
                    FullImage(threadID: threadID, itemID: itemID, index: i).tag(i)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: count > 1 ? .automatic : .never))
            .ignoresSafeArea()
            HStack {
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
                        .font(.subheadline.monospacedDigit().weight(.medium))
                        .padding(.horizontal, 12)
                        .frame(height: 32)
                        .glassEffect(.regular, in: .capsule)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
        }
        .preferredColorScheme(.dark)
        .statusBarHidden()
        .onAppear { page = start }
    }
}

private struct FullImage: View {
    var threadID: String
    var itemID: String
    var index: Int
    @Environment(AppModel.self) private var model
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                ZoomableImage(image: image)
            } else if failed {
                ContentUnavailableView("Couldn't load this image", systemImage: "photo",
                                       description: Text("It may not be on the Mac any more."))
            } else {
                // The thumbnail, if it's here, while the full image comes.
                ZStack {
                    if let small = Attachments.cache.object(forKey: "\(threadID)/\(itemID)/\(index)@480" as NSString) {
                        Image(uiImage: small).resizable().scaledToFit().blur(radius: 6)
                    }
                    ProgressView().tint(.white)
                }
            }
        }
        .task {
            let loaded = await model.attachment(threadID, item: itemID, index: index, maxPx: 2048)
            image = loaded
            failed = loaded == nil
        }
    }
}

/// An image that pinches and double-taps to zoom, and pans when zoomed.
private struct ZoomableImage: UIViewRepresentable {
    var image: UIImage

    func makeUIView(context: Context) -> UIScrollView {
        let scroll = UIScrollView()
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
        return scroll
    }

    func updateUIView(_ scroll: UIScrollView, context: Context) {
        context.coordinator.view?.image = image
        DispatchQueue.main.async { context.coordinator.layout(scroll) }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, UIScrollViewDelegate {
        weak var view: UIImageView?

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
