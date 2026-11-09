import UIKit
import XCTest

/// What's on the screen as accessibility has it, flattened into a list an agent can read and
/// tap from (`GET /elements`): each control or text with its type, label, identifier, value and
/// frame in screen points, in reading order, nested by `depth`. Read from the app in front's
/// snapshot (and SpringBoard's, for the Home Screen and for alerts over an app). Bounded: at
/// most so many elements, and it stops walking when its time is up.
enum Elements {
    /// One node of a snapshot, as much of it as the list needs (a real snapshot, or a made-up
    /// one in the tests).
    protocol Node {
        var elementType: XCUIElement.ElementType { get }
        var label: String { get }
        var identifier: String { get }
        /// Its value as text, when it has one.
        var text: String? { get }
        var frame: CGRect { get }
        var isEnabled: Bool { get }
        var isSelected: Bool { get }
        var isFocused: Bool { get }
        var kids: [Node] { get }
    }

    struct Element: Equatable {
        var type: String
        var label: String
        var identifier: String
        /// Never a secure field's: `secure` says there is one instead.
        var value: String?
        var secure = false
        var frame: CGRect
        var enabled = true
        var selected = false
        var focused = false
        /// How many listed elements it sits inside.
        var depth = 0

        /// What makes two elements the same one on the screen: all it says, and where.
        var same: String {
            "\(type)\u{1}\(label)\u{1}\(identifier)\u{1}\(value ?? "")\u{1}\(frame)"
        }

        var json: [String: Any] {
            let r = { (v: CGFloat) -> Double in (Double(v) * 10).rounded() / 10 }
            var out: [String: Any] = [
                "type": type,
                "label": label,
                "identifier": identifier,
                "frame": [r(frame.minX), r(frame.minY), r(frame.width), r(frame.height)],
                "enabled": enabled,
                "selected": selected,
                "focused": focused,
                "depth": depth,
            ]
            if secure {
                out["secure"] = true
            } else if let value {
                out["value"] = value
            }
            return out
        }
    }

    /// How much one answer may hold and how long it may take.
    struct Limits: Equatable {
        var count = 300
        var seconds = 5.0

        init() {}

        /// From the query: `max=<elements>` and `ms=<milliseconds>`, each within bounds.
        init(query: String) {
            for pair in query.split(separator: "&") {
                let kv = pair.split(separator: "=", maxSplits: 1)
                guard kv.count == 2, let n = Int(kv[1]) else { continue }
                switch kv[0] {
                case "max": count = min(max(n, 1), 1000)
                case "ms": seconds = Double(min(max(n, 200), 20_000)) / 1000
                default: break
                }
            }
        }
    }

    /// The list being made.
    struct Flat {
        var elements: [Element] = []
        /// Elements that were on screen but past the count.
        var more = 0
        /// The time ran out (or the tree was too large) before all of it was walked.
        var partial = false
        var visited = 0
        /// What's listed already (`Element.same`), to leave repeats out.
        var seen: Set<String> = []
        let limit: Int
        let deadline: Date

        init(limit: Int, deadline: Date) {
            self.limit = limit
            self.deadline = deadline
        }
    }

    /// Run `body` (a snapshot) with XCTest giving an app's accessibility at most `seconds` to
    /// answer, where its own limit is longer: a screen with thousands of elements, or an app
    /// that hangs, then fails the snapshot instead of holding the driver. The limit is XCTest's
    /// `XCTTimeoutControls.defaultQueryExecutionTimeout` (not public API: left alone when it
    /// isn't there as expected) and goes back to what it was.
    static func bounded<T>(seconds: TimeInterval, _ body: () -> T) -> T {
        let get = NSSelectorFromString("defaultQueryExecutionTimeout")
        let set = NSSelectorFromString("setDefaultQueryExecutionTimeout:")
        guard let cls: AnyClass = NSClassFromString("XCTTimeoutControls"),
              let getter = class_getClassMethod(cls, get), Capture.shape(getter) == "d@:",
              let setter = class_getClassMethod(cls, set), Capture.shape(setter) == "v@:d"
        else { return body() }
        typealias Get = @convention(c) (AnyClass, Selector) -> Double
        typealias Set = @convention(c) (AnyClass, Selector, Double) -> Void
        let read = unsafeBitCast(method_getImplementation(getter), to: Get.self)
        let write = unsafeBitCast(method_getImplementation(setter), to: Set.self)
        // 60 s as Xcode 27 ships it.
        let before = read(cls, get)
        guard before > seconds, seconds > 0 else { return body() }
        write(cls, set, seconds)
        defer { write(cls, set, before) }
        return body()
    }

