import SwiftUI
import UIKit

// Pictures and videos a thread names, drawn as the Mac's transcript draws them (`md.rs`
// LocalImage, `video.rs`): a picture an answer shows, a video it links, videos attached to a
// message, and a `watch_video` call's video with the frames the model saw. A file on the Mac
// comes from it (`media`); an https one from the web. A Mac that doesn't send them (`media`
// unlisted) leaves a card saying the file is on the Mac.

/// An image or video link in an answer: `![alt](link)` or `[name](link)`.
nonisolated struct MediaLink: Hashable, Sendable {
    var name: String
    var link: String

    private static var image: Regex<(Substring, Substring, Substring?, Substring?)> { /!\[([^\]\n]*)\]\(\s*(?:<([^>\n]+)>|([^)\s]+))(?:\s+"[^"\n]*")?\s*\)/ }
    private static var plain: Regex<(Substring, Substring, Substring?, Substring?)> { /\[([^\]\n]*)\]\(\s*(?:<([^>\n]+)>|([^)\s]+))(?:\s+"[^"\n]*")?\s*\)/ }

    static let videoExtensions: Set<String> = ["mp4", "m4v", "mov", "webm"]
    static let imageExtensions: Set<String> = ["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "tiff", "tif", "bmp"]

    /// The pictures on a line that's nothing but pictures, else nil.
    static func images(_ line: String) -> [MediaLink]? {
        let line = line.trimmingCharacters(in: .whitespaces)
        var found: [MediaLink] = []
        var rest = ""
        var at = line.startIndex
        for m in line.matches(of: image) {
            rest += line[at..<m.range.lowerBound]
            found.append(MediaLink(name: String(m.1), link: String(m.2 ?? m.3 ?? "")))
            at = m.range.upperBound
        }
        rest += line[at...]
        guard !found.isEmpty, rest.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        // Only links that are pictures: a local file by its extension, or anything on the web.
        return found.allSatisfy { $0.web != nil || imageExtensions.contains($0.ext) } ? found : nil
    }

    /// The video a line links, when the line is that link and nothing else.
    static func video(_ line: String) -> MediaLink? {
        let line = line.trimmingCharacters(in: .whitespaces)
        guard let m = line.wholeMatch(of: plain) else { return nil }
        let link = MediaLink(name: String(m.1), link: String(m.2 ?? m.3 ?? ""))
        guard videoExtensions.contains(link.ext) else { return nil }
        guard link.web != nil || !link.link.contains("://") || link.link.hasPrefix("file://") else { return nil }
        return link
    }

    /// The link without a `file://` or a `#fragment`/`?query`, as the Mac resolves it.
    var path: String {
        var p = link
        if p.hasPrefix("file://") { p = String(p.dropFirst("file://".count)) }
        if let cut = p.firstIndex(where: { $0 == "#" || $0 == "?" }), web == nil { p = String(p[..<cut]) }
        return p.removingPercentEncoding ?? p
    }

    var ext: String { ((web?.path ?? path) as NSString).pathExtension.lowercased() }

    var web: URL? {
        guard link.hasPrefix("https://") || link.hasPrefix("http://") else { return nil }
        return URL(string: link)
    }

    var label: String {
        let n = name.trimmingCharacters(in: .whitespaces)
        return n.isEmpty ? ((web?.path ?? path) as NSString).lastPathComponent : n
    }

    /// What the image viewer shows for it in `threadID`'s transcript.
    func viewer(_ threadID: String?) -> ViewerImage? {
        if let web { return .web(web) }
        return threadID.map { .media(threadID: $0, path: path) }
    }
}

extension AppModel {
    /// A picture a thread names, fitting `maxPx` a side (`media`); kept a while in memory.
    func mediaImage(_ tid: String, path: String, maxPx: Int) async -> UIImage? {
        let key = "media:\(tid)/\(path)@\(maxPx)" as NSString
        if let image = Attachments.cache.object(forKey: key) { return image }
        guard serves(.media, on: tid) else { return nil }
        let data: Data? = await withCheckedContinuation { c in
            request(.media(threadId: tid, path: path, maxPx: maxPx, offset: 0, length: nil)) { reply in
                if case .media(_, let m) = reply { c.resume(returning: Data(base64Encoded: m.bytesB64)) } else { c.resume(returning: nil) }
            }
        }
        guard let data, let image = await Attachments.decode(data) else { return nil }
        Attachments.cache.setObject(image, forKey: key, cost: data.count)
        return image
    }
}

/// The pictures an answer shows on a line of their own; a tap opens them full screen.
struct AnswerImages: View {
    var pictures: [MediaLink]
    var size: CGFloat = 16
    @Environment(\.liveThreadID) private var threadID
    @State private var open: Int?

    var body: some View {
        let sources = pictures.map { $0.viewer(threadID) }
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(pictures.enumerated()), id: \.offset) { i, picture in
                AnswerImage(picture: picture, source: sources[i]) { open = i }
            }
        }
        .fullScreenCover(item: Binding(get: { open.map(Opened.init) }, set: { open = $0?.index })) { o in
            ImageViewer(images: sources.compactMap { $0 }, start: min(o.index, sources.compactMap { $0 }.count - 1))
        }
    }

    private struct Opened: Identifiable {
        var index: Int
        var id: Int { index }
    }
}

