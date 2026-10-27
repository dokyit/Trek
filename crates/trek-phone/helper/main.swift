// trek-phone: a real iPhone's screen over USB and Trek's Bluetooth keyboard and mouse for it.
//
// Screen: a trusted iPhone on the cable becomes a capture device once a process sets
// kCMIOHardwarePropertyAllowScreenCaptureDevices (QuickTime's switch); frames come through
// AVFoundation. Input: the Mac publishes a Bluetooth LE HID service (HID over GATT) with
// CBPeripheralManager; the iPhone pairs with "Trek" like any keyboard, and with AssistiveTouch on
// it shows a pointer. Trek computes the reports (trek-phone's hid.rs); this only sends them. The
// GATT layout follows xhoantran/iphone-use (MIT).
//
//   trek-phone <report map hex> <frame max width>
// stdin, one command a line:
//   hid <id>:<hex> w<ms> ...     send reports to the paired phone, pausing as told
//   frames on [fps] | frames off stream JPEG frames of the screen
//   shot <max edge> <path>       write the newest frame there as PNG (the path may have spaces)
//   status                       report the state now
//   quit
// stdout, framed: <type u8><length u32 BE><payload>
//   1  a frame: width u16 BE, height u16 BE (the screen's), then JPEG
//   2  a message (UTF-8 JSON): {"event":"status",...} | {"event":"shot",...} | {"event":"error",...}

import AVFoundation
import AppKit
import CoreBluetooth
import CoreImage
import CoreMediaIO
import Foundation
import ImageIO
import UniformTypeIdentifiers

let outLock = NSLock()
func emit(_ type: UInt8, _ payload: Data) {
  outLock.lock()
  defer { outLock.unlock() }
  var header = Data([type])
  var len = UInt32(payload.count).bigEndian
  header.append(Data(bytes: &len, count: 4))
  FileHandle.standardOutput.write(header + payload)
}
func message(_ dict: [String: Any]) {
  if let data = try? JSONSerialization.data(withJSONObject: dict, options: [.sortedKeys]) {
    emit(2, data)
  }
}

func hexBytes(_ s: Substring) -> [UInt8]? {
  var out: [UInt8] = []
  var i = s.startIndex
  while i < s.endIndex {
    let j = s.index(i, offsetBy: 2, limitedBy: s.endIndex) ?? s.endIndex
    guard let b = UInt8(s[i..<j], radix: 16) else { return nil }
    out.append(b)
    i = j
  }
  return out
}

enum Args {
  static let all = CommandLine.arguments
  static let reportMap: [UInt8] = all.count >= 3 ? hexBytes(Substring(all[1])) ?? [] : []
  static let maxWidth: Int = all.count >= 3 ? Int(all[2]) ?? 0 : 0
}
if Args.reportMap.isEmpty || Args.maxWidth <= 0 {
  FileHandle.standardError.write("usage: trek-phone <report map hex> <frame max width>\n".data(using: .utf8)!)
  exit(2)
}

// MARK: - Bluetooth HID

final class HID: NSObject, CBPeripheralManagerDelegate {
  let name = "Trek"
  let queue = DispatchQueue(label: "trek-phone.bluetooth")
  var manager: CBPeripheralManager!
  var toAdd: [CBMutableService] = []
  var published = false
  var inputs: [UInt8: CBMutableCharacteristic] = [:]
  var keyboardOutput: CBMutableCharacteristic!
  var controlPoint: CBMutableCharacteristic!
  var serviceChanged: CBMutableCharacteristic!
  var centrals: [UUID: CBCentral] = [:]
  var subscriptions: [UUID: Set<ObjectIdentifier>] = [:]
  var serviceChangedSent = Set<UUID>()
  var outbox: [(CBMutableCharacteristic, Data, [CBCentral])] = []
  var state = "starting"
  var onChange: (() -> Void)?
  /// The state and how many phones listen, for readers off the Bluetooth queue: that queue can
  /// sit inside CoreBluetooth while macOS asks the user about Bluetooth, and a status report
  /// must not wait for the answer.
  let snapshotLock = NSLock()
  var snapshot: (state: String, hosts: Int) = ("starting", 0)

  func publish() {
    let count = hosts.count
    snapshotLock.lock()
    snapshot = (state, count)
    snapshotLock.unlock()
  }

  func current() -> (state: String, hosts: Int) {
    snapshotLock.lock()
    defer { snapshotLock.unlock() }
    return snapshot
  }

