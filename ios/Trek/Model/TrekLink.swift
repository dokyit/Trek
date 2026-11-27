import Foundation

/// A link to a thread as the Mac writes it in an answer and reads it back (`trek://thread/<id>`,
/// `deep_link.rs`). Tapped on the phone it used to land in the pairing-link reader, which called
/// it "incomplete or damaged".
nonisolated enum TrekLink {
    /// The thread `link` names, by the id its machine gave it; nil for any other link.
    static func thread(_ link: String) -> String? {
        guard let url = URLComponents(string: link), url.scheme?.lowercased() == "trek",
              url.host?.lowercased() == "thread" else { return nil }
        let id = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        return id.isEmpty ? nil : id
    }
}

extension AppModel {
    /// The listed thread a machine calls `id`, on whichever machine has it: its id here.
    func listedThread(_ id: String) -> String? {
        threads.first { $0.id == id || $0.id.hasSuffix(":\(id)") }?.id
    }

    /// Opens the thread `url` links to, or says it isn't listed here. False when `url` isn't a
    /// thread's link at all (a pairing link, a notification's): someone else's to read.
    @discardableResult
    func open(link url: URL) -> Bool {
        guard let id = TrekLink.thread(url.absoluteString) else { return false }
        if let listed = listedThread(id) {
            openRequest = listed
        } else {
            show("That thread isn't on this iPhone's list")
        }
        return true
    }
}