/// One picture of an answer, in its own proportions (a tall one no taller than `tallest`).
struct AnswerImage: View {
    var picture: MediaLink
    var source: ViewerImage?
    var open: () -> Void
    @Environment(AppModel.self) private var model
    @Environment(\.liveThreadID) private var threadID
    @State private var loaded: UIImage?
    @State private var failed = false

    /// Sharp across a phone's width.
    static let thumbPx = 1024
    private let tallest: CGFloat = 360

    private var reachable: Bool { picture.web != nil || model.serves(.media, on: threadID) }

    var body: some View {
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
                    MediaPlaceholder(symbol: failed ? "photo.badge.exclamationmark" : "photo",
                                     text: failed ? "Couldn't load \(picture.label)" : reachable ? nil : "\(picture.label) · on your Mac")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(.plain)
        .disabled(loaded == nil)
        .accessibilityLabel(picture.label)
        .accessibilityHint(loaded == nil ? "" : "Opens it full screen")
        .task(id: source) {
            guard let source, reachable else { return }
            let got = await source.load(model, maxPx: Self.thumbPx)
            withAnimation(.easeOut(duration: 0.2)) {
                loaded = got
                failed = got == nil
            }
        }
    }
}

/// Where a picture or video will be: a spinner while it comes, or a line saying why it won't.
struct MediaPlaceholder: View {
    var symbol: String
    /// Nil: it's on its way.
    var text: String?
    var height: CGFloat = 140