  func start() {
    queue.async { self.manager = CBPeripheralManager(delegate: self, queue: self.queue) }
  }

  var hosts: [CBCentral] {
    let ids = Set(inputs.values.map(ObjectIdentifier.init))
    return subscriptions.filter { !$0.value.isDisjoint(with: ids) }.compactMap { centrals[$0.key] }
  }

  /// Send one report to every phone listening (Trek drives one phone).
  func send(_ id: UInt8, _ bytes: [UInt8]) -> Bool {
    // Nobody listening (or Bluetooth not up): don't queue behind CoreBluetooth.
    if current().hosts == 0 { return false }
    return queue.sync {
      guard let c = inputs[id] else { return false }
      let targets = hosts
      if targets.isEmpty { return false }
      outbox.append((c, Data(bytes), targets))
      drain()
      return true
    }
  }

  static func sig(_ short: String) -> CBUUID {
    // The 16-bit form of a SIG service (1812) is refused as reserved; the full form isn't.
    CBUUID(string: "0000\(short)-0000-1000-8000-00805F9B34FB")
  }

  func services() -> [CBMutableService] {
    serviceChanged = CBMutableCharacteristic(
      type: CBUUID(string: "2A05"), properties: [.indicate], value: nil, permissions: [.readable])
    let gatt = CBMutableService(type: Self.sig("1801"), primary: true)
    gatt.characteristics = [serviceChanged]

    let info = CBMutableCharacteristic(
      type: CBUUID(string: "2A4A"), properties: [.read], value: Data([0x11, 0x01, 0x00, 0x02]),
      permissions: [.readable])
    let map = CBMutableCharacteristic(
      type: CBUUID(string: "2A4B"), properties: [.read], value: Data(Args.reportMap),
      permissions: [.readEncryptionRequired])
    controlPoint = CBMutableCharacteristic(
      type: CBUUID(string: "2A4C"), properties: [.writeWithoutResponse], value: nil,
      permissions: [.writeEncryptionRequired])
    let protocolMode = CBMutableCharacteristic(
      type: CBUUID(string: "2A4E"), properties: [.read, .writeWithoutResponse], value: nil,
      permissions: [.readable, .writeable])
    var chars: [CBMutableCharacteristic] = [info, map, controlPoint, protocolMode]
    for id: UInt8 in [1, 2, 3, 4] {
      let report = CBMutableCharacteristic(
        type: CBUUID(string: "2A4D"), properties: [.read, .notify], value: nil,
        permissions: [.readEncryptionRequired])
      report.descriptors = [CBMutableDescriptor(type: CBUUID(string: "2908"), value: Data([id, 1]))]
      inputs[id] = report
      chars.append(report)
    }
    keyboardOutput = CBMutableCharacteristic(
      type: CBUUID(string: "2A4D"), properties: [.read, .write, .writeWithoutResponse], value: nil,
      permissions: [.readEncryptionRequired, .writeEncryptionRequired])
    keyboardOutput.descriptors = [CBMutableDescriptor(type: CBUUID(string: "2908"), value: Data([1, 2]))]
    chars.append(keyboardOutput)
    let hid = CBMutableService(type: Self.sig("1812"), primary: true)
    hid.characteristics = chars

    let device = CBMutableService(type: Self.sig("180A"), primary: true)
    device.characteristics = [
      CBMutableCharacteristic(
        type: CBUUID(string: "2A29"), properties: [.read], value: Data("Trek".utf8), permissions: [.readable]),
      CBMutableCharacteristic(
        type: CBUUID(string: "2A50"), properties: [.read],
        value: Data([0x02, 0xFF, 0xFF, 0x00, 0x01, 0x00, 0x01]), permissions: [.readable]),
    ]
    let battery = CBMutableService(type: Self.sig("180F"), primary: true)
    battery.characteristics = [
      CBMutableCharacteristic(type: CBUUID(string: "2A19"), properties: [.read, .notify], value: nil, permissions: [.readable])
    ]
    return [gatt, hid, device, battery]
  }

  func addNext() {
    guard !toAdd.isEmpty else {
      published = true
      advertise()
      return
    }
    manager.add(toAdd.removeFirst())
  }

  func advertise() {
    guard published, !manager.isAdvertising else { return }
    manager.startAdvertising([
      CBAdvertisementDataLocalNameKey: name,
      CBAdvertisementDataServiceUUIDsKey: [CBUUID(string: "1812")],
    ])
  }

