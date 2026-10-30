import SwiftUI
import Testing
@testable import Trek

/// Every card state drawn at 3x, light and dark, into `TREK_CARD_SHOTS` (set it with
/// `TEST_RUNNER_TREK_CARD_SHOTS=<dir> xcodebuild test …`), to put beside the Figma frames on
/// "Phone · cards". Without it, the cards are still drawn (each must lay out), and nothing is kept.
@MainActor
struct CardShots {
    @Test func everyStateDraws() throws {
        let dir = ProcessInfo.processInfo.environment["TREK_CARD_SHOTS"].map { URL(fileURLWithPath: $0) }
        if let dir { try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true) }
        for state in DemoMac.cardStates {
            for scheme in [ColorScheme.light, .dark] {
                let card = DemoMac.card(state)
                // As the transcript lays it out: a 402 pt phone, 20 pt each side.
                let view = LiveCard(card: card, act: { _ in })
                    .frame(width: 362)
                    .padding(20)
                    .background(Trek.background)
                    .environment(\.colorScheme, scheme)
                let renderer = ImageRenderer(content: view)
                renderer.scale = 3
                let image = try #require(renderer.uiImage, "\(state) draws")
                #expect(image.size.height > 60, "\(state) has a body")
                if let dir, let png = image.pngData() {
                    try png.write(to: dir.appendingPathComponent("\(state)-\(scheme == .dark ? "dark" : "light").png"))
                }
            }
        }
    }
}
