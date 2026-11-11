import LinkPresentation
import SwiftUI
import UIKit

// A long press on a picture: Save to Photos, Copy and Share…, the same three in a message's
// thumbnails, an answer's pictures and the full-screen viewer (`PictureKeeper` does the saving).

/// A picture handed to the share sheet.
struct SharedPicture: Identifiable {
    let id = UUID()
    var image: UIImage
}

/// Carries out what was picked for a picture, and holds what it may end in: the share sheet, or
/// the alert that says Photos is off for Trek.
@Observable
final class PictureRunner {
    var sharing: SharedPicture?
    /// Saving was refused for a reason Settings can change.
    var blocked: PictureSave?

    /// `act` on the picture `source` names, at the size the Mac sends it (a thumbnail is what's on
    /// screen; what's kept is the picture).
    func run(_ act: PictureAct, on source: ViewerImage, model: AppModel) {
        Task {
            guard let image = await source.load(model) else {
                model.show("Couldn't get that picture from your Mac.", error: true)
                return
            }
            await run(act, image: image, model: model)
        }
    }

    func run(_ act: PictureAct, image: UIImage, model: AppModel) async {
        switch act {
        case .copy:
            UIPasteboard.general.image = image
            model.show("Copied")
        case .share:
            sharing = SharedPicture(image: image)
        case .save:
            let outcome = await PictureKeeper.save(image)
            if outcome == .saved {
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                model.show(outcome.message)
            } else if outcome.needsSettings {
                blocked = outcome
            } else {
                model.show(outcome.message, error: true)
            }
        }
    }
}

/// The menu's rows, for a SwiftUI context menu.
struct PictureMenuItems: View {
    var act: (PictureAct) -> Void

    var body: some View {
        ForEach(PictureAct.allCases, id: \.self) { a in
            Button(a.title, systemImage: a.symbol) { act(a) }
        }
    }
}

/// What a picture's actions may end in: the share sheet, and the alert for Photos turned off.
struct PictureOutcomes: ViewModifier {
    @Bindable var runner: PictureRunner
    @Environment(\.openURL) private var openURL

    func body(content: Content) -> some View {
        content
            .sheet(item: $runner.sharing) { shared in
                ShareSheet(image: shared.image)
                    .presentationDetents([.medium, .large])
            }
            .alert("Trek can't add to Photos", isPresented: Binding(get: { runner.blocked != nil }, set: { if !$0 { runner.blocked = nil } }),
                   presenting: runner.blocked) { _ in
                Button("Open Settings") {
                    if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                }
                Button("Not now", role: .cancel) {}
            } message: { outcome in
                Text(outcome.message)
            }
    }
}

private struct PictureMenu: ViewModifier {
    var source: ViewerImage?
    @Environment(AppModel.self) private var model
    @State private var runner = PictureRunner()

    func body(content: Content) -> some View {
        content
            .contextMenu {
                if let source {
                    PictureMenuItems { runner.run($0, on: source, model: model) }
                }
            }
            .modifier(PictureOutcomes(runner: runner))
    }
}

extension View {
    /// A long press offers to save, copy or share the picture `source` names (nil: nothing to
    /// offer yet). The tap stays the view's own.
    func pictureMenu(_ source: ViewerImage?) -> some View {
        modifier(PictureMenu(source: source))
    }
}

/// The system's share sheet for a picture.
struct ShareSheet: UIViewControllerRepresentable {
    var image: UIImage

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [SharedPictureItem(image)], applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

/// What the share sheet is handed: the picture itself, and for the sheet's header the picture
/// small and a name (a bare image gets a blank tile there).
private final class SharedPictureItem: NSObject, UIActivityItemSource {
    let image: UIImage

    init(_ image: UIImage) { self.image = image }

    func activityViewControllerPlaceholderItem(_ controller: UIActivityViewController) -> Any { image }

    func activityViewController(_ controller: UIActivityViewController, itemForActivityType type: UIActivity.ActivityType?) -> Any? { image }

    func activityViewControllerLinkMetadata(_ controller: UIActivityViewController) -> LPLinkMetadata? {
        let metadata = LPLinkMetadata()
        metadata.title = "Picture"
        metadata.imageProvider = NSItemProvider(object: image)
        return metadata
    }
}