  func set(_ s: String) {
    state = s
    publish()
    onChange?()
  }

  func peripheralManagerDidUpdateState(_ p: CBPeripheralManager) {
    switch p.state {
    case .poweredOn:
      p.removeAllServices()
      inputs = [:]
      published = false
      toAdd = services()
      set("starting")
      addNext()
    case .unauthorized: set("not allowed")
    case .poweredOff: set("off")
    case .unsupported: set("unsupported")
    default: set("unavailable")
    }
  }

  func peripheralManager(_ p: CBPeripheralManager, didAdd service: CBService, error: Error?) {
    if let error {
      set("failed: \(error.localizedDescription)")
      return
    }
    addNext()
  }

  func peripheralManagerDidStartAdvertising(_ p: CBPeripheralManager, error: Error?) {
    if let error, (error as? CBError)?.code != .alreadyAdvertising {
      set("advertising failed: \(error.localizedDescription)")
    } else {
      set(hosts.isEmpty ? "advertising" : "connected")
    }
  }

  func peripheralManager(_ p: CBPeripheralManager, didReceiveRead request: CBATTRequest) {
    let value: Data
    switch request.characteristic.uuid {
    case CBUUID(string: "2A4E"): value = Data([0x01])
    case CBUUID(string: "2A19"): value = Data([100])
    case CBUUID(string: "2A4D"): value = Data(count: length(request.characteristic))
    default:
      p.respond(to: request, withResult: .attributeNotFound)
      return
    }
    guard request.offset <= value.count else {
      p.respond(to: request, withResult: .invalidOffset)
      return
    }
    request.value = value.subdata(in: request.offset..<value.count)
    p.respond(to: request, withResult: .success)
  }

  func peripheralManager(_ p: CBPeripheralManager, didReceiveWrite requests: [CBATTRequest]) {
    for r in requests {
      centrals[r.central.identifier] = r.central
      if r.characteristic.uuid == controlPoint.uuid { recoverStaleCache(r.central) }
    }
    if let first = requests.first { p.respond(to: first, withResult: .success) }
  }

  func peripheralManager(_ p: CBPeripheralManager, central: CBCentral, didSubscribeTo c: CBCharacteristic) {
    centrals[central.identifier] = central
    subscriptions[central.identifier, default: []].insert(ObjectIdentifier(c))
    if let (_, input) = inputs.first(where: { $0.value === c }) {
      outbox.append((input, Data(count: length(input)), [central]))
      drain()
      set("connected")
      advertise()
    }
  }

  func peripheralManager(_ p: CBPeripheralManager, central: CBCentral, didUnsubscribeFrom c: CBCharacteristic) {
    subscriptions[central.identifier]?.remove(ObjectIdentifier(c))
    set(hosts.isEmpty ? "advertising" : "connected")
  }

  func peripheralManagerIsReady(toUpdateSubscribers p: CBPeripheralManager) { drain() }

  func drain() {
    while let (c, data, targets) = outbox.first {
      guard manager.updateValue(data, for: c, onSubscribedCentrals: targets) else { return }
      outbox.removeFirst()
    }
  }

  /// A bonded phone that trusts a stale GATT cache writes the control point but never
  /// subscribes: Service Changed makes it look again.
  func recoverStaleCache(_ central: CBCentral) {
    let subscribed = subscriptions[central.identifier] ?? []
    let hasInput = !subscribed.isDisjoint(with: inputs.values.map(ObjectIdentifier.init))
    guard !hasInput, subscribed.contains(ObjectIdentifier(serviceChanged)),
      !serviceChangedSent.contains(central.identifier)
    else { return }
    serviceChangedSent.insert(central.identifier)
    outbox.insert((serviceChanged, Data([0x10, 0x00, 0xFF, 0xFF]), [central]), at: 0)
    drain()
  }

  func length(_ c: CBCharacteristic) -> Int {
    if c === keyboardOutput { return 1 }
    switch inputs.first(where: { $0.value === c })?.key {
    case 1: return 8
    case 2: return 5
    case 3: return 4
    case 4: return 2
    default: return 0
    }
  }
}

// MARK: - Screen

