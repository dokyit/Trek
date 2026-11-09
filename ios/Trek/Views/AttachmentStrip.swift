import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// What's picked to go with a message, above a composer: photos (a tap opens them full screen),
/// videos (a frame of it with a play badge and its length) and files (icon, name, size), each with
/// a button to take it off. While they go up (`locked`), how far each has got, and none comes off.
struct AttachmentStrip: View {
    @Binding var photos: [PickedPhoto]
    @Binding var files: [PickedFile]
    var locked = false
    @State private var open: Int?
    @MotionAllowed private var motion

    static let tile: CGFloat = 56

    /// A tile grows in from a little smaller as it's added and shrinks away as it's taken off, the
    /// others closing up after it (never from nothing: 0.9).
    private var tileTransition: AnyTransition {
        motion ? .scale(scale: 0.9).combined(with: .opacity) : .opacity
    }

    var body: some View {
        SidewaysScroll {
            HStack(spacing: 8) {
                ForEach(photos) { p in
                    Button { open = photos.firstIndex { $0.id == p.id } } label: {
                        Image(uiImage: p.image)
                            .resizable()
                            .scaledToFill()
                            .frame(width: Self.tile, height: Self.tile)
                            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            // The tile, not the whole picture its fill crops: taps stay on their own photo.
                            .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    }
                    .buttonStyle(.press)
                    .accessibilityLabel("Attached photo")
                    .accessibilityHint("Opens it full screen")
                    // Beside the button, not in its label: a tap here only removes.
                    .overlay(alignment: .topTrailing) {
                        remove("Remove photo") { photos.removeAll { $0.id == p.id } }
                    }
                    .transition(tileTransition)
                }
                ForEach(files) { f in
                    Group {
                        switch f.kind {
                        case .video: VideoTile(file: f)
                        case .file: FileTile(file: f)
                        }
                    }
                    .overlay(alignment: .topTrailing) {
                        remove(f.kind == .video ? "Remove video" : "Remove \(f.name)") {
                            files.removeAll { $0.id == f.id }
                            f.discard()
                        }
                    }
                    .transition(tileTransition)
                }
            }
            .padding(.top, 8)
            .padding(.horizontal, 4)
        }
        .fullScreenCover(item: Binding(get: { open.map(Opened.init) }, set: { open = $0?.index })) { o in
            ImageViewer(images: photos.map { .picked($0.image) }, start: o.index)
        }
    }

    @ViewBuilder
    private func remove(_ label: String, _ action: @escaping () -> Void) -> some View {
        if !locked {
            Button {
                withAnimation(.snappy) { action() }
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 18))
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.white, .black.opacity(0.6))
            }
            .buttonStyle(.pressRound)
            .offset(x: 6, y: -6)
            .accessibilityLabel(label)
            .transition(.opacity)
        }
    }

    private struct Opened: Identifiable {
        var index: Int
        var id: Int { index }
    }
}

/// A video on its way: a frame of it, a play badge (a ring while it goes up), its length.
private struct VideoTile: View {
    var file: PickedFile

    var body: some View {
        ZStack {
            Color.black
            if let frame = file.thumbnail {
                Image(uiImage: frame).resizable().scaledToFill()
            } else {
                LinearGradient(colors: [Color(hex: 0x26262B), Color(hex: 0x111113)], startPoint: .top, endPoint: .bottom)
            }
            if let progress = file.progress {
                Color.black.opacity(0.35)
                ProgressRing(fraction: progress, size: 26, on: .white, label: "Sending")
            } else if file.duration == nil {
                Image(systemName: "play.fill")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 24, height: 24)
                    .background(.black.opacity(0.45), in: Circle())
            }
        }
        .frame(width: AttachmentStrip.tile, height: AttachmentStrip.tile)
        // The play mark and the length together at the foot, as Photos marks a video.
        .overlay(alignment: .bottom) {
            if let duration = file.duration, file.progress == nil {
                HStack(spacing: 2) {
                    Image(systemName: "play.fill").font(.system(size: 7, weight: .bold))
                    Text(PickedFile.clock(duration))
                        .font(.ui(size: 10, weight: .semibold, relativeTo: .caption2).monospacedDigit())
                }
                .foregroundStyle(.white)
                .padding(.horizontal, 5)
                .padding(.vertical, 1.5)
                .background(.black.opacity(0.55), in: Capsule())
                .padding(.bottom, 4)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Attached video")
        .accessibilityValue(value)
    }

    private var value: String {
        if let progress = file.progress { return "Sending, \(Int(progress * 100)) percent" }
        return [file.duration.map(PickedFile.clock), fileSizeWords(file.size)].compactMap { $0 }.joined(separator: ", ")
    }
}

/// A file on its way: its icon (a ring while it goes up), name and size.
private struct FileTile: View {
    var file: PickedFile