    /// The most nodes walked for one answer, whatever the time left.
    static let maxVisits = 20_000
    /// Labels and values are cut to this many characters.
    static let maxText = 240

    /// Add `node` and what's inside it to `flat`, in order: the ones on `screen` that say
    /// something (a label or a value, or an identifier) or are a control. Left out, with their
    /// children taking their place: a container that says nothing, and one with only an
    /// identifier and nothing listed inside it (`label-view` under a Home Screen icon). Left
    /// out whole: the status bar; a keyboard is listed without its keys (text goes through
    /// `/type`); and an element that repeats one already listed (XCTest gives each scroll
    /// indicator twice).
    static func flatten(_ node: Node, screen: CGRect, depth: Int = 0, into flat: inout Flat) {
        guard !flat.partial else { return }
        flat.visited += 1
        if flat.visited > maxVisits || (flat.visited % 32 == 0 && Date() > flat.deadline) {
            flat.partial = true
            return
        }
        let type = node.elementType
        if type == .statusBar { return }
        let frame = node.frame
        let secure = type == .secureTextField
        let (label, identifier) = (clip(node.label), clip(node.identifier))
        let value = secure ? nil : node.text.map(clip)
        let speaks = !label.isEmpty || !(value ?? "").isEmpty
        let silent = silentTypes.contains(type)
        let onScreen = frame.width > 0 && frame.height > 0 && frame.intersects(screen)
        // A picture with only an identifier is a symbol's name: decoration.
        var listed = onScreen && (speaks || !silent || (!identifier.isEmpty && type != .image))
        let element = Element(
            type: name(type), label: label, identifier: identifier, value: value, secure: secure,
            frame: frame, enabled: node.isEnabled, selected: node.isSelected, focused: node.isFocused,
            depth: depth)
        if listed, !flat.seen.insert(element.same).inserted { listed = false }
        let (at, before) = (flat.elements.count, flat.elements.count + flat.more)
        let room = flat.elements.count < flat.limit
        if listed, room { flat.elements.append(element) }
        if listed, type == .keyboard {
            if !room { flat.more += 1 }
            return
        }
        for kid in node.kids {
            flatten(kid, screen: screen, depth: listed ? depth + 1 : depth, into: &flat)
            if flat.partial { break }
        }
        guard listed else { return }
        let inside = flat.elements.count + flat.more - before - (room ? 1 : 0)
        if silent, !speaks, inside == 0 {
            // Only an identifier, and nothing in it: not worth a line.
            if room { flat.elements.remove(at: at) }
        } else if !room {
            flat.more += 1
        }
    }

    /// Types listed only when they say something: containers, and pictures.
    static let silentTypes: Set<XCUIElement.ElementType> = [
        .any, .other, .application, .window, .group, .layoutArea, .layoutItem, .splitGroup, .matte, .grid,
        .scrollView, .scrollBar, .handle, .cell, .image,
    ]

    static func clip(_ text: String) -> String {
        text.count > maxText ? String(text.prefix(maxText)) + "…" : text
    }

    /// A real snapshot's node.
    struct Live: Node {
        let snapshot: XCUIElementSnapshot

