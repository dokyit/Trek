import Testing
import UIKit
@testable import Trek

/// The app icon picker: Night is the app's own icon, Paper and Glass the alternatives.
struct AppIconChoiceTests {
    @Test func nightIsTheAppsOwnIcon() {
        #expect(AppIconChoice.night.iconName == nil)
        #expect(AppIconChoice.of(nil) == .night)
        #expect(AppIconChoice.allCases.map(\.label) == ["Night", "Paper", "Glass"])
        #expect(AppIconChoice.of("AppIcon-Paper") == .paper)
        #expect(AppIconChoice.of("AppIcon-Glass") == .glass)
    }

    @Test func anIconOfAnEarlierVersionGoesBackToTheAppsOwn() {
        // "AppIcon-Night" was an alternative while the orange icon was the app's own.
        #expect(AppIconChoice.isRetired("AppIcon-Night"))
        #expect(AppIconChoice.of("AppIcon-Night") == .night)
        #expect(!AppIconChoice.isRetired(nil))
        for choice in AppIconChoice.allCases {
            #expect(!AppIconChoice.isRetired(choice.iconName))
        }
    }

    @Test func everyChoiceHasItsPreviewInTheCatalog() {
        for choice in AppIconChoice.allCases {
            #expect(UIImage(named: choice.preview) != nil, "\(choice.preview)")
        }
    }
}
