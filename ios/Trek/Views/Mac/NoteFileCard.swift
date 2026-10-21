import AVFoundation
import AVKit
import PDFKit
import QuickLook
import SwiftUI
import UIKit

/// A file a note links on a line of its own (`[name](attachments/…)`), as a card: a PDF's first
/// page with its name and page count (a tap reads it full screen), a video's poster with a play
/// badge (a tap plays it), anything else a chip that opens in Quick Look. The file comes from the
/// Mac a piece at a time into the phone's cache (`NoteFiles`), a progress ring showing how far;
/// a PDF or video small enough is fetched as the card appears, for its page or poster. Once here
/// it opens without the Mac. Not here and the Mac doesn't send notes' files (or none is
/// connected): the card just says it's on the Mac.
struct NoteFileCard: View {
    var file: NoteFileRef
    /// The note's qualified id: the file comes from its machine.
    var noteID: String? = nil
    @Environment(AppModel.self) private var model
    @State private var url: URL?
    @State private var look: Look?
    @State private var phase: Phase = .idle
    /// Open it once it's here (the tap that fetched it).
    @State private var openWhenHere = false
    @State private var reading: Shown?
    @State private var playing: Shown?
    @State private var quickLook: URL?

    private enum Phase: Equatable {
        case idle, loading, onMac
        case large(Int64)
        case failed(String)
    }

    /// What the card draws of a file that's here.
    nonisolated struct Look: Sendable {
        var image: UIImage?
        /// "12 pages", "1:24".
        var detail: String?
        var aspect: CGFloat?
    }

    private var loader: NoteFiles { NoteFiles.shared }
    /// The machine the note is on.
    private var link: HostLink? { noteID.flatMap { model.link(for: $0) } ?? model.selected }
    private var key: String { loader.scoped(file.path, on: link) }
    private var progress: Double? { phase == .loading ? loader.progress[key] ?? 0 : nil }
    private var size: Int64? {
        if let url, let bytes = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize { return Int64(bytes) }
        return loader.sizes[key]
    }

    var body: some View {
        Button(action: tap) {
            switch file.kind {
            case .pdf: pdfCard
            case .video: videoCard
            case .other: chip
            }
        }
        .buttonStyle(.plain)
        .disabled(url == nil && phase == .onMac)
        .accessibilityLabel(file.label)
        .accessibilityValue(accessibilityValue)
        .accessibilityHint(url == nil && phase == .onMac ? "" : "Opens it")
        .task(id: Key(path: key, serves: model.serves(.noteFiles, on: noteID))) { await appear() }
        .fullScreenCover(item: $reading) { shown in PDFSheet(url: shown.url, title: file.label) }
        .sheet(item: $playing) { shown in VideoSheet(url: shown.url) }
        .quickLookPreview($quickLook)
    }

    private struct Key: Hashable {
        var path: String
        var serves: Bool
    }

    // MARK: Fetching

    private func appear() async {
        if let here = loader.cached(file.path, on: link) {
            await arrived(here)
            return
        }
        guard model.serves(.noteFiles, on: noteID) else { phase = .onMac; return }
        // A page or a poster to show: fetched now when it's not too big.
        guard file.kind != .other else { phase = .idle; return }
        phase = .loading
        await take(loader.fetch(file.path, model: model, on: link, upTo: NoteFiles.autoLimit))
    }

    private func tap() {
        if let url { return open(url) }
        guard phase != .loading, phase != .onMac else { return }
        openWhenHere = true
        phase = .loading
        Task { await take(loader.fetch(file.path, model: model, on: link)) }
    }

    private func take(_ outcome: NoteFiles.Outcome) async {
        switch outcome {
        case .file(let here): await arrived(here)
        case .large(let total): phase = .large(total)
        case .failed(let why): phase = .failed(why); openWhenHere = false
        case .onMac: phase = .onMac; openWhenHere = false
        }
    }

    private func arrived(_ here: URL) async {
        let made = await Self.look(of: here, kind: file.kind)
        withAnimation(.easeOut(duration: 0.2)) {
            url = here
            look = made
            phase = .idle
        }
        if openWhenHere {
            openWhenHere = false
            open(here)
        }
    }