        var elementType: XCUIElement.ElementType { snapshot.elementType }
        var label: String { snapshot.label }
        var identifier: String { snapshot.identifier }
        var text: String? {
            switch snapshot.value {
            case let s as String: return s
            case let n as NSNumber: return n.stringValue
            default: return nil
            }
        }
        var frame: CGRect { snapshot.frame }
        var isEnabled: Bool { snapshot.isEnabled }
        var isSelected: Bool { snapshot.isSelected }
        /// The keyboard's focus, which XCTest's snapshot has under a name that isn't public.
        var isFocused: Bool {
            guard let object = snapshot as? NSObject else { return false }
            for key in ["hasKeyboardFocus", "hasFocus"] where object.responds(to: NSSelectorFromString(key)) {
                if (object.value(forKey: key) as? NSNumber)?.boolValue == true { return true }
            }
            return false
        }
        var kids: [Node] { snapshot.children.map { Live(snapshot: $0) } }
    }

    /// An element type's name, as XCTest spells it without its prefix (`Button`, `StaticText`).
    static func name(_ type: XCUIElement.ElementType) -> String {
        switch type {
        case .any: return "Any"
        case .other: return "Other"
        case .application: return "Application"
        case .group: return "Group"
        case .window: return "Window"
        case .sheet: return "Sheet"
        case .drawer: return "Drawer"
        case .alert: return "Alert"
        case .dialog: return "Dialog"
        case .button: return "Button"
        case .radioButton: return "RadioButton"
        case .radioGroup: return "RadioGroup"
        case .checkBox: return "CheckBox"
        case .disclosureTriangle: return "DisclosureTriangle"
        case .popUpButton: return "PopUpButton"
        case .comboBox: return "ComboBox"
        case .menuButton: return "MenuButton"
        case .toolbarButton: return "ToolbarButton"
        case .popover: return "Popover"
        case .keyboard: return "Keyboard"
        case .key: return "Key"
        case .navigationBar: return "NavigationBar"
        case .tabBar: return "TabBar"
        case .tabGroup: return "TabGroup"
        case .toolbar: return "Toolbar"
        case .statusBar: return "StatusBar"
        case .table: return "Table"
        case .tableRow: return "TableRow"
        case .tableColumn: return "TableColumn"
        case .outline: return "Outline"
        case .outlineRow: return "OutlineRow"
        case .browser: return "Browser"
        case .collectionView: return "CollectionView"
        case .slider: return "Slider"
        case .pageIndicator: return "PageIndicator"
        case .progressIndicator: return "ProgressIndicator"
        case .activityIndicator: return "ActivityIndicator"
        case .segmentedControl: return "SegmentedControl"
        case .picker: return "Picker"
        case .pickerWheel: return "PickerWheel"
        case .switch: return "Switch"
        case .toggle: return "Toggle"
        case .link: return "Link"
        case .image: return "Image"
        case .icon: return "Icon"
        case .searchField: return "SearchField"
        case .scrollView: return "ScrollView"
        case .scrollBar: return "ScrollBar"
        case .staticText: return "StaticText"
        case .textField: return "TextField"
        case .secureTextField: return "SecureTextField"
        case .datePicker: return "DatePicker"
        case .textView: return "TextView"
        case .menu: return "Menu"
        case .menuItem: return "MenuItem"
        case .menuBar: return "MenuBar"
        case .menuBarItem: return "MenuBarItem"
        case .map: return "Map"
        case .webView: return "WebView"
        case .incrementArrow: return "IncrementArrow"
        case .decrementArrow: return "DecrementArrow"
        case .timeline: return "Timeline"
        case .ratingIndicator: return "RatingIndicator"
        case .valueIndicator: return "ValueIndicator"
        case .splitGroup: return "SplitGroup"
        case .splitter: return "Splitter"
        case .relevanceIndicator: return "RelevanceIndicator"
        case .colorWell: return "ColorWell"
        case .helpTag: return "HelpTag"
        case .matte: return "Matte"
        case .dockItem: return "DockItem"
        case .ruler: return "Ruler"
        case .rulerMarker: return "RulerMarker"
        case .grid: return "Grid"
        case .levelIndicator: return "LevelIndicator"
        case .cell: return "Cell"
        case .layoutArea: return "LayoutArea"
        case .layoutItem: return "LayoutItem"
        case .handle: return "Handle"
        case .stepper: return "Stepper"
        case .tab: return "Tab"
        case .touchBar: return "TouchBar"
        case .statusItem: return "StatusItem"
        @unknown default: return "Other"
        }
    }
}
