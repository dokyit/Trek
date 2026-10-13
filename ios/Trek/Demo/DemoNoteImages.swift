import UIKit

/// The demo Mac's pictures in notes: a whiteboard sketch drawn on the fly, and the photos the
/// phone adds (kept in memory, as the Mac keeps them in the notes' `attachments/`).
extension DemoMac {
    /// The sketch the demo's "Sync flow" note shows.
    static let whiteboard = "attachments/3f2a9c41d07e.png"

    /// Photos added from the phone this run, by path.
    static var addedNoteImages: [String: Data] = [:]

    /// The picture at `path`, as a JPEG fitting `maxPx` a side, with its size.
    static func noteAsset(_ path: String, maxPx: Int) -> (Data, Int, Int)? {
        if let data = addedNoteImages[path], let image = UIImage(data: data) {
            return jpeg(image, maxPx: maxPx)
        }
        guard path == whiteboard else { return nil }
        return jpeg(sketch(), maxPx: maxPx)
    }

    /// Keep a photo the phone sent; its link from a note.
    static func addNoteImage(_ data: Data) -> String {
        let path = "attachments/\(UUID().uuidString.prefix(12).lowercased()).jpg"
        addedNoteImages[path] = data
        return path
    }

    private static func jpeg(_ image: UIImage, maxPx: Int) -> (Data, Int, Int)? {
        let scale = min(1, CGFloat(maxPx) / max(image.size.width, image.size.height, 1))
        let size = CGSize(width: (image.size.width * scale).rounded(), height: (image.size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let scaled = UIGraphicsImageRenderer(size: size, format: format).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
        guard let data = scaled.jpegData(compressionQuality: 0.82) else { return nil }
        return (data, Int(size.width), Int(size.height))
    }

    /// Boxes and arrows in marker on a whiteboard: phone, queue, Mac.
    private static func sketch() -> UIImage {
        let size = CGSize(width: 1600, height: 1000)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        return UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            let c = ctx.cgContext
            UIColor(white: 0.96, alpha: 1).setFill()
            c.fill(CGRect(origin: .zero, size: size))
            let ink = [UIColor(red: 0.13, green: 0.35, blue: 0.75, alpha: 1), UIColor(red: 0.8, green: 0.22, blue: 0.2, alpha: 1),
                       UIColor(red: 0.1, green: 0.5, blue: 0.3, alpha: 1)]
            let boxes = [CGRect(x: 120, y: 360, width: 340, height: 220), CGRect(x: 630, y: 360, width: 340, height: 220),
                         CGRect(x: 1140, y: 360, width: 340, height: 220)]
            let labels = ["iPhone", "Queue", "Mac"]
            for (i, box) in boxes.enumerated() {
                ink[i].setStroke()
                let path = UIBezierPath(roundedRect: box, cornerRadius: 28)
                path.lineWidth = 10
                path.stroke()
                let text = labels[i] as NSString
                let attrs: [NSAttributedString.Key: Any] = [.font: UIFont.systemFont(ofSize: 72, weight: .heavy), .foregroundColor: ink[i]]
                let t = text.size(withAttributes: attrs)
                text.draw(at: CGPoint(x: box.midX - t.width / 2, y: box.midY - t.height / 2), withAttributes: attrs)
            }
            UIColor(white: 0.2, alpha: 1).setStroke()
            for (a, b) in [(boxes[0], boxes[1]), (boxes[1], boxes[2])] {
                let arrow = UIBezierPath()
                arrow.move(to: CGPoint(x: a.maxX + 20, y: a.midY))
                arrow.addLine(to: CGPoint(x: b.minX - 20, y: b.midY))
                arrow.move(to: CGPoint(x: b.minX - 60, y: b.midY - 36))
                arrow.addLine(to: CGPoint(x: b.minX - 20, y: b.midY))
                arrow.addLine(to: CGPoint(x: b.minX - 60, y: b.midY + 36))
                arrow.lineWidth = 9
                arrow.lineCapStyle = .round
                arrow.stroke()
            }
            let note = "retry with backoff" as NSString
            note.draw(at: CGPoint(x: 560, y: 680), withAttributes: [.font: UIFont.italicSystemFont(ofSize: 56), .foregroundColor: ink[1]])
            let title = "Sync flow" as NSString
            title.draw(at: CGPoint(x: 120, y: 120), withAttributes: [.font: UIFont.systemFont(ofSize: 96, weight: .black), .foregroundColor: UIColor(white: 0.15, alpha: 1)])
        }
    }
}
