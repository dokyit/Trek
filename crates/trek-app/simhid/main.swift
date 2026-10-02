// trek-simhid: one long-lived connection to a booted iOS Simulator, built on FBSimulatorControl
// (shipped with AXe). Streams the screen as JPEG frames when it changes and applies touch, button,
// key and text input with no per-event process start-up.
//
// stdin, one command per line:
//   down X Y | move X Y | up X Y      touch in device points
//   tap X Y                           down + up
//   button home|lock|side|siri|applepay
//   key USAGE [shift]                 USB HID usage code
//   text <utf-8 text>
//   fps N                             frame cap (0 pauses the stream)
// stdout, framed: [kind: u8][len: u32 BE][payload]
//   kind 1 = frame: [screen width px: u16 BE][screen height px: u16 BE][jpeg, possibly scaled down]
//   kind 2 = message: utf-8 ("ready WxH@scale", "error …")

import AppKit
import CoreImage
import FBControlCore
import FBSimulatorControl
import Foundation
import IOSurface
import ImageIO
import UniformTypeIdentifiers

let out = FileHandle.standardOutput
let outLock = NSLock()

func emit(_ kind: UInt8, _ payload: Data) {
    var header = Data([kind])
    var len = UInt32(payload.count).bigEndian
    header.append(Data(bytes: &len, count: 4))
    outLock.lock()
    out.write(header + payload)
    outLock.unlock()
}

func message(_ s: String) { emit(2, Data(s.utf8)) }

guard CommandLine.arguments.count > 1 else {
    FileHandle.standardError.write(Data("usage: trek-simhid <udid> [max-width-px]\n".utf8))
    exit(2)
}
let udid = CommandLine.arguments[1]
let maxWidth = CommandLine.arguments.count > 2 ? Double(CommandLine.arguments[2]) ?? 900 : 900

let logger = FBControlCoreLoggerFactory.systemLoggerWriting(toStderr: false, withDebugLogging: false)
FBSimulatorControlFrameworkLoader.essentialFrameworks.loadPrivateFrameworksOrAbort()
FBSimulatorControlFrameworkLoader.xcodeFrameworks.loadPrivateFrameworksOrAbort()

let simulator: FBSimulator
do {
    let config = FBSimulatorControlConfiguration(deviceSetPath: nil, logger: logger, reporter: nil)
    let control = try FBSimulatorControl.withConfiguration(config)
    guard let sim = control.set.simulator(withUDID: udid) else {
        message("error no simulator with UDID \(udid)")
        exit(1)
    }
    simulator = sim
} catch {
    message("error \(error.localizedDescription)")
    exit(1)
}

// ---------- screen ----------

final class Streamer: NSObject, FBFramebufferConsumer {
    let queue = DispatchQueue(label: "trek.simhid.frames")
    let ci = CIContext(options: [.useSoftwareRenderer: false])
    var surface: IOSurface?
    var dirty = true
    var minInterval: Double = 1.0 / 30
    var lastSent: Double = 0
    var pending = false

    func didChange(_ surface: IOSurface?) {
        self.surface = surface
        dirty = true
        schedule()
    }

    func didReceiveDamageRect() {
        dirty = true
        schedule()
    }

    func schedule() {
        guard minInterval > 0, !pending else { return }
        let now = CACurrentMediaTime()
        let wait = max(0, lastSent + minInterval - now)
        pending = true
        queue.asyncAfter(deadline: .now() + wait) { [weak self] in
            guard let self else { return }
            self.pending = false
            self.send()
        }
    }

    func send() {
        guard dirty, let surface else { return }
        dirty = false
        lastSent = CACurrentMediaTime()
        let image = CIImage(ioSurface: surface)
        let w = image.extent.width
        let scale = min(1, maxWidth / max(w, 1))
        let scaled = scale < 1 ? image.transformed(by: CGAffineTransform(scaleX: scale, y: scale)) : image
        guard let cg = ci.createCGImage(scaled, from: scaled.extent) else { return }
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, UTType.jpeg.identifier as CFString, 1, nil) else { return }
        CGImageDestinationAddImage(dest, cg, [kCGImageDestinationLossyCompressionQuality: 0.82] as CFDictionary)
        guard CGImageDestinationFinalize(dest) else { return }
        var payload = Data()
        // Report the device's real pixel size; the image itself may be scaled down.
        var pw = UInt16(image.extent.width).bigEndian
        var ph = UInt16(image.extent.height).bigEndian
        payload.append(Data(bytes: &pw, count: 2))
        payload.append(Data(bytes: &ph, count: 2))
        payload.append(data as Data)
        emit(1, payload)
    }
}

let streamer = Streamer()
var framebuffer: FBFramebuffer?
do {
    let fb = try FBFramebuffer.mainScreenSurface(for: simulator, logger: logger)
    framebuffer = fb
    if let s = fb.attach(streamer, on: streamer.queue) {
        streamer.queue.async { streamer.didChange(s) }
    }
} catch {
    message("error framebuffer: \(error.localizedDescription)")
}

