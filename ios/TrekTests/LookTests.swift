import Foundation
import SwiftUI
import Testing
import UIKit
@testable import Trek

/// Trek themes and background art on the phone: the built-ins as the Mac ships them, the Mac's
/// look as the snapshot brings it, the colours a look gives the app, and the scrim over the art.
struct LookTests {
    /// The repository's root, from this file's place in it (tests run on the Mac's simulator, which
    /// reads the Mac's files).
    private static let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

    /// A theme file's top-level keys and tables, as far as Trek's built-ins use TOML.
    private static func toml(_ id: String) throws -> [String: [String: String]] {
        let url = root.appending(path: "crates/trek-core/themes/\(id).toml")
        var out: [String: [String: String]] = [:]
        var table = ""
        for raw in try String(contentsOf: url, encoding: .utf8).split(separator: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("#") { continue }
            if line.hasPrefix("[") { table = String(line.dropFirst().dropLast()); continue }
            let kv = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
            var value = kv[1]
            if let hash = value.range(of: " #") { value = String(value[..<hash.lowerBound]).trimmingCharacters(in: .whitespaces) }
            out[table, default: [:]][kv[0]] = value.trimmingCharacters(in: CharacterSet(charactersIn: "\""))
        }
        return out
    }

    @Test(arguments: ["console", "journal", "alpenglow"])
    func theBuiltInsAreTheMacsFiles(id: String) throws {
        let file = try Self.toml(id)
        let look = try #require(Look.builtIn(id))
        #expect(look.name == file[""]?["name"])
        #expect(look.description == file[""]?["description"])
        #expect(look.mode.rawValue == file[""]?["mode"])
        #expect(look.uiFont.rawValue == file["fonts"]?["ui"])
        #expect(look.conversationFont.rawValue == file["fonts"]?["conversation"])
        #expect(look.corners == Double(file["shape"]?["corners"] ?? ""))
        #expect(look.density.rawValue == file["shape"]?["density"])
        #expect(look.motion.rawValue == file["motion"]?["style"])
        // Sizes: the file's, else its density's (themes.rs `Density::sizes`).
        let sizes: [String: (Double, Double)] = ["compact": (13, 13.5), "regular": (14, 14.5), "roomy": (15, 16)]
        let bySize = try #require(sizes[look.density.rawValue])
        #expect(look.uiSize == (file["fonts"]?["ui_size"].flatMap(Double.init) ?? bySize.0))
        #expect(look.conversationSize == (file["fonts"]?["conversation_size"].flatMap(Double.init) ?? bySize.1))
        #expect(look.dark == Look.Palette(file["dark"] ?? [:]))
        #expect(look.light == Look.Palette(file["light"] ?? [:]))
        #expect(!look.dark.isEmpty && !look.light.isEmpty)
        #expect(look.art == file["background"]?["art"])
    }

    @Test func everyBuiltInIsOnTheMacAndTrekComesFirst() throws {
        #expect(Look.builtIn.map(\.id) == ["trek", "console", "journal", "alpenglow"])
        let names = try FileManager.default.contentsOfDirectory(atPath: Self.root.appending(path: "crates/trek-core/themes").path)
            .filter { $0.hasSuffix(".toml") }.map { String($0.dropLast(5)) }
        #expect(Set(names) == Set(Look.builtIn.dropFirst().map(\.id)))
        #expect(Look.trek.dark.isEmpty && Look.trek.light.isEmpty)
    }

    @Test func colours() {
        #expect(Look.colour("#6BDB9A") == 0x6BDB9AFF)
        #expect(Look.colour("#6BDB9A80") == 0x6BDB9A80)
        #expect(Look.colour("6BDB9A") == nil)
        #expect(Look.colour("#6BDB9") == nil)
        #expect(Look.colour("#GGGGGG") == nil)
    }

    // MARK: The snapshot's look

