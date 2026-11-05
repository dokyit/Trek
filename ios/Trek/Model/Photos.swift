import PhotosUI
import SwiftUI
import UIKit

/// Photos picked to go with a message: shown as thumbnails, sent as JPEGs small enough for the
/// wire (the long side at most 1600 pixels, under about 900 KB).
struct PickedPhoto: Identifiable, Equatable {
    let id = UUID()
    let image: UIImage

    static func == (a: PickedPhoto, b: PickedPhoto) -> Bool { a.id == b.id }

    /// The photo as it goes to the Mac.
    var upload: ImageUpload? {
        let longest = max(image.size.width, image.size.height)
        let scale = min(1, 1600 / max(longest, 1))
        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        // Scale 1: the default format renders at the screen's scale, three times the pixels here.
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: size, format: format).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
        var quality: CGFloat = 0.82
        var data = resized.jpegData(compressionQuality: quality)
        while let d = data, d.count > 900_000, quality > 0.35 {
            quality -= 0.12
            data = resized.jpegData(compressionQuality: quality)
        }
        return data.map { ImageUpload(mime: "image/jpeg", data: $0.base64EncodedString()) }
    }

    /// Load what the system photo picker handed back.
    static func load(_ items: [PhotosPickerItem]) async -> [PickedPhoto] {
        var out: [PickedPhoto] = []
        for item in items {
            if let data = try? await item.loadTransferable(type: Data.self), let image = UIImage(data: data) {
                out.append(PickedPhoto(image: image))
            }
        }
        return out
    }
}

/// The picked photos above a composer, each with a button to take it off; a tap opens them full
/// screen, as a message's images do.
struct PhotoStrip: View {
    @Binding var photos: [PickedPhoto]
    @State private var open: Int?

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(photos) { p in
                    Button { open = photos.firstIndex { $0.id == p.id } } label: {
                        Image(uiImage: p.image)
                            .resizable()
                            .scaledToFill()
                            .frame(width: 56, height: 56)
                            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            // The tile, not the whole picture its fill crops: taps stay on their own photo.
                            .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Attached photo")
                    .accessibilityHint("Opens it full screen")
                    // Beside the button, not in its label: a tap here only removes.
                    .overlay(alignment: .topTrailing) {
                        Button {
                            withAnimation(.snappy) { photos.removeAll { $0.id == p.id } }
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .font(.system(size: 18))
                                .symbolRenderingMode(.palette)
                                .foregroundStyle(.white, .black.opacity(0.6))
                        }
                        .buttonStyle(.plain)
                        .offset(x: 6, y: -6)
                        .accessibilityLabel("Remove photo")
                    }
                }
            }
            .padding(.top, 8)
            .padding(.horizontal, 4)
        }
        .fullScreenCover(item: Binding(get: { open.map(Opened.init) }, set: { open = $0?.index })) { o in
            ImageViewer(images: photos.map { .picked($0.image) }, start: o.index)
        }
    }

    private struct Opened: Identifiable {
        var index: Int
        var id: Int { index }
    }
}
