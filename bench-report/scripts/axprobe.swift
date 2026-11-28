// axprobe <pid>: walk an app's accessibility tree (what VoiceOver reads) and count its controls
// and how many carry a name. Electron/Chromium apps are asked to build their tree first
// (AXManualAccessibility), as an assistive app would. Prints one JSON line.
import Foundation
import ApplicationServices

let pid = pid_t(CommandLine.arguments[1])!
let app = AXUIElementCreateApplication(pid)
AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)
AXUIElementSetAttributeValue(app, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
Thread.sleep(forTimeInterval: Double(ProcessInfo.processInfo.environment["AX_WAIT"] ?? "2") ?? 2)
func attr(_ e: AXUIElement, _ a: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, a as CFString, &v) == .success ? v : nil
}
let interactive: Set<String> = ["AXButton", "AXTextField", "AXTextArea", "AXCheckBox", "AXMenuButton", "AXPopUpButton",
    "AXLink", "AXRadioButton", "AXSlider", "AXComboBox", "AXDisclosureTriangle", "AXIncrementor", "AXSearchField", "AXTab", "AXRow", "AXCell"]
var nodes = 0, controls = 0, named = 0, texts = 0
var roles: [String: Int] = [:]
var unnamedSamples: [String] = []
func walk(_ e: AXUIElement, _ depth: Int) {
    if nodes > 30000 || depth > 60 { return }
    nodes += 1
    let role = attr(e, "AXRole") as? String ?? "?"
    roles[role, default: 0] += 1
    if role == "AXStaticText" { texts += 1 }
    if interactive.contains(role) {
        controls += 1
        let name = [attr(e, "AXTitle"), attr(e, "AXDescription"), attr(e, "AXLabel"), attr(e, "AXHelp"), attr(e, "AXPlaceholderValue")]
            .compactMap { $0 as? String }.first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        var hasName = name != nil
        if !hasName, let t = attr(e, "AXTitleUIElement") { hasName = (t as! AXUIElement) != e }
        if !hasName, role == "AXRow" || role == "AXCell" || role == "AXTextField" || role == "AXTextArea" {
            hasName = !((attr(e, "AXValue") as? String) ?? "").isEmpty
            if !hasName, let kids = attr(e, "AXChildren") as? [AXUIElement] { hasName = !kids.isEmpty }
        }
        if hasName { named += 1 } else if unnamedSamples.count < 8 { unnamedSamples.append(role) }
    }
    if let kids = attr(e, "AXChildren") as? [AXUIElement] { for k in kids { walk(k, depth + 1) } }
}
let trusted = AXIsProcessTrusted()
let wins = (attr(app, "AXWindows") as? [AXUIElement]) ?? []
// Two walks: Chromium builds its tree lazily as it is first read; the second walk is the one kept.
for w in wins { walk(w, 0) }
Thread.sleep(forTimeInterval: 2)
nodes = 0; controls = 0; named = 0; texts = 0; roles = [:]; unnamedSamples = []
for w in wins { walk(w, 0) }
let top = roles.sorted { $0.value > $1.value }.prefix(8).map { "\($0.key):\($0.value)" }
let out: [String: Any] = ["trusted": trusted, "windows": wins.count, "nodes": nodes, "controls": controls, "named_controls": named,
    "static_texts": texts, "top_roles": top, "unnamed_examples": unnamedSamples]
print(String(data: try! JSONSerialization.data(withJSONObject: out), encoding: .utf8)!)