    private func open(_ url: URL) {
        switch file.kind {
        case .pdf: reading = Shown(url: url)
        case .video: playing = Shown(url: url)
        case .other: quickLook = url
        }
    }

    /// A PDF's first page and page count, or a video's first frame and length, made off the main actor.
    @concurrent nonisolated private static func look(of url: URL, kind: NoteFileRef.Kind) async -> Look? {
        switch kind {
        case .pdf:
            guard let doc = PDFDocument(url: url), let page = doc.page(at: 0) else { return nil }
            let box = page.bounds(for: .mediaBox)
            let thumb = page.thumbnail(of: CGSize(width: 360, height: 480), for: .mediaBox)
            let count = doc.pageCount
            return Look(image: thumb, detail: count == 1 ? "1 page" : "\(count) pages", aspect: box.height > 0 ? box.width / box.height : nil)
        case .video:
            let asset = AVURLAsset(url: url)
            let frames = AVAssetImageGenerator(asset: asset)
            frames.appliesPreferredTrackTransform = true
            frames.maximumSize = CGSize(width: 1024, height: 1024)
            let poster = try? await frames.image(at: CMTime(seconds: 0.1, preferredTimescale: 600)).image
            let seconds = (try? await asset.load(.duration)).map(CMTimeGetSeconds) ?? 0
            let image = poster.map { UIImage(cgImage: $0) }
            return Look(image: image, detail: seconds.isFinite && seconds > 0 ? Self.clock(seconds) : nil,
                        aspect: image.map { $0.size.height > 0 ? $0.size.width / $0.size.height : 16 / 9 })
        case .other:
            return nil
        }
    }

    nonisolated static func clock(_ seconds: Double) -> String {
        let s = Int(seconds.rounded())
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }

    private static func bytes(_ n: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: n, countStyle: .file)
    }

    // MARK: Cards

    /// What's said under the name.
    private var caption: String {
        let kind = file.kind == .pdf ? "PDF" : file.kind == .video ? "Video" : (file.ext.isEmpty ? "File" : file.ext.uppercased())
        switch phase {
        case .onMac where url == nil: return "On your Mac"
        case .failed: return "Couldn't load it. Tap to try again."
        case .large(let total): return "\(kind) · \(Self.bytes(total)) · Tap to download"
        case .loading:
            if let size { return "\(kind) · \(Self.bytes(size)) · Downloading…" }
            return "\(kind) · Downloading…"
        default:
            var parts = [kind]
            if let detail = look?.detail { parts.append(detail) }
            if let size { parts.append(Self.bytes(size)) }
            // AVPlayer can't play WebM: Quick Look still shows it to share or save.
            if file.ext == "webm" { parts.append("Plays on your Mac") } else if url == nil, file.kind == .other { parts.append("Tap to open") }
            return parts.joined(separator: " · ")
        }
    }

    private var accessibilityValue: String { caption }

    private var pdfCard: some View {
        HStack(spacing: 14) {
            ZStack {
                if let image = look?.image {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFit()
                        .background(.white)
                        .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
                        .shadow(color: .black.opacity(0.08), radius: 3, y: 1)
                } else {
                    RoundedRectangle(cornerRadius: 4, style: .continuous)
                        .fill(Trek.foreground.opacity(0.06))
                        .overlay { Image(FileIcons.file(file.path)).resizable().interpolation(.high).frame(width: 26, height: 26) }
                }
                if let progress { ProgressRing(fraction: progress) }
            }
            .frame(width: 78, height: 104)
            text
            Spacer(minLength: 0)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Trek.foreground.opacity(0.04), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    private var videoCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            Color.black
                .aspectRatio(min(max(look?.aspect ?? 16 / 9, 0.75), 2.4), contentMode: .fit)
                .overlay {
                    if let image = look?.image {
                        Image(uiImage: image).resizable().scaledToFill()
                    } else {
                        LinearGradient(colors: [Color(hex: 0x26262B), Color(hex: 0x111113)], startPoint: .top, endPoint: .bottom)
                    }
                }
                .overlay { badge }
                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
                .frame(maxWidth: .infinity, maxHeight: 300)
            text.padding(.horizontal, 2)
        }
        .contentShape(Rectangle())
    }

    @ViewBuilder
    private var badge: some View {
        if let progress {
            ProgressRing(fraction: progress, size: 44, on: .white)
        } else if phase != .onMac || url != nil {
            Image(systemName: "play.fill")
                .font(.system(size: 20, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 52, height: 52)
                .background(.black.opacity(0.35), in: Circle())
                .glassEffect(.regular, in: .circle)
        } else {
            Image(systemName: "film").font(.system(size: 24)).foregroundStyle(.white.opacity(0.6))
        }
    }

    private var chip: some View {
        HStack(spacing: 12) {
            Image(FileIcons.file(file.path))
                .resizable()
                .interpolation(.high)
                .frame(width: 26, height: 26)
                .opacity(url == nil && phase == .onMac ? 0.5 : 1)
            text
            Spacer(minLength: 0)
            if let progress {
                ProgressRing(fraction: progress, size: 22)
            } else if url == nil, phase != .onMac {
                Image(systemName: isFailed ? "arrow.clockwise" : "arrow.down.circle")
                    .font(.system(size: 17))
                    .foregroundStyle(Trek.muted)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Trek.foreground.opacity(0.04), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var text: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(file.label)
                .font(.ui(.subheadline).weight(.semibold))
                .foregroundStyle(url == nil && phase == .onMac ? Trek.muted : Trek.foreground)
                .lineLimit(2)
            Text(caption)
                .font(.ui(.footnote).monospacedDigit())
                .foregroundStyle(isFailed ? Trek.failed : Trek.muted)
                .lineLimit(2)
        }
    }

    private var isFailed: Bool { if case .failed = phase { true } else { false } }
}

/// How much of a file is here: a ring filling clockwise.
private struct ProgressRing: View {
    var fraction: Double
    var size: CGFloat = 34
    var on: Color = Trek.foreground

    var body: some View {
        ZStack {
            Circle().stroke(on.opacity(0.2), lineWidth: 3)
            Circle()
                .trim(from: 0, to: max(0.03, fraction))
                .stroke(on, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                .rotationEffect(.degrees(-90))
                .animation(.easeOut(duration: 0.2), value: fraction)
        }
        .frame(width: size, height: size)
        .accessibilityLabel("Downloading")
        .accessibilityValue("\(Int(fraction * 100)) percent")
    }
}

/// A file a sheet shows.
private struct Shown: Identifiable {
    var url: URL
    var id: URL { url }
}

// MARK: Viewers

/// A PDF full screen, as PDFKit draws it: pinch to zoom, pages one under another.
private struct PDFSheet: View {
    var url: URL
    var title: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            PDFKitView(url: url)
                .ignoresSafeArea(edges: .bottom)
                .navigationTitle(title)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Done", systemImage: "xmark") { dismiss() }
                    }
                    ToolbarItem(placement: .topBarTrailing) {
                        ShareLink(item: url)
                    }
                }
        }
    }
}

private struct PDFKitView: UIViewRepresentable {
    var url: URL

    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displayMode = .singlePageContinuous
        view.displayDirection = .vertical
        view.backgroundColor = .secondarySystemBackground
        view.document = PDFDocument(url: url)
        return view
    }

    func updateUIView(_ view: PDFView, context: Context) {
        if view.document?.documentURL != url { view.document = PDFDocument(url: url) }
    }
}

/// A video from the cache, playing as the sheet opens.
private struct VideoSheet: View {
    var url: URL
    @State private var player: AVPlayer?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VideoPlayer(player: player)
                .ignoresSafeArea(edges: .bottom)
                .background(.black)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Done", systemImage: "xmark") { dismiss() }
                    }
                    ToolbarItem(placement: .topBarTrailing) {
                        ShareLink(item: url)
                    }
                }
        }
        .onAppear {
            // Heard with the ring switch on silent, as a video someone chose to play.
            try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
            let p = AVPlayer(url: url)
            player = p
            p.play()
        }
        .onDisappear { player?.pause() }
    }
}