    /// As docs/MOBILE.md shows it.
    private static let journalSnapshot = """
    {"type":"snapshot","threads":[],"projects":[],"agents":[],
     "look":{"theme":{"id":"journal","name":"Journal","description":"Warm paper, the conversation set in a serif, terracotta accents.",
                      "mode":"light","ui_font":"sans","conversation_font":"serif","ui_size":15.0,"conversation_size":16.0,
                      "corners":1.25,"density":"roomy","motion":"calm",
                      "dark":{"accent":"#E0895AFF","window":"#14110EFF","text":"#ECE4D8FF"},
                      "light":{"accent":"#B0552CFF","on_accent":"#FFFFFFFF","window":"#FBF8F2FF"}},
             "background":"builtin:paintings/matterhorn","placement":"chat","dim":0.5}}
    """

    @Test func theSnapshotBringsTheMacsLook() throws {
        guard case .snapshot(let s) = try ServerMessage.decode(Data(Self.journalSnapshot.utf8)) else {
            Issue.record("not a snapshot")
            return
        }
        let mac = try #require(s.look)
        let look = mac.look
        #expect(look.id == "journal")
        #expect(look.conversationFont == .serif)
        #expect(look.uiSize == 15 && look.conversationSize == 16)
        #expect(look.density == .roomy && look.motion == .calm && look.mode == .light)
        #expect(look.corners == 1.25)
        #expect(look.light.accent == 0xB0552CFF)
        // `on_accent` comes through the decoder's snake_case conversion of a dictionary's keys.
        #expect(look.light.onAccent == 0xFFFFFFFF)
        #expect(look.dark.window == 0x14110EFF)
        #expect(look.dark.sidebar == nil)
        let art = try #require(mac.art)
        #expect(art.piece.key == "matterhorn" && art.placement == .chat && art.dim == 0.5)
    }

    @Test func anOlderMacSendsNoLook() throws {
        let json = #"{"type":"snapshot","threads":[],"projects":[],"agents":[]}"#
        guard case .snapshot(let s) = try ServerMessage.decode(Data(json.utf8)) else {
            Issue.record("not a snapshot")
            return
        }
        #expect(s.look == nil)
        // Match your Mac then draws Trek's own look and no art.
        let (look, art) = LookStore.resolve(theme: AppearanceKey.matchMac, art: AppearanceKey.matchMac, mac: s.look, placement: nil, dim: nil)
        #expect(look == .trek)
        #expect(art == nil)
    }

    @Test func aLookThePhoneCantReadCostsNothingElse() throws {
        let json = #"{"type":"snapshot","threads":[],"projects":[],"agents":[],"look":{"theme":"journal"},"full_access":true}"#
        guard case .snapshot(let s) = try ServerMessage.decode(Data(json.utf8)) else {
            Issue.record("not a snapshot")
            return
        }
        #expect(s.look == nil)
        #expect(s.fullAccess == true)
    }

    @Test func aNewerMacsValuesReadAsTheNearestKnown() throws {
        let json = """
        {"theme":{"id":"neon","name":"Neon","mode":"twilight","ui_font":"cursive","conversation_font":"mono",
                  "ui_size":40,"conversation_size":-3,"corners":9,"density":"airy","motion":"bouncy","future":true,
                  "dark":{"accent":"#FF00FF","glow":"#FFFFFF","text":"not a colour"}},
         "background":"builtin:paintings/elsewhere","placement":"floating","dim":0.3,"extra":[1,2]}
        """
        let mac = try JSONDecoder.snake.decode(MacLook.self, from: Data(json.utf8))
        let look = mac.look
        #expect(look.mode == .any && look.uiFont == .sans && look.conversationFont == .mono)
        #expect(look.uiSize == 18 && look.conversationSize == 12 && look.corners == 2)
        #expect(look.density == .regular && look.motion == .standard)
        #expect(look.dark.accent == 0xFF00FFFF && look.dark.text == nil)
        #expect(mac.art == nil, "art this phone hasn't got isn't drawn")
    }