    var body: some View {
        HStack(spacing: 9) {
            ZStack {
                if let progress = file.progress {
                    ProgressRing(fraction: progress, size: 24, label: "Sending")
                } else {
                    Image(FileIcons.file(file.name))
                        .resizable()
                        .interpolation(.high)
                        .frame(width: 24, height: 24)
                }
            }
            .frame(width: 26, height: 26)
            VStack(alignment: .leading, spacing: 1) {
                Text(file.name)
                    .font(.ui(.footnote).weight(.semibold))
                    .foregroundStyle(Trek.foreground)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(file.progress.map { "\(Int($0 * 100))% sent" } ?? fileSizeWords(file.size))
                    .font(.ui(.caption2).monospacedDigit())
                    .foregroundStyle(Trek.muted)
                    .contentTransition(.numericText())
            }
            .frame(maxWidth: 150, alignment: .leading)
        }
        .padding(.leading, 10)
        .padding(.trailing, 14)
        .frame(height: AttachmentStrip.tile)
        .background(Trek.foreground.opacity(0.05), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Attached file \(file.name)")
        .accessibilityValue(file.progress.map { "Sending, \(Int($0 * 100)) percent" } ?? fileSizeWords(file.size))
    }
}

/// What the composer's + offers: photos and videos from the library, files from the Files app.
/// Without a Mac that takes files (`uploads`), the photo picker as it always was.
struct AttachPicker<Face: View>: View {
    @Binding var photos: [PickedPhoto]
    @Binding var files: [PickedFile]
    /// The Mac takes videos and files.
    var uploads: Bool
    @ViewBuilder var label: () -> Face
    @State private var picked: [PhotosPickerItem] = []
    @State private var pickingMedia = false
    @State private var pickingFiles = false
    @Environment(AppModel.self) private var model: AppModel?

    /// At most this many photos and videos in one go, as before.
    static var limit: Int { 4 }

    var body: some View {
        Group {
            if uploads {
                Menu {
                    Button { pickingMedia = true } label: {
                        Label("Photos & Videos", systemImage: "photo.on.rectangle")
                    }
                    Button { pickingFiles = true } label: {
                        Label("Files", systemImage: "folder")
                    }
                } label: {
                    label()
                }
                .accessibilityLabel("Attach")
                .accessibilityHint("Photos, videos or files")
                .photosPicker(isPresented: $pickingMedia, selection: $picked, maxSelectionCount: Self.limit,
                              matching: .any(of: [.images, .videos]), preferredItemEncoding: .current)
                .fileImporter(isPresented: $pickingFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
                    switch result {
                    case .success(let urls):
                        Task {
                            let loaded = await PickedFile.load(files: urls, model: model)
                            withAnimation(.snappy) { files.append(contentsOf: loaded) }
                        }
                    case .failure(let error):
                        model?.show(error.localizedDescription, error: true)
                    }
                }
            } else {
                Button { pickingMedia = true } label: { label() }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Add photos")
                    .photosPicker(isPresented: $pickingMedia, selection: $picked, maxSelectionCount: Self.limit, matching: .images)
            }
        }
        .onChange(of: picked) {
            let items = picked
            guard !items.isEmpty else { return }
            picked = []
            Task {
                let images = await PickedPhoto.load(items.filter { !PickedFile.isVideo($0) })
                let videos = await PickedFile.loadVideos(items)
                withAnimation(.snappy) {
                    photos.append(contentsOf: images)
                    files.append(contentsOf: videos)
                }
                if videos.count < items.count(where: PickedFile.isVideo) {
                    model?.show("A video couldn't be read from the photo library", error: true)
                }
            }
        }
    }
}

/// The files a message ends with, under its bubble: what the phone sent (or the Mac's composer
/// mentioned), by name, as the bubble's own text would only show a long path on the Mac.
struct MessageFileChips: View {
    var paths: [String]

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            ForEach(paths, id: \.self) { path in
                let name = MessageFiles.displayName(path)
                HStack(spacing: 8) {
                    Image(FileIcons.file(name))
                        .resizable()
                        .interpolation(.high)
                        .frame(width: 18, height: 18)
                        .accessibilityHidden(true)
                    Text(name)
                        .font(.ui(.footnote).weight(.medium))
                        .foregroundStyle(Trek.foreground)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .padding(.horizontal, 11)
                .padding(.vertical, 7)
                .background(Trek.surface, in: Capsule())
                .overlay(Capsule().strokeBorder(Trek.border, lineWidth: 0.5))
                .contextMenu {
                    Button("Copy Path on the Mac", systemImage: "doc.on.doc") { UIPasteboard.general.string = path }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Attached file \(name)")
            }
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, 48)
    }
}
