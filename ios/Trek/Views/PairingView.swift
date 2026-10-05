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

                        if let error = model.pairingError {
                            Label(error, systemImage: "exclamationmark.circle.fill")
                                .font(.footnote)
                                .foregroundStyle(Trek.failed)
                                .multilineTextAlignment(.leading)
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
                model.pair(address: link.address, code: link.code)
            }
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
            Button {
                var addr = address.trimmingCharacters(in: .whitespaces)
                if !addr.contains(":") { addr += ":7420" }
                model.pair(address: addr, code: code)
            } label: {
                Text("Pair").fontWeight(.semibold).frame(maxWidth: .infinity)
            }
            .buttonStyle(.glassProminent)
            .tint(Trek.foreground)
            .controlSize(.large)
            .disabled(address.isEmpty || code.count < 8)
            .padding(.top, 4)
        }
        .padding(16)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
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