    @Test func aDemoMacsLookGoesThereAndBack() throws {
        let art = ArtChoice(piece: try #require(ArtPiece.named("builtin:ascii/braille")), placement: .sidebar, dim: 0.2)
        for look in Look.builtIn {
            let mac = MacLook(look, art: art)
            let back = try JSONDecoder.snake.decode(MacLook.self, from: JSONEncoder.snake.encode(mac))
            var expected = look
            expected.art = art.piece.spec
            #expect(back.look == expected)
            #expect(back.art == art)
        }
    }

    // MARK: Colours

    @Test func trekKeepsTheAppsOwnColours() {
        let c = LookColors(.trek)
        #expect(c.background.light == 0xFFFFFF && c.background.dark == 0x0A0A0B)
        #expect(c.foreground.light == 0x1A1A1C && c.foreground.dark == 0xE4E4E7)
        #expect(c.ember.light == 0xE85D1F && c.ember.dark == 0xFF7A3D)
        #expect(c.link.light == 0xC2410C)
        #expect(c.grouped.light == 0xF6F5F3 && c.grouped.dark == 0x0A0A0B)
    }

    @Test func aThemesNineColoursMapToTheApps() {
        let c = LookColors(.console)
        #expect(c.background.dark == 0x0B0D0C && c.background.light == 0xF6F6F1)
        #expect(c.grouped.light == 0xEEEEE7, "grouped lists stand on the sidebar in light")
        #expect(c.grouped.dark == 0x0B0D0C, "and on the window in dark")
        #expect(c.surface.dark == 0x151A18 && c.elevated.dark == 0x121615 && c.border.dark == 0x222A26)
        #expect(c.foreground.dark == 0xD4DDD7 && c.muted.dark == 0x7C8A82)
        #expect(c.ember.dark == 0x6BDB9A && c.link.dark == 0x6BDB9A && c.onEmber.dark == 0x06140C)
    }

    @Test func anAccentWithoutTextOnItGetsTextThatReads() {
        var look = Look(id: "mine", name: "Mine")
        look.dark = Look.Palette(accent: 0xFFE066FF)
        look.light = Look.Palette(accent: 0x1F4E99FF)
        let c = LookColors(look)
        #expect(c.onEmber.dark == 0x111111)
        #expect(c.onEmber.light == 0xFFFFFF)
        // What it leaves out stays Trek's.
        #expect(c.background.dark == 0x0A0A0B)
    }

    @Test func motionPacesAsOnTheMac() {
        #expect(Look.console.motion.scale == 0.75)
        #expect(Look.journal.motion.scale == 1.35 && Look.alpenglow.motion.scale == 1.35)
        #expect(Look.trek.motion.scale == 1)
        #expect(Look.Motion.still.scale == 0)
    }

    @Test func sizesAndCornersScale() {
        #expect(Look.trek.uiScale == 1 && Look.trek.conversationScale == 1)
        #expect(abs(Look.journal.conversationScale - 16 / 14.5) < 1e-9)
        #expect(abs(Look.console.uiScale - 13 / 14) < 1e-9)
        #expect(Look.Face.serif.family == "Spectral")
        #expect(Look.Face.mono.postScript(.semibold) == "JetBrainsMonoNL-SemiBold")
    }

    @Test func theFontsAThemeNeedsAreBundled() {
        for face in [Look.Face.sans, .mono, .serif] {
            for weight in [Font.Weight.regular, .medium, .semibold, .bold] {
                #expect(UIFont(name: face.postScript(weight), size: 12) != nil, "\(face.postScript(weight))")
            }
        }
    }

    // MARK: Background art

    @Test func everyPieceIsInTheCatalogAndNamedAsOnTheMac() throws {
        #expect(ArtPiece.all.count == 20)
        #expect(Set(ArtPiece.all.map(\.spec)).count == 20)
        for p in ArtPiece.all {
            #expect(UIImage(named: p.asset) != nil, "\(p.asset)")
            #expect(UIImage(named: p.thumbnail) != nil, "\(p.thumbnail)")
        }
        #expect(ArtPiece.named("builtin:dawn")?.collection == .classic)
        #expect(ArtPiece.named("builtin:paintings/oxbow")?.name == "Oxbow")
        #expect(ArtPiece.named("builtin:oxbow")?.collection == .paintings, "a bare name, as the Mac reads it")
        #expect(ArtPiece.named("/Users/me/hills.png") == nil)
        // The Mac's list, in its order (crates/trek-app/src/backgrounds.rs).
        let rust = try String(contentsOf: Self.root.appending(path: "crates/trek-app/src/backgrounds.rs"), encoding: .utf8)
        var ids: [String] = []
        var afterPainting = false
        for line in rust.split(separator: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("piece(\"") || afterPainting, let id = t.components(separatedBy: "\"").dropFirst().first {
                ids.append(String(id))
            }
            afterPainting = t == "painting("
        }
        #expect(ids == ArtPiece.all.map(\.key))
    }

    @Test func artShowsWhereItsPlacementPutsIt() throws {
        let oxbow = try #require(ArtPiece.named("builtin:paintings/oxbow"))
        let braille = try #require(ArtPiece.named("builtin:ascii/braille"))
        let dawn = try #require(ArtPiece.named("builtin:dawn"))
        let chat = ArtChoice(piece: oxbow, placement: .chat, dim: 0)
        #expect(chat.shows(on: .threadList) == .full && chat.shows(on: .thread) == .full)
        // ASCII is the sidebar's (the thread list), wherever it's put.
        let ascii = ArtChoice(piece: braille, placement: .chat, dim: 0)
        #expect(ascii.shows(on: .threadList) == .full)
        #expect(ascii.shows(on: .thread) == nil && ascii.shows(on: .pane) == nil && ascii.shows(on: .newThread) == nil)
        let header = ArtChoice(piece: dawn, placement: .newThread, dim: 0)
        #expect(header.shows(on: .threadList) == .header && header.shows(on: .newThread) == .header)
        #expect(header.shows(on: .thread) == nil)
    }

    @Test func thePhonesOwnChoiceAndItsLegacySwitch() throws {
        let (look, art) = LookStore.resolve(theme: "journal", art: "builtin:dawn", mac: nil, placement: "chat", dim: 0.2)
        #expect(look == .journal)
        #expect(art == ArtChoice(piece: try #require(ArtPiece.named("builtin:dawn")), placement: .chat, dim: 0.2))
        // A collection says where its art goes.
        let (_, ascii) = LookStore.resolve(theme: "trek", art: "builtin:ascii/led", mac: nil, placement: "new_thread", dim: nil)
        #expect(ascii?.placement == .sidebar)
        #expect(LookStore.resolve(theme: "nope", art: "none", mac: nil, placement: nil, dim: nil).0 == .trek)
        // The earlier Background art switch, on, is the ridge it showed.
        let defaults = try #require(UserDefaults(suiteName: "LookTests-\(UUID())"))
        #expect(LookStore.artSetting(defaults) == AppearanceKey.matchMac)
        defaults.set(true, forKey: AppearanceKey.backgroundArt)
        #expect(LookStore.artSetting(defaults) == "builtin:night")
        defaults.set("none", forKey: AppearanceKey.background)
        #expect(LookStore.artSetting(defaults) == "none")
    }

    @Test func matchYourMacTakesItsThemeAndArt() throws {
        let mac = try JSONDecoder.snake.decode(MacLook.self, from: Data(##"{"theme":{"id":"mine","name":"Mine","dark":{"accent":"#123456"}},"background":"builtin:light/aurora","placement":"chat","dim":0.1}"##.utf8))
        let (look, art) = LookStore.resolve(theme: AppearanceKey.matchMac, art: AppearanceKey.matchMac, mac: mac, placement: nil, dim: nil)
        #expect(look.id == "mine" && look.dark.accent == 0x123456FF)
        #expect(art?.piece.key == "aurora" && art?.dim == 0.1)
        // The phone's own art with the Mac's theme, and the other way about.
        #expect(LookStore.resolve(theme: AppearanceKey.matchMac, art: "none", mac: mac, placement: nil, dim: nil).1 == nil)
        #expect(LookStore.resolve(theme: "console", art: AppearanceKey.matchMac, mac: mac, placement: nil, dim: nil).0 == .console)
    }

    @Test func aStoreKeepsTheMacsLastLook() throws {
        let defaults = try #require(UserDefaults(suiteName: "LookTests-\(UUID())"))
        let store = LookStore(defaults: defaults)
        #expect(store.look == .trek)
        store.macSent(MacLook(.alpenglow, art: nil))
        #expect(store.look.id == "alpenglow")
        store.macSent(nil)
        #expect(store.look.id == "alpenglow", "a link with no look yet keeps the last")
        #expect(LookStore(defaults: defaults).look.id == "alpenglow", "and the app opens in it")
        defaults.set("journal", forKey: AppearanceKey.trekTheme)
        store.refresh()
        #expect(store.look == .journal)
    }

    // MARK: The scrim

    @Test func theScrimIsTheLeastThatKeepsTheRatio() {
        let ground: UInt32 = 0x0A0A0B, text: UInt32 = 0xE4E4E7
        let a = Scrim.alpha(art: 0xFFFFFF, ground: ground, text: text, ratio: 4.5)
        #expect(Scrim.contrast(Scrim.rgb(text), Scrim.mix(Scrim.rgb(0xFFFFFF), Scrim.rgb(ground), a)) >= 4.5)
        #expect(Scrim.contrast(Scrim.rgb(text), Scrim.mix(Scrim.rgb(0xFFFFFF), Scrim.rgb(ground), a - 0.03)) < 4.5)
        #expect(Scrim.alpha(art: ground, ground: ground, text: text, ratio: 4.5) == 0, "plain ground needs none")
        #expect(abs(Scrim.withDim(0.4, 0.5) - 0.7) < 1e-9)
        #expect(abs(Scrim.withDim(0, 0.2) - 0.2) < 1e-9)
    }

    /// In every built-in theme, light and dark, text keeps 4.5:1 and grey text 3:1 over each
    /// collection's most demanding pixel (backgrounds.py holds every pixel of the phone's files to
    /// that pixel). Classic's extremes are pure white and black, which the scrim's most (0.92)
    /// covers for text but not always for grey text: the Mac lets it be, and so does the phone.
    @Test(arguments: Look.builtIn)
    func textOverEveryCollectionsExtremeKeepsItsRatio(look: Look) throws {
        let c = LookColors(look)
        for collection in ArtCollection.allCases where collection != .classic {
            for dark in [true, false] {
                let piece = try #require(ArtPiece.all.first { $0.collection == collection })
                let choice = ArtChoice(piece: piece, placement: .chat, dim: 0)
                let ground = c.background.rgb(dark: dark)
                let alpha = Scrim.over(choice, dark: dark, ground: ground, text: c.foreground.rgb(dark: dark), muted: c.muted.rgb(dark: dark))
                let under = Scrim.mix(Scrim.rgb(collection.extreme(dark: dark)), Scrim.rgb(ground), alpha)
                #expect(Scrim.contrast(Scrim.rgb(c.foreground.rgb(dark: dark)), under) >= 4.5 - 0.01, "\(look.id) \(collection) dark=\(dark)")
                #expect(Scrim.contrast(Scrim.rgb(c.muted.rgb(dark: dark)), under) >= 3 - 0.01, "\(look.id) \(collection) dark=\(dark)")
            }
        }
    }
}

/// The Mac's interruption settings, which the phone shows and changes when the Mac sends them.
struct MacInterruptionSettingsTests {
    private static func settings(_ extra: String) throws -> MacSettings {
        let json = """
        {"type":"settings","re":"9","default_agent":"claude-code","default_effort":"high","default_access":"auto-accept-edits","follow_up":"steer",
         "notifications":"banner","push":{"enabled":false,"when":"away","server":"https://ntfy.sh","topic":""},
         "auto_settle_days":3,"theme":"system","full_access":false,"session_approvals":false\(extra)}
        """
        guard case .settings(_, let s) = try ServerMessage.decode(Data(json.utf8)) else { throw CocoaError(.coderReadCorrupt) }
        return s
    }

    @Test func aNewMacSendsThemAndAnOlderOneDoesnt() throws {
        let new = try Self.settings(#","resume_interrupted":true,"continue_on_other_account":false"#)
        #expect(new.resumeInterrupted == true && new.continueOnOtherAccount == false)
        let old = try Self.settings("")
        #expect(old.resumeInterrupted == nil && old.continueOnOtherAccount == nil)
    }

    @Test func aChangeSaysOnlyWhatChanged() throws {
        let change = SettingsChange(resumeInterrupted: true)
        #expect(change.json["resume_interrupted"] as? Bool == true)
        #expect(change.json["continue_on_other_account"] == nil)
        let shown = change.applied(to: try Self.settings(#","resume_interrupted":false,"continue_on_other_account":false"#))
        #expect(shown.resumeInterrupted == true && shown.continueOnOtherAccount == false)
    }
}
