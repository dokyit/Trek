import Foundation
import SwiftUI

/// Updating this app: the offer (a paired Mac knows a newer Trek), "Later", and "Update now",
/// which asks the Mac to build the app and install it here (`update_phone`). The Mac says how it
/// goes to every phone (`phone_update`); installing closes this app, so the one that asked usually
/// last hears "Installing…".
extension AppModel {
    /// A newer Trek than this app, whether or not the user put it off.
    var appUpdate: AppUpdate.Available? { AppUpdate.available(in: links) }

    /// The offer to show above the threads: one not put off, or any while an update runs.
    var offeredUpdate: AppUpdate.Available? {
        guard let update = appUpdate else { return nil }
        if phoneUpdate != nil { return update }
        return updateDismissal?.hides(update.version) == true ? nil : update
    }

    /// "Later": out of sight for this version until the next launch or a day.
    func later(_ update: AppUpdate.Available) {
        let d = AppUpdate.Dismissal(version: update.version, at: Date(), launch: AppUpdate.Dismissal.launch)
        d.save()
        withAnimation(.snappy) {
            updateDismissal = d
            phoneUpdate = nil
        }
    }

    /// "Update now": the Mac builds the app and installs it on this iPhone.
    func updatePhone(_ update: AppUpdate.Available) {
        guard let link = link(id: update.hostId), link.serves(.updatePhone) else { return }
        withAnimation(.snappy) { phoneUpdate = PhoneUpdate(state: .asking, version: update.version) }
        link.request(.updatePhone(version: update.version)) { [weak self] reply in
            guard let self else { return }
            switch reply {
            case .error(_, _, let message):
                withAnimation(.snappy) { self.phoneUpdate = PhoneUpdate(state: .failed, version: update.version, message: message) }
            default:
                // Started: the pushes say how it goes, unless one already has.
                if self.phoneUpdate?.state == .asking {
                    withAnimation(.snappy) { self.phoneUpdate = PhoneUpdate(state: .building, version: update.version) }
                }
            }
        }
    }

    /// Done with a finished or failed update's card.
    func closeUpdate() {
        withAnimation(.snappy) { phoneUpdate = nil }
    }

    /// A Mac says how an update goes. A state this app doesn't know changes nothing.
    func heard(_ update: PhoneUpdate) {
        guard update.state != .unknown, update.state != .asking else { return }
        withAnimation(.snappy) { phoneUpdate = update }
    }
}
