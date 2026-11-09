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

    /// What goes with photos sent with no words: the agent and the transcript read it.
    nonisolated static func caption(_ count: Int) -> String {
        count > 1 ? "Here are \(count) photos." : "Here's a photo."
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
