import Photos
import UIKit

// Keeping a picture a thread shows: a long press on one offers Save to Photos, Copy and Share…, as
// the system does for a picture anywhere else. Saving asks for add-only access to Photos
// (`NSPhotoLibraryAddUsageDescription`): Trek puts a picture there and never reads the library.

/// What a long press on a picture offers.
nonisolated enum PictureAct: CaseIterable, Sendable {
    case save, copy, share

    var title: String {
        switch self {
        case .save: "Save to Photos"
        case .copy: "Copy"
        case .share: "Share…"
        }
    }

    var symbol: String {
        switch self {
        case .save: "square.and.arrow.down"
        case .copy: "doc.on.doc"
        case .share: "square.and.arrow.up"
        }
    }
}

/// How saving a picture to Photos went.
nonisolated enum PictureSave: Equatable, Sendable {
    case saved
    /// Photos is off for Trek: refused before, or just now when asked.
    case refused
    /// This iPhone lets no app add to Photos (Screen Time, a profile at work).
    case restricted
    /// Photos took the request and couldn't keep the picture: why, in its words.
    case failed(String)

    /// Nothing was saved and Settings is where that changes: said in an alert, which can go there,
    /// not in a toast that's gone in a few seconds.
    var needsSettings: Bool { self == .refused }

    /// What to say about it: a toast's words, or an alert's message.
    var message: String {
        switch self {
        case .saved: "Saved to Photos"
        case .refused: "Saving a picture needs Photos turned on for Trek in Settings. Trek only adds pictures there; it never looks at your library."
        case .restricted: "This iPhone doesn't let apps add to Photos (Screen Time, or a profile from work or school)."
        case .failed(let why): why.isEmpty ? "Photos couldn't keep that picture." : "Photos couldn't keep that picture: \(why)"
        }
    }
}

/// The part of Photos that saving needs, so it can be tried without the library.
nonisolated protocol PhotoShelf {
    /// Whether Trek may add to Photos: not asked yet, allowed, or not.
    var access: PHAuthorizationStatus { get }
    /// Asks the user (the system's own prompt), once.
    func askAccess() async -> PHAuthorizationStatus
    func add(_ image: UIImage) async throws
}

/// Photos itself, asked only to add.
nonisolated struct PhotoLibraryShelf: PhotoShelf {
    var access: PHAuthorizationStatus { PHPhotoLibrary.authorizationStatus(for: .addOnly) }

    func askAccess() async -> PHAuthorizationStatus {
        await PHPhotoLibrary.requestAuthorization(for: .addOnly)
    }

    func add(_ image: UIImage) async throws {
        // Photos runs this on a queue of its own: it must not be taken for the main actor's.
        try await PHPhotoLibrary.shared().performChanges { @Sendable in
            _ = PHAssetChangeRequest.creationRequestForAsset(from: image)
        }
    }
}

nonisolated enum PictureKeeper {
    /// Puts `image` in Photos, asking for add-only access the first time. Nothing is added unless
    /// the user has said yes.
    static func save(_ image: UIImage, to shelf: any PhotoShelf = PhotoLibraryShelf()) async -> PictureSave {
        var access = shelf.access
        if access == .notDetermined { access = await shelf.askAccess() }
        switch access {
        case .authorized, .limited: break
        case .restricted: return .restricted
        default: return .refused
        }
        do {
            try await shelf.add(image)
            return .saved
        } catch {
            return .failed(error.localizedDescription)
        }
    }
}