// ---------- input ----------

// US keyboard: character → (HID usage, shift)
func keyFor(_ ch: Character) -> (UInt32, Bool)? {
    let lower = "abcdefghijklmnopqrstuvwxyz"
    if let i = lower.firstIndex(of: ch) { return (4 + UInt32(lower.distance(from: lower.startIndex, to: i)), false) }
    if let l = Character(ch.lowercased()) as Character?, ch.isUppercase, let i = lower.firstIndex(of: l) {
        return (4 + UInt32(lower.distance(from: lower.startIndex, to: i)), true)
    }
    let table: [Character: (UInt32, Bool)] = [
        "1": (30, false), "2": (31, false), "3": (32, false), "4": (33, false), "5": (34, false),
        "6": (35, false), "7": (36, false), "8": (37, false), "9": (38, false), "0": (39, false),
        "!": (30, true), "@": (31, true), "#": (32, true), "$": (33, true), "%": (34, true),
        "^": (35, true), "&": (36, true), "*": (37, true), "(": (38, true), ")": (39, true),
        "\n": (40, false), "\t": (43, false), " ": (44, false),
        "-": (45, false), "_": (45, true), "=": (46, false), "+": (46, true),
        "[": (47, false), "{": (47, true), "]": (48, false), "}": (48, true),
        "\\": (49, false), "|": (49, true), ";": (51, false), ":": (51, true),
        "'": (52, false), "\"": (52, true), "`": (53, false), "~": (53, true),
        ",": (54, false), "<": (54, true), ".": (55, false), ">": (55, true),
        "/": (56, false), "?": (56, true),
    ]
    return table[ch]
}

func keyEvents(_ usage: UInt32, shift: Bool) -> [FBSimulatorHIDEvent] {
    var e: [FBSimulatorHIDEvent] = []
    if shift { e.append(.keyboard(direction: .down, keyCode: 225)) }
    e.append(.keyboard(direction: .down, keyCode: usage))
    e.append(.keyboard(direction: .up, keyCode: usage))
    if shift { e.append(.keyboard(direction: .up, keyCode: 225)) }
    return e
}

func parse(_ line: String) -> FBSimulatorHIDEvent? {
    let parts = line.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
    guard let cmd = parts.first else { return nil }
    let rest = parts.count > 1 ? String(parts[1]) : ""
    let nums = rest.split(separator: " ").compactMap { Double($0) }
    switch cmd {
    case "down" where nums.count >= 2, "move" where nums.count >= 2:
        return .touch(direction: .down, x: nums[0], y: nums[1])
    case "up" where nums.count >= 2:
        return .touch(direction: .up, x: nums[0], y: nums[1])
    case "tap" where nums.count >= 2:
        return .tapAt(x: nums[0], y: nums[1])
    case "button":
        let map: [String: FBSimulatorHIDButton] = ["home": .homeButton, "lock": .lock, "side": .sideButton, "siri": .siri, "applepay": .applePay]
        return map[rest.trimmingCharacters(in: .whitespaces)].map { .shortButtonPress($0) }
    case "key":
        let p = rest.split(separator: " ")
        guard let usage = p.first.flatMap({ UInt32($0) }) else { return nil }
        return .composite(keyEvents(usage, shift: p.contains("shift")))
    case "text":
        return .composite(rest.flatMap { ch in keyFor(ch).map { keyEvents($0.0, shift: $0.1) } ?? [] })
    default:
        return nil
    }
}

var lastDown: Date?

Task {
    do {
        let hid = try await simulator.connectToHID()
        message("ready")
        while let line = readLine(strippingNewline: true) {
            if line.hasPrefix("fps ") {
                let n = Double(line.dropFirst(4)) ?? 30
                streamer.queue.async {
                    streamer.minInterval = n > 0 ? 1.0 / n : 0
                    streamer.dirty = true
                    streamer.schedule()
                }
                continue
            }
            // Buttons and alerts ignore taps shorter than a real finger press.
            if line.hasPrefix("down ") {
                lastDown = Date()
            } else if line.hasPrefix("up "), let d = lastDown {
                let held = Date().timeIntervalSince(d)
                if held < 0.07 { try? await Task.sleep(nanoseconds: UInt64((0.07 - held) * 1_000_000_000)) }
                lastDown = nil
            }
            guard let event = parse(line) else {
                message("error unknown command: \(line)")
                continue
            }
            do {
                try await hid.send(event: event, logger: logger)
            } catch {
                message("error \(error.localizedDescription)")
            }
        }
        hid.disconnect()
        exit(0)
    } catch {
        message("error HID: \(error.localizedDescription)")
        exit(1)
    }
}

RunLoop.main.run()
