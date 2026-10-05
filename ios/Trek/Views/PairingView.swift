import SwiftUI
import VisionKit

/// First run: pair with Trek on a Mac (scan its QR code, or type the address and code), or look
/// around in demo mode.
struct PairingView: View {
    @Environment(AppModel.self) private var model
    @State private var scanning = false
    @State private var manual = false
    @State private var address = ""
    @State private var code = ""
    @State private var fingerprint = ""
    @State private var confirmPlain = false

    var body: some View {
        ZStack {
            ScrollView {
                VStack(spacing: 28) {
                    VStack(spacing: 14) {
                        Image("TrekMark")
                            .resizable()
                            .scaledToFit()
                            .frame(width: 84, height: 72)
                            .shadow(color: Color(hex: 0xFF8A3D).opacity(0.5), radius: 24)
                        Text("Trek")
                            .font(.system(size: 40, weight: .bold))
                            .tracking(-0.8)
                        Text("Every agent. One trail.\nKeep your agents moving from anywhere.")
                            .multilineTextAlignment(.center)
                            .font(.body)
                            .foregroundStyle(Trek.muted)
                    }
                    .padding(.top, 70)

                    steps

                    VStack(spacing: 12) {
                        Button {
                            scanning = true
                        } label: {
                            Label("Scan pairing code", systemImage: "qrcode.viewfinder")
                                .font(.body.weight(.semibold))
                                .foregroundStyle(Trek.background)
                                .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.glassProminent)
                        .tint(Trek.foreground)
                        .controlSize(.extraLarge)

                        if let error = model.pairingError {
                            Label(error, systemImage: "exclamationmark.circle.fill")
                                .font(.footnote)
                                .foregroundStyle(Trek.failed)
                                .multilineTextAlignment(.leading)
                        }

                        if manual {
                            manualForm.transition(.opacity.combined(with: .move(edge: .top)))
                        } else {
                            Button {
                                withAnimation(.snappy) { manual = true }
                            } label: {
                                Text("Enter address and code").frame(maxWidth: .infinity)
                            }
                            .buttonStyle(.glass)
                            .controlSize(.extraLarge)
                        }

                        Button("Explore the demo") { model.startDemo() }
                            .font(.subheadline.weight(.medium))
                            .foregroundStyle(Trek.muted)
                            .padding(.top, 6)
                    }
                    .padding(.horizontal, 24)
                }
                .padding(.bottom, 40)
            }
            .scrollBounceBehavior(.basedOnSize)
            .background { RidgeBackdrop(height: 560) }

            if model.pairing {
                Color.black.opacity(0.2).ignoresSafeArea()
                ProgressView("Pairing…")
                    .padding(24)
                    .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 20))
            }
        }
        .sheet(isPresented: $scanning) {
            ScannerSheet { link in
                scanning = false
                // Same confirmation as a deep link: name, address and fingerprint first.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { model.offer(link) }
            }
        }
        .alert("Unencrypted connection", isPresented: $confirmPlain) {
            Button("Connect without encryption", role: .destructive) { pairTyped(.plain) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(Self.plainWarning)
        }
    }

    private var steps: some View {
        VStack(alignment: .leading, spacing: 14) {
            step(1, "Open Trek on your Mac", "Settings › Mobile › Pair iPhone")
            step(2, "Scan the code it shows", "Same Wi-Fi, or anywhere over Tailscale")
            step(3, "Approve, steer and start work", "Agents keep running on the Mac")
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
        .padding(.horizontal, 24)
    }

    private func step(_ n: Int, _ title: String, _ detail: String) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(n)")
                .font(.footnote.weight(.bold).monospacedDigit())
                .foregroundStyle(Trek.ember)
                .frame(width: 24, height: 24)
                .background(Trek.ember.opacity(0.14), in: Circle())
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.subheadline.weight(.semibold))
                Text(detail).font(.footnote).foregroundStyle(Trek.muted)
            }
        }
    }

    private var manualForm: some View {
        VStack(spacing: 10) {
            TextField("Mac address, e.g. 192.168.1.20:7420", text: $address)
                .textContentType(.URL)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            Divider()
            TextField("Pairing code, e.g. K7Q2-9XMV", text: $code)
                .textInputAutocapitalization(.characters)
                .autocorrectionDisabled()
                .font(.body.monospaced())
            Divider()
            HStack(spacing: 8) {
                TextField("Fingerprint, e.g. ABCD-1234", text: $fingerprint)
                    .textInputAutocapitalization(.characters)
                    .autocorrectionDisabled()
                    .font(.body.monospaced())
                Image(systemName: "lock.fill").font(.footnote).foregroundStyle(Trek.muted)
            }
            Text(fingerprintHint)
                .font(.caption)
                .foregroundStyle(fingerprintInvalid ? Trek.failed : Trek.muted)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button {
                if let prefix = Fingerprint.typedPrefix(fingerprint) {
                    pairTyped(.tls(.prefix(prefix)))
                } else {
                    confirmPlain = true
                }
            } label: {
                Text("Pair").fontWeight(.semibold).frame(maxWidth: .infinity)
            }
            .buttonStyle(.glassProminent)
            .tint(Trek.foreground)
            .controlSize(.large)
            .disabled(address.trimmingCharacters(in: .whitespaces).isEmpty || code.count < 8 || fingerprintInvalid)
            .padding(.top, 4)
        }
        .padding(16)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
    }
}

