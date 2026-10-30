import SwiftUI

/// A newer Trek for this iPhone, above the threads (and in Settings): what's new in a line,
/// "Update now" where the Mac builds and installs on request, "Later"; then how the update goes,
/// and why it didn't in the Mac's words.
struct UpdateCard: View {
    @Environment(AppModel.self) private var model
    var update: AppUpdate.Available
    /// Settings shows it for as long as there's an update: nothing to put off there.
    var offersLater = true

    private var progress: PhoneUpdate? { model.phoneUpdate }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                icon
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.ui(.subheadline).weight(.semibold))
                        .contentTransition(.opacity)
                        .animation(.smooth, value: title)
                    Text(detail)
                        .font(.ui(.footnote))
                        .foregroundStyle(progress?.state == .failed ? Trek.failed : Trek.muted)
                        .fixedSize(horizontal: false, vertical: true)
                        .contentTransition(.opacity)
                        .animation(.smooth, value: detail)
                }
                Spacer(minLength: 0)
            }
            if let buttons { buttons.padding(.leading, 30) }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder private var icon: some View {
        Group {
            switch progress?.state {
            case .asking?, .building?, .installing?, .unknown?:
                ProgressView().controlSize(.small)
            case .installed?:
                Image(systemName: "checkmark.circle.fill").foregroundStyle(Trek.done)
            case .failed?:
                Image(systemName: "exclamationmark.circle.fill").foregroundStyle(Trek.failed)
            case nil:
                Image(systemName: "arrow.down.circle.fill").foregroundStyle(Trek.link)
            }
        }
        .frame(width: 20)
    }

    private var title: String {
        let version = progress?.version ?? update.version
        return switch progress?.state {
        case .asking?, .building?, .installing?, .unknown?: "Updating to Trek \(version)"
        case .installed?: "Trek \(version) is installed"
        case .failed?, nil: "Trek \(version) is available"
        }
    }

    /// The line under the title: what's new, else how it gets here; while it runs, where it is.
    private var detail: String {
        if let progress { return progress.text }
        if update.canUpdate {
            return update.note ?? "\(update.mac) builds it and installs it on this iPhone. Trek closes while it installs."
        }
        let note = update.note.map { "\($0) " } ?? ""
        return "\(note)Update available — your Mac will install it over Wi-Fi. Keep the iPhone on the same Wi-Fi as \(update.mac), or plugged in."
    }

    /// Above the threads, closing puts the offer off too (as "Later"): it was just tried.
    private func close() {
        if offersLater { model.later(update) } else { model.closeUpdate() }
    }

    private var buttons: AnyView? {
        switch progress?.state {
        case nil:
            guard update.canUpdate || offersLater else { return nil }
            return AnyView(HStack(spacing: 10) {
                if update.canUpdate {
                    Button { model.updatePhone(update) } label: {
                        Text("Update now").fontWeight(.semibold).foregroundStyle(Trek.background)
                    }
                    .buttonStyle(.glassProminent)
                    .tint(Trek.foreground)
                }
                if offersLater {
                    Button("Later") { model.later(update) }
                        .buttonStyle(.glass)
                }
            }
            .font(.ui(.subheadline)))
        case .failed?:
            return AnyView(HStack(spacing: 10) {
                if update.canUpdate {
                    Button { model.updatePhone(update) } label: {
                        Text("Try again").fontWeight(.semibold).foregroundStyle(Trek.background)
                    }
                    .buttonStyle(.glassProminent)
                    .tint(Trek.foreground)
                }
                Button("Close", action: close)
                    .buttonStyle(.glass)
            }
            .font(.ui(.subheadline)))
        case .installed?:
            return AnyView(Button("Close", action: close)
                .buttonStyle(.glass)
                .font(.ui(.subheadline)))
        default:
            return nil
        }
    }
}