final class Screen: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
  let context = CIContext()
  let queue = DispatchQueue(label: "trek-phone.capture")
  var session: AVCaptureSession?
  var device: AVCaptureDevice?
  var latest: CVPixelBuffer?
  var frames = 0
  var lastFrame: Date?
  var streaming = false
  var fps = 15.0
  var lastSent = Date.distantPast
  var camera = "undetermined"
  var onChange: (() -> Void)?

  func start() {
    var address = CMIOObjectPropertyAddress(
      mSelector: CMIOObjectPropertySelector(kCMIOHardwarePropertyAllowScreenCaptureDevices),
      mScope: CMIOObjectPropertyScope(kCMIOObjectPropertyScopeGlobal),
      mElement: CMIOObjectPropertyElement(kCMIOObjectPropertyElementMain))
    var allow: UInt32 = 1
    CMIOObjectSetPropertyData(
      CMIOObjectID(kCMIOObjectSystemObject), &address, 0, nil, UInt32(MemoryLayout<UInt32>.size), &allow)
    for name in [AVCaptureDevice.wasConnectedNotification, AVCaptureDevice.wasDisconnectedNotification] {
      NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
        self?.refresh()
      }
    }
    switch AVCaptureDevice.authorizationStatus(for: .video) {
    case .authorized:
      camera = "granted"
      refresh()
    case .notDetermined:
      AVCaptureDevice.requestAccess(for: .video) { ok in
        DispatchQueue.main.async {
          self.camera = ok ? "granted" : "denied"
          self.refresh()
        }
      }
    default:
      camera = "denied"
      onChange?()
    }
    // The phone's screen shows up a moment after the switch is set.
    for delay in [0.5, 1.5, 3.0, 6.0] {
      DispatchQueue.main.asyncAfter(deadline: .now() + delay) { self.refresh() }
    }
  }

  func refresh() {
    guard camera == "granted" else {
      onChange?()
      return
    }
    let found = AVCaptureDevice.DiscoverySession(
      deviceTypes: [.external], mediaType: .muxed, position: .unspecified
    ).devices.first { $0.modelID.contains("iOS") && $0.isConnected }
    if let d = device, found?.uniqueID == d.uniqueID { return }
    session?.stopRunning()
    session = nil
    device = nil
    queue.sync { latest = nil }
    guard let found else {
      onChange?()
      return
    }
    do {
      let s = AVCaptureSession()
      let input = try AVCaptureDeviceInput(device: found)
      guard s.canAddInput(input) else { throw NSError(domain: "trek-phone", code: 1) }
      s.addInput(input)
      let output = AVCaptureVideoDataOutput()
      output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
      output.alwaysDiscardsLateVideoFrames = true
      output.setSampleBufferDelegate(self, queue: queue)
      guard s.canAddOutput(output) else { throw NSError(domain: "trek-phone", code: 2) }
      s.addOutput(output)
      s.startRunning()
      session = s
      device = found
    } catch {
      message(["event": "error", "message": "Couldn't capture \(found.localizedName): \(error.localizedDescription)"])
    }
    onChange?()
  }

  func captureOutput(_ o: AVCaptureOutput, didOutput sb: CMSampleBuffer, from c: AVCaptureConnection) {
    guard let buffer = CMSampleBufferGetImageBuffer(sb) else { return }
    let first = latest == nil
    latest = buffer
    frames += 1
    lastFrame = Date()
    if first { DispatchQueue.main.async { self.onChange?() } }
    guard streaming, Date().timeIntervalSince(lastSent) >= 1.0 / fps else { return }
    lastSent = Date()
    if let jpeg = encode(buffer, maxEdge: nil, maxWidth: Args.maxWidth, png: false) {
      var payload = Data()
      for v in [UInt16(CVPixelBufferGetWidth(buffer)), UInt16(CVPixelBufferGetHeight(buffer))] {
        var be = v.bigEndian
        payload.append(Data(bytes: &be, count: 2))
      }
      emit(1, payload + jpeg)
    }
  }

  func encode(_ buffer: CVPixelBuffer, maxEdge: Int?, maxWidth: Int?, png: Bool) -> Data? {
    var image = CIImage(cvPixelBuffer: buffer)
    var scale = 1.0
    if let maxWidth { scale = min(scale, Double(maxWidth) / image.extent.width) }
    if let maxEdge { scale = min(scale, Double(maxEdge) / max(image.extent.width, image.extent.height)) }
    if scale < 1 { image = image.transformed(by: CGAffineTransform(scaleX: scale, y: scale)) }
    guard let cg = context.createCGImage(image, from: image.extent) else { return nil }
    let data = NSMutableData()
    let type = (png ? UTType.png : UTType.jpeg).identifier as CFString
    guard let dest = CGImageDestinationCreateWithData(data, type, 1, nil) else { return nil }
    CGImageDestinationAddImage(dest, cg, [kCGImageDestinationLossyCompressionQuality: 0.75] as CFDictionary)
    guard CGImageDestinationFinalize(dest) else { return nil }
    return data as Data
  }

  /// The newest frame as PNG at `path`, no longer than `maxEdge` on its long side.
  func shot(path: String, maxEdge: Int) {
    guard let buffer = queue.sync(execute: { latest }) else {
      let why =
        device == nil
        ? (camera == "granted"
          ? "No iPhone screen: plug the iPhone in with a data cable, unlock it and trust this Mac."
          : "Trek isn't allowed to use the camera, which the iPhone's screen counts as: System Settings › Privacy & Security › Camera.")
        : "No picture from the iPhone yet: it may be locked or its screen off. Ask the user to unlock it."
      message(["event": "shot", "error": why])
      return
    }
    let age = lastFrame.map { Date().timeIntervalSince($0) } ?? 0
    let (w, h) = (CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer))
    guard let png = encode(buffer, maxEdge: maxEdge, maxWidth: nil, png: true) else {
      message(["event": "shot", "error": "Couldn't encode the frame"])
      return
    }
    do {
      try png.write(to: URL(fileURLWithPath: path))
      let scale = min(1.0, Double(maxEdge) / Double(max(w, h)))
      message([
        "event": "shot", "path": path, "width": Int((Double(w) * scale).rounded()),
        "height": Int((Double(h) * scale).rounded()), "screen_width": w, "screen_height": h,
        "age": age,
      ])
    } catch {
      message(["event": "shot", "error": "Couldn't write \(path): \(error.localizedDescription)"])
    }
  }
}