    var body: some View {
        VStack(spacing: 6) {
            if let text {
                Image(systemName: symbol).font(.system(size: 20))
                Text(text).font(.ui(.footnote)).lineLimit(1).truncationMode(.middle)
            } else {
                ProgressView().controlSize(.small)
            }
        }
        .foregroundStyle(Trek.muted)
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity)
        .frame(height: height)
        .background(Trek.foreground.opacity(0.05), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

/// A video an answer links on a line of its own: a file on the Mac comes over as a notes' video
/// does (poster, then it plays here); one on the web plays from there.
struct AnswerVideo: View {
    var link: MediaLink
    var size: CGFloat = 16
    @Environment(\.liveThreadID) private var threadID
    @State private var playing: URL?

    var body: some View {
        if let web = link.web {
            Button { playing = web } label: {
                HStack(spacing: 10) {
                    Image(systemName: "play.circle.fill").font(.system(size: 26)).foregroundStyle(Trek.foreground.opacity(0.85))
                    VStack(alignment: .leading, spacing: 2) {
                        Text(link.label).font(.ui(.subheadline).weight(.medium)).foregroundStyle(Trek.foreground).lineLimit(1)
                        Text(web.host() ?? "Video").font(.ui(.caption)).foregroundStyle(Trek.muted).lineLimit(1)
                    }
                    Spacer(minLength: 0)
                }
                .padding(12)
                .background(Trek.surface, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
            }
            .buttonStyle(.plain)
            .accessibilityHint("Plays it")
            .sheet(item: Binding(get: { playing.map(Playing.init) }, set: { playing = $0?.url })) { VideoSheet(url: $0.url) }
        } else {
            NoteFileCard(file: NoteFileRef(name: link.name, path: link.path), threadID: threadID)
        }
    }

    private struct Playing: Identifiable {
        var url: URL
        var id: URL { url }
    }
}

/// The videos a message names (`@/path/clip.mov`), each a card under the bubble.
struct AttachedVideos: View {
    var paths: [String]
    var threadID: String

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            ForEach(paths, id: \.self) { path in
                NoteFileCard(file: NoteFileRef(name: "", path: path), threadID: threadID)
                    .frame(maxWidth: 260)
            }
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

/// Under a finished `watch_video` row: the video, if it's still on the Mac, and the frames the
/// model saw in a strip; a frame opens full screen.
struct WatchedVideoView: View {
    var watch: WatchedMedia
    var threadID: String
    @State private var open: Int?

    static let framePx = 360
    private let frameHeight: CGFloat = 64

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let video = watch.video {
                NoteFileCard(file: NoteFileRef(name: (watch.source as NSString).lastPathComponent, path: video), threadID: threadID)
            }
            Text(watch.frames.count == 1 ? "The frame the agent saw" : watch.frames.isEmpty
                 ? "No frames: the video had no picture, or they were cleared" : "The \(watch.frames.count) frames the agent saw")
                .font(.ui(.caption))
                .foregroundStyle(Trek.muted)
            if !watch.frames.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(Array(watch.frames.enumerated()), id: \.offset) { i, frame in
                            Button { open = i } label: {
                                VStack(spacing: 3) {
                                    FrameThumb(source: .media(threadID: threadID, path: frame.path), height: frameHeight)
                                    Text(Self.clock(frame.t))
                                        .font(.code(size: 10.5))
                                        .foregroundStyle(Trek.muted)
                                }
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Frame at \(Self.clock(frame.t))")
                        }
                    }
                }
                .scrollClipDisabled()
            }
        }
        .fullScreenCover(item: Binding(get: { open.map(Opened.init) }, set: { open = $0?.index })) { o in
            ImageViewer(images: watch.frames.map { .media(threadID: threadID, path: $0.path) }, start: o.index)
        }
    }

    /// "1:05", "0:07", as the Mac's `media::clock`.
    static func clock(_ t: Double) -> String {
        let s = max(0, Int(t.rounded(.down)))
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }

    private struct Opened: Identifiable {
        var index: Int
        var id: Int { index }
    }
}

/// One frame, `height` tall in its own proportions.
private struct FrameThumb: View {
    var source: ViewerImage
    var height: CGFloat
    @Environment(AppModel.self) private var model
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        ZStack {
            Trek.foreground.opacity(0.06)
            if let image {
                Image(uiImage: image).resizable().scaledToFill()
            } else if failed {
                Image(systemName: "photo").foregroundStyle(Trek.muted)
            } else {
                ProgressView().controlSize(.mini)
            }
        }
        .frame(width: image.map { min(160, max(36, height * $0.size.width / max(1, $0.size.height))) } ?? height * 16 / 9, height: height)
        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        .task(id: source) {
            let got = await source.load(model, maxPx: WatchedVideoView.framePx)
            image = got
            failed = got == nil
        }
    }
}
