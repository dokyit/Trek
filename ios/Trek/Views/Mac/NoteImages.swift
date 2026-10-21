import SwiftUI
import UIKit

/// A picture in a note: `![alt](link)`, as the Mac's preview draws it (`note_images.rs`). Ones
/// kept with the notes (`attachments/…`) come from the Mac scaled down (`note_asset`), ones on
/// the web straight from there; a file elsewhere on the Mac is a placeholder naming it.
nonisolated struct NoteImageRef: Hashable {
    var alt: String
    var link: String

    private static var pattern: Regex<(Substring, Substring, Substring?, Substring?)> { /!\[([^\]\n]*)\]\(\s*(?:<([^>\n]+)>|([^)\s]+))(?:\s+"[^"\n]*")?\s*\)/ }

    /// The pictures on a line that's nothing but pictures (as the Mac's Write puts them in), else nil.
    static func only(_ line: String) -> [NoteImageRef]? {
        var found: [NoteImageRef] = []
        var rest = ""
        var at = line.startIndex
        for m in line.matches(of: pattern) {
            rest += line[at..<m.range.lowerBound]
            found.append(NoteImageRef(alt: String(m.1), link: String(m.2 ?? m.3 ?? "")))
            at = m.range.upperBound
        }
        rest += line[at...]
        return found.isEmpty || !rest.trimmingCharacters(in: .whitespaces).isEmpty ? nil : found
    }

    /// One kept with the notes: its path there (`attachments/<name>`).
    var attachment: String? {
        let path = link.hasPrefix("./") ? String(link.dropFirst(2)) : link
        return path.hasPrefix("attachments/") && !path.contains("..") ? path : nil
    }

    var web: URL? {
        guard link.hasPrefix("https://") || link.hasPrefix("http://") else { return nil }
        return URL(string: link)
    }

    /// What the viewer shows for it in note `noteID`; nil for one only the Mac can open.
    func viewer(_ noteID: String?) -> ViewerImage? {
        if let attachment, let noteID { return .note(noteID: noteID, path: attachment) }
        return web.map(ViewerImage.web)
    }

    /// What a placeholder calls it: its alt text, else its file's name.
    var label: String {
        alt.isEmpty ? (link as NSString).lastPathComponent : alt
    }
}

/// A file kept with the notes, linked on a line of its own: `[name](attachments/<file>)`, as the
/// Mac's Write puts in a dropped PDF or video. Its bytes come from the Mac (`note_file`).
nonisolated struct NoteFileRef: Hashable {
    var name: String
    /// `attachments/<file>`.
    var path: String

    enum Kind { case pdf, video, other }

    private static var pattern: Regex<(Substring, Substring, Substring?, Substring?)> { /\[([^\]\n]*)\]\(\s*(?:<([^>\n]+)>|([^)\s]+))(?:\s+"[^"\n]*")?\s*\)/ }

    /// The file a line links, when the line is that link and nothing else, and the link is to a
    /// file kept with the notes.
    static func only(_ line: String) -> NoteFileRef? {
        guard let m = line.wholeMatch(of: pattern) else { return nil }
        let link = String(m.2 ?? m.3 ?? "")
        let path = link.hasPrefix("./") ? String(link.dropFirst(2)) : link
        guard path.hasPrefix("attachments/") else { return nil }
        let file = path.dropFirst("attachments/".count)
        guard !file.isEmpty, !file.hasPrefix("."), !file.contains("/"), !file.contains("..") else { return nil }
        return NoteFileRef(name: String(m.1), path: path)
    }

    var ext: String { (path as NSString).pathExtension.lowercased() }

    /// What it shows as: a PDF's first page, a video's poster (what AVPlayer plays; WebM it
    /// doesn't, so that's a file like any other), else a chip.
    var kind: Kind {
        switch ext {
        case "pdf": .pdf
        case "mp4", "m4v", "mov": .video
        default: .other
        }
    }

    /// Its link text, else its file's name.
    var label: String {
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        return trimmed.isEmpty ? (path as NSString).lastPathComponent : trimmed
    }
}

/// One of a note's pictures, fit to the note's width in its own proportions (a tall one no taller
/// than `tallest`); a tap opens it full screen.
struct NoteImageView: View {
    var image: NoteImageRef
    var noteID: String?
    var open: () -> Void
    @Environment(AppModel.self) private var model
    @State private var loaded: UIImage?
    @State private var failed = false

    /// The size thumbnails are asked for: sharp across a phone's width.
    static let thumbPx = 1024
    private let tallest: CGFloat = 320

    var body: some View {
        let source = image.viewer(noteID)
        Button(action: open) {
            Group {
                if let loaded {
                    Image(uiImage: loaded)
                        .resizable()
                        .scaledToFit()
                        .frame(maxHeight: tallest, alignment: .leading)
                        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
                        .transition(.opacity)
                } else {
                    placeholder(source == nil ? .onMac : failed ? .failed : .loading)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(.plain)
        .disabled(source == nil || loaded == nil)
        .accessibilityLabel(image.alt.isEmpty ? "Picture" : image.alt)
        .accessibilityHint(loaded == nil ? "" : "Opens it full screen")
        .task(id: source) {
            guard let source, model.serves(.noteImages, on: noteID) || source.isWeb else { return }
            let got = await source.load(model, maxPx: Self.thumbPx)
            withAnimation(.easeOut(duration: 0.2)) {
                loaded = got
                failed = got == nil
            }
        }
    }

    private enum Placeholder { case loading, failed, onMac }

    private func placeholder(_ why: Placeholder) -> some View {
        VStack(spacing: 6) {
            switch why {
            case .loading where model.serves(.noteImages, on: noteID) || image.web != nil:
                ProgressView().controlSize(.small)
            case .failed:
                Image(systemName: "photo.badge.exclamationmark").font(.system(size: 20))
                Text("Couldn't load \(image.label)").font(.ui(.footnote)).lineLimit(1)
            default:
                Image(systemName: "photo").font(.system(size: 20))
                Text("\(image.label) · on your Mac").font(.ui(.footnote)).lineLimit(1)
            }
        }
        .foregroundStyle(Trek.muted)
        .frame(maxWidth: .infinity)
        .frame(height: 140)
        .background(Trek.foreground.opacity(0.05), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

extension ViewerImage {
    var isWeb: Bool { if case .web = self { true } else { false } }
}