// MARK: - Main

let hid = HID()
let screen = Screen()

func status() {
  let (w, h): (Int, Int) = screen.queue.sync {
    screen.latest.map { (CVPixelBufferGetWidth($0), CVPixelBufferGetHeight($0)) } ?? (0, 0)
  }
  let age = screen.lastFrame.map { Date().timeIntervalSince($0) } ?? -1
  let (bluetooth, hosts) = hid.current()
  message([
    "event": "status", "camera": screen.camera, "screen": screen.device?.localizedName ?? "",
    "width": w, "height": h, "frames": screen.frames, "frame_age": age,
    "bluetooth": bluetooth, "hosts": hosts,
  ])
}
screen.onChange = { status() }
hid.onChange = { DispatchQueue.main.async { status() } }

/// Run `hid` tokens in order on their own queue, so a gesture's pauses don't hold up the rest.
let inputQueue = DispatchQueue(label: "trek-phone.input")
func runHID(_ tokens: [Substring]) {
  inputQueue.async {
    var sent = 0
    var missed = 0
    for t in tokens {
      if t.hasPrefix("w"), let ms = Double(t.dropFirst()) {
        Thread.sleep(forTimeInterval: ms / 1000)
      } else if let colon = t.firstIndex(of: ":"), let id = UInt8(t[..<colon]),
        let bytes = hexBytes(t[t.index(after: colon)...])
      {
        if hid.send(id, bytes) { sent += 1 } else { missed += 1 }
      }
    }
    message(["event": "hid", "sent": sent, "missed": missed])
  }
}

Thread {
  while let line = readLine() {
    let parts = line.split(separator: " ")
    guard let cmd = parts.first else { continue }
    switch cmd {
    case "hid": runHID(Array(parts.dropFirst()))
    case "frames":
      let on = parts.count > 1 && parts[1] == "on"
      let fps = parts.count > 2 ? Double(parts[2]) ?? 15 : 15
      screen.queue.async {
        screen.streaming = on
        screen.fps = max(1, min(30, fps))
      }
    case "shot":
      let rest = line.split(separator: " ", maxSplits: 2)
      if rest.count == 3, let edge = Int(rest[1]) {
        DispatchQueue.main.async { screen.shot(path: String(rest[2]), maxEdge: edge) }
      }
    case "status": DispatchQueue.main.async { status() }
    case "quit": exit(0)
    default: message(["event": "error", "message": "unknown command \(cmd)"])
    }
  }
  exit(0)
}.start()

hid.start()
screen.start()
status()
RunLoop.main.run()