extension PairingView {
    static let plainWarning = "Without a fingerprint this iPhone can't check that it's really talking to your Mac, and everything you send and see, approvals included, crosses the network unencrypted. Only continue for an older Trek on a network you trust."

    private var fingerprintInvalid: Bool {
        !fingerprint.trimmingCharacters(in: .whitespaces).isEmpty && Fingerprint.typedPrefix(fingerprint) == nil
    }

    private var fingerprintHint: String {
        fingerprintInvalid ? "8 characters, 0–9 and A–F, as your Mac shows it."
            : "Shown beside the code on your Mac. It proves this is your Mac."
    }

    private func pairTyped(_ transport: Transport) {
        var addr = address.trimmingCharacters(in: .whitespaces)
        if !addr.contains(":") { addr += ":7420" }
        model.pair(address: addr, code: code.trimmingCharacters(in: .whitespaces), transport: transport)
    }
}

/// "Pair with Tobias's MacBook Pro?": a pairing link (deep link or QR code) never pairs on its
/// own. The user sees which Mac, where, and its fingerprint (to compare with the Mac's screen)
/// and confirms; a link without a fingerprint gets the "Unencrypted connection" warning instead.
struct PairingConfirmSheet: View {
    var link: PairingLink
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var height: CGFloat = 440

    private var macName: String { link.name ?? "this Mac" }
    private var replacing: String? {
        guard let paired = PairedMac.load(), paired.hostId != link.hostId else { return nil }
        return paired.hostName
    }

    var body: some View {
        VStack(spacing: 18) {
            Image(systemName: "laptopcomputer.and.iphone")
                .font(.system(size: 34, weight: .medium))
                .foregroundStyle(Trek.foreground)
                .padding(.top, 30)
            VStack(spacing: 6) {
                Text("Pair with \(macName)?")
                    .font(.title2.weight(.semibold))
                    .multilineTextAlignment(.center)
                Text(link.address)
                    .font(.subheadline.monospaced())
                    .foregroundStyle(Trek.muted)
            }

            if let fp = link.fingerprint {
                VStack(spacing: 6) {
                    HStack(spacing: 8) {
                        Image(systemName: "lock.fill").font(.subheadline).foregroundStyle(Trek.done)
                        Text(Fingerprint.short(fp))
                            .font(.system(.title2, design: .monospaced).weight(.semibold))
                            .tracking(1)
                    }
                    Text("Check that your Mac shows this fingerprint beside the code.")
                        .font(.footnote)
                        .foregroundStyle(Trek.muted)
                        .multilineTextAlignment(.center)
                }
                .padding(14)
                .frame(maxWidth: .infinity)
                .background(Trek.foreground.opacity(0.05), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    Label("Unencrypted connection", systemImage: "lock.open.fill")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(Trek.approval)
                    Text(PairingView.plainWarning)
                        .font(.footnote)
                        .foregroundStyle(Trek.foreground.opacity(0.8))
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Trek.approval.opacity(0.1), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Trek.approval.opacity(0.4), lineWidth: 1))
            }

            if let replacing {
                Text("This replaces the pairing with \(replacing).")
                    .font(.footnote)
                    .foregroundStyle(Trek.muted)
            }

            VStack(spacing: 10) {
                if link.fingerprint != nil {
                    Button {
                        model.pair(address: link.address, code: link.code, transport: link.transport)
                    } label: {
                        Text("Pair").fontWeight(.semibold).foregroundStyle(Trek.background).frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.glassProminent)
                    .tint(Trek.foreground)
                    .controlSize(.extraLarge)
                } else {
                    Button {
                        model.pair(address: link.address, code: link.code, transport: .plain)
                    } label: {
                        Text("Pair without encryption").fontWeight(.medium).foregroundStyle(Trek.approval).frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.glass)
                    .controlSize(.extraLarge)
                }
                Button {
                    dismiss()
                } label: {
                    Text("Cancel").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glass)
                .controlSize(.extraLarge)
            }
            .padding(.top, 8)
        }
        .padding(.horizontal, 24)
        .padding(.bottom, 12)
        .fixedSize(horizontal: false, vertical: true)
        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        .frame(maxHeight: .infinity, alignment: .top)
        .presentationDetents([.height(height)])
        .presentationDragIndicator(.visible)
        .presentationBackground(Trek.background)
    }
}


/// The camera, looking for a `trek://pair` QR code.
struct ScannerSheet: View {
    var found: (PairingLink) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Group {
                if DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
                    QRScanner(found: found).ignoresSafeArea()
                } else {
                    ContentUnavailableView("No camera here",
                                           systemImage: "camera.metering.unknown",
                                           description: Text("Scanning needs a camera. Enter the address and code your Mac shows instead."))
                }
            }
            .navigationTitle("Scan pairing code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { dismiss() } label: { Image(systemName: "xmark") }
                }
            }
        }
    }
}

private struct QRScanner: UIViewControllerRepresentable {
    var found: (PairingLink) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced,
                                           isHighlightingEnabled: true)
        vc.delegate = context.coordinator
        try? vc.startScanning()
        return vc
    }

    func updateUIViewController(_ vc: DataScannerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        var found: (PairingLink) -> Void
        var done = false

        init(found: @escaping (PairingLink) -> Void) { self.found = found }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for item in items {
                if case .barcode(let code) = item, let s = code.payloadStringValue, let link = PairingLink(s), !done {
                    done = true
                    scanner.stopScanning()
                    found(link)
                }
            }
        }
    }
}
