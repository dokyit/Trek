// trek-phone: a real iPhone's screen over USB and Trek's Bluetooth keyboard and mouse for it.
//
// Screen: a trusted iPhone on the cable becomes a capture device once a process sets
// kCMIOHardwarePropertyAllowScreenCaptureDevices (QuickTime's switch); frames come through
// AVFoundation. Input: the Mac publishes a Bluetooth LE HID service (HID over GATT) with
// CBPeripheralManager; the iPhone pairs with "Trek" like any keyboard, and with AssistiveTouch on
// it shows a pointer. Trek computes the reports (trek-phone's hid.rs); this only sends them. The
// GATT layout follows xhoantran/iphone-use (MIT).
//
// Mirroring: where Bluetooth can't work (a phone on the Mac's own Apple Account doesn't take it
// as a keyboard), taps and keys go to macOS's iPhone Mirroring window instead, in the background
// as Trek's computer use does it (trek-mcp's desktop/skylight.rs: the same private calls and
// fields), and its state is read through Accessibility (trek-phone's mirror.rs reads it).
//
//   trek-phone <report map hex> <frame max width>
// stdin, one command a line:
//   hid <id>:<hex> w<ms> ...     send reports to the paired phone, pausing as told
//   target <name>                the phone to drive, by its name: no other device gets reports
//   frames on [fps] | frames off stream JPEG frames of the screen
//   shot <max edge> <path>       write the newest frame there as PNG (the path may have spaces)
//   status                       report the state now
//   mirror look | open | press <label> | menu <key> | tap <fx> <fy> <ms>
//     | drag <fx> <fy> <fx> <fy> <ms> | text <utf-8 hex> | key <keycode> <flags>
//                                iPhone Mirroring: act (points as fractions of its window), then
//                                answer with its state
//   mirrorshot <max edge> <path> its window as PNG, answered as a shot
//   mirrorframes on | off        frames from its window instead of the cable
//   quit
// stdout, framed: <type u8><length u32 BE><payload>
//   1  a frame: width u16 BE, height u16 BE (the screen's), then JPEG
//   2  a message (UTF-8 JSON): {"event":"status",...} | {"event":"shot",...} | {"event":"error",...}
//      | {"event":"mirror",...}

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

final class HID: NSObject, CBPeripheralManagerDelegate, CBCentralManagerDelegate {
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
  /// Phones that read the report map: real HID hosts. A phone bonded to the Mac for Continuity
  /// (the same Apple Account) can get old subscriptions restored onto Trek's reports without
  /// ever taking Trek as a keyboard, and drops what's sent. Kept across runs: a bonded host
  /// reconnecting trusts its cached map and doesn't read it again.
  var hidHosts = Set(UserDefaults.standard.stringArray(forKey: "hidHosts") ?? [])
  /// The phone Trek drives, by its name (`target`); nil until Trek says, and every host counts.
  /// Other devices of the account take the keyboard too (an Apple Watch reads the map and
  /// subscribes as soon as Trek advertises), and nothing meant for the phone may reach them.
  var target: String?
  /// What each central is called, looked up without connecting to it.
  var names: [UUID: String] = [:]
  var lookup: CBCentralManager?
  var outbox: [(CBMutableCharacteristic, Data, [CBCentral])] = []
  var state = "starting"
  var onChange: (() -> Void)?
  /// The state and who listens, for readers off the Bluetooth queue: that queue can sit inside
  /// CoreBluetooth while macOS asks the user about Bluetooth, and a status report must not wait
  /// for the answer.
  let snapshotLock = NSLock()
  var snapshot = Snapshot()

  struct Snapshot {
    var state = "starting"
    /// The phone, subscribed and taking Trek as a keyboard and mouse.
    var hosts = 0
    /// The phone, subscribed without ever taking Trek as one (a Continuity bond).
    var linked = 0
    /// Other devices taking Trek as a keyboard and mouse, by name.
    var elsewhere: [String] = []
  }

  func publish() {
    var s = Snapshot(state: state, hosts: hosts.count)
    for c in subscribers where !hidHosts.contains(c.identifier.uuidString) && isTarget(c) {
      s.linked += 1
    }
    for c in subscribers where hidHosts.contains(c.identifier.uuidString) && !isTarget(c) {
      s.elsewhere.append(names[c.identifier] ?? "a device without a name")
    }
    snapshotLock.lock()
    snapshot = s
    snapshotLock.unlock()
  }

  func current() -> Snapshot {
    snapshotLock.lock()
    defer { snapshotLock.unlock() }
    return snapshot
  }

  func start() {
    queue.async { self.manager = CBPeripheralManager(delegate: self, queue: self.queue) }
  }

  /// Drive the phone called `name` from now on.
  func aim(_ name: String) {
    queue.async {
      self.target = name
      self.set(self.hosts.isEmpty ? (self.published ? "advertising" : self.state) : "connected")
    }
  }

  /// Phones subscribed to a report, HID hosts or not.
  var subscribers: [CBCentral] {
    let ids = Set(inputs.values.map(ObjectIdentifier.init))
    return subscriptions.filter { !$0.value.isDisjoint(with: ids) }.compactMap { centrals[$0.key] }
  }

  /// Subscribers that are the phone and take Trek as a keyboard and mouse.
  var hosts: [CBCentral] {
    subscribers.filter { hidHosts.contains($0.identifier.uuidString) && isTarget($0) }
  }

  static func plain(_ name: String) -> String {
    name.replacingOccurrences(of: "\u{2019}", with: "'").trimmingCharacters(in: .whitespaces)
      .lowercased()
  }

  /// Whether `central` is the phone Trek drives (a central whose name can't be found isn't).
  func isTarget(_ central: CBCentral) -> Bool {
    guard let target else { return true }
    if names[central.identifier] == nil, let lookup, lookup.state == .poweredOn,
      let name = lookup.retrievePeripherals(withIdentifiers: [central.identifier]).first?.name
    {
      names[central.identifier] = name
    }
    return names[central.identifier].map { Self.plain($0) == Self.plain(target) } ?? false
  }

  /// Send one report to the phone, if it listens.
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
    // No fixed value, so reads come here: only a phone taking Trek as HID reads the map.
    let map = CBMutableCharacteristic(
      type: CBUUID(string: "2A4B"), properties: [.read], value: nil,
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

  /// Names come once the lookup is up: say who listens again then.
  func centralManagerDidUpdateState(_ c: CBCentralManager) {
    if c.state == .poweredOn { set(state) }
  }

  func peripheralManagerDidUpdateState(_ p: CBPeripheralManager) {
    switch p.state {
    case .poweredOn:
      // Only to look names up (retrievePeripherals): it never scans or connects.
      if lookup == nil { lookup = CBCentralManager(delegate: self, queue: queue) }
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
    case CBUUID(string: "2A4B"):
      value = Data(Args.reportMap)
      becameHost(request.central)
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
      guard hidHosts.contains(central.identifier.uuidString) else {
        // Restored, or not a host yet: ask it to look again (a host that paired before Trek
        // remembered hosts reads the map then), and wait for the read.
        recoverStaleCache(central, force: true)
        set(hosts.isEmpty ? "advertising" : "connected")
        return
      }
      guard isTarget(central) else {
        set(hosts.isEmpty ? "advertising" : "connected")
        return
      }
      outbox.append((input, Data(count: length(input)), [central]))
      drain()
      set("connected")
      advertise()
    }
  }

  /// `central` read the report map: it takes Trek as a keyboard and mouse.
  func becameHost(_ central: CBCentral) {
    centrals[central.identifier] = central
    guard hidHosts.insert(central.identifier.uuidString).inserted else { return }
    UserDefaults.standard.set(Array(hidHosts), forKey: "hidHosts")
    set(hosts.isEmpty ? "advertising" : "connected")
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
  /// subscribes (or, `force`, is subscribed without having read the map): Service Changed makes
  /// it look again. Once a run for each phone.
  func recoverStaleCache(_ central: CBCentral, force: Bool = false) {
    let subscribed = subscriptions[central.identifier] ?? []
    let hasInput = !subscribed.isDisjoint(with: inputs.values.map(ObjectIdentifier.init))
    guard force || !hasInput, subscribed.contains(ObjectIdentifier(serviceChanged)),
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
  /// Frames come from iPhone Mirroring's window instead (`mirrorframes`).
  var mirrored = false
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
    guard streaming, !mirrored, Date().timeIntervalSince(lastSent) >= 1.0 / fps else { return }
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
    encodeImage(CIImage(cvPixelBuffer: buffer), maxEdge: maxEdge, maxWidth: maxWidth, png: png)
  }

  func encodeImage(_ source: CIImage, maxEdge: Int?, maxWidth: Int?, png: Bool) -> Data? {
    var image = source
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

// MARK: - iPhone Mirroring

/// The window server's private calls background input needs, as trek-mcp's skylight.rs uses them
/// (it says what each is for); the public ones stand in where one is missing.
enum Sky {
  static let loaded = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW | RTLD_GLOBAL)
  typealias PostToPid = @convention(c) (pid_t, UnsafeMutableRawPointer) -> Void
  typealias SetField = @convention(c) (UnsafeMutableRawPointer, UInt32, Int64) -> Void
  typealias SetWindowLocation = @convention(c) (UnsafeMutableRawPointer, CGPoint) -> Void
  typealias PostRecord = @convention(c) (UnsafeRawPointer, UnsafeRawPointer) -> Int32
  typealias ProcessForPid = @convention(c) (pid_t, UnsafeMutableRawPointer) -> Int32
  typealias CreateImage = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
  static let post: PostToPid? = find("SLEventPostToPid")
  static let setField: SetField? = find("SLEventSetIntegerValueField")
  static let setWindowLocation: SetWindowLocation? = find("CGEventSetWindowLocation")
  static let postRecord: PostRecord? = find("SLPSPostEventRecordTo")
  static let processForPid: ProcessForPid? = find("GetProcessForPID")
  /// CGWindowListCreateImage: Swift no longer offers it; the window server still does.
  static let createImage: CreateImage? = find("CGWindowListCreateImage")

  static func find<T>(_ name: String) -> T? {
    _ = loaded
    guard let p = dlsym(UnsafeMutableRawPointer(bitPattern: -2), name) else { return nil }
    return unsafeBitCast(p, to: T.self)
  }

  static func ptr(_ e: CGEvent) -> UnsafeMutableRawPointer { Unmanaged.passUnretained(e).toOpaque() }

  static func set(_ e: CGEvent, _ field: UInt32, _ value: Int64) {
    if let setField { setField(ptr(e), field, value) } else if let f = CGEventField(rawValue: field) {
      e.setIntegerValueField(f, value: value)
    }
  }

  static func post(_ pid: pid_t, _ e: CGEvent) {
    if let post { post(pid, ptr(e)) } else { e.postToPid(pid) }
  }

  /// Make window `wid` key inside its app without activating the app or raising the window.
  static func makeKey(_ pid: pid_t, _ wid: CGWindowID) {
    guard let postRecord, let processForPid else { return }
    var psn = [UInt8](repeating: 0, count: 8)
    guard psn.withUnsafeMutableBytes({ processForPid(pid, $0.baseAddress!) }) == 0 else { return }
    func record(_ kind: UInt8) -> [UInt8] {
      var r = [UInt8](repeating: 0, count: 0xf8)
      r[0x04] = 0xf8
      withUnsafeBytes(of: wid.littleEndian) { r.replaceSubrange(0x3c..<0x40, with: $0) }
      r[0x08] = kind
      return r
    }
    var focus = record(0x0d)
    focus[0x8a] = 0x01
    var records = [focus]
    for kind: UInt8 in [0x01, 0x02] {
      var key = record(kind)
      key[0x3a] = 0x10
      for i in 0x20..<0x30 { key[i] = 0xff }
      records.append(key)
    }
    for r in records {
      _ = psn.withUnsafeBytes { p in r.withUnsafeBytes { postRecord(p.baseAddress!, $0.baseAddress!) } }
    }
    Thread.sleep(forTimeInterval: 0.05)
  }
}

final class Mirror {
  static let bundleID = "com.apple.ScreenContinuity"
  /// One action at a time, off the main thread (taps and drags sleep between events).
  let queue = DispatchQueue(label: "trek-phone.mirror")
  /// A point outside every window, for the primer click.
  let nowhere = CGPoint(x: -1, y: -1)

  struct Win {
    let id: CGWindowID
    let pid: pid_t
    /// Screen points, from the top-left of the main display.
    let frame: CGRect
  }

  func app() -> NSRunningApplication? {
    NSRunningApplication.runningApplications(withBundleIdentifier: Self.bundleID).first
  }

  /// Its main window: the phone's screen.
  func window() -> Win? {
    guard let pid = app()?.processIdentifier else { return nil }
    let list =
      CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    for w in list where (w[kCGWindowOwnerPID as String] as? Int32) == pid && (w[kCGWindowLayer as String] as? Int) == 0 {
      guard let b = w[kCGWindowBounds as String] as? NSDictionary,
        let frame = CGRect(dictionaryRepresentation: b), frame.height > 200,
        let id = w[kCGWindowNumber as String] as? CGWindowID
      else { continue }
      return Win(id: id, pid: pid, frame: frame)
    }
    return nil
  }

  func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
  }

  func children(_ e: AXUIElement) -> [AXUIElement] { attr(e, kAXChildrenAttribute) as? [AXUIElement] ?? [] }

  func axWindow(_ pid: pid_t) -> AXUIElement? {
    let windows = attr(AXUIElementCreateApplication(pid), kAXWindowsAttribute) as? [AXUIElement] ?? []
    return windows.first { attr($0, kAXIdentifierAttribute) as? String == "iphone-mirroring-main" } ?? windows.first
  }

  /// What the window says (its messages: locked, timed out, …) and the buttons it offers.
  func contents(_ e: AXUIElement, depth: Int = 0, texts: inout [String], buttons: inout [(String, AXUIElement)]) {
    guard depth < 10 else { return }
    switch attr(e, kAXRoleAttribute) as? String {
    case kAXStaticTextRole:
      if let v = attr(e, kAXValueAttribute) as? String, !v.isEmpty { texts.append(v) }
    case kAXButtonRole:
      let label = (attr(e, kAXDescriptionAttribute) as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? attr(e, kAXTitleAttribute) as? String ?? ""
      buttons.append((label, e))
    default: break
    }
    for c in children(e) { contents(c, depth: depth + 1, texts: &texts, buttons: &buttons) }
  }

  func look() -> [String: Any] {
    var out: [String: Any] = [
      "event": "mirror",
      "installed": NSWorkspace.shared.urlForApplication(withBundleIdentifier: Self.bundleID) != nil,
      "trusted": AXIsProcessTrusted(),
    ]
    guard let app = app() else {
      out["running"] = false
      return out
    }
    out["running"] = true
    if let w = window() {
      out["window"] = Int(w.id)
      out["width"] = Double(w.frame.width)
      out["height"] = Double(w.frame.height)
    }
    var texts: [String] = []
    var buttons: [(String, AXUIElement)] = []
    if let ax = axWindow(app.processIdentifier) { contents(ax, texts: &texts, buttons: &buttons) }
    out["texts"] = texts
    out["buttons"] = buttons.map(\.0)
    return out
  }

  func open() -> String? {
    guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: Self.bundleID) else {
      return "iPhone Mirroring isn't on this Mac (it needs macOS 15 or later)."
    }
    let config = NSWorkspace.OpenConfiguration()
    config.activates = false
    config.addsToRecentItems = false
    let done = DispatchSemaphore(value: 0)
    var failure: String?
    NSWorkspace.shared.openApplication(at: url, configuration: config) { _, error in
      failure = error?.localizedDescription
      done.signal()
    }
    _ = done.wait(timeout: .now() + 10)
    return failure
  }

  /// Press the window's button called `label` (Connect, Try Again).
  func press(_ label: String) -> String? {
    guard let pid = app()?.processIdentifier, let ax = axWindow(pid) else { return "iPhone Mirroring has no window." }
    var texts: [String] = []
    var buttons: [(String, AXUIElement)] = []
    contents(ax, texts: &texts, buttons: &buttons)
    guard let b = buttons.first(where: { $0.0 == label }) else { return "iPhone Mirroring shows no \(label) button." }
    return AXUIElementPerformAction(b.1, kAXPressAction as CFString) == .success ? nil : "Couldn't press \(label)."
  }

  /// Choose the menu item whose shortcut is Command and `key` (View: 1 Home Screen, 2 App
  /// Switcher, 3 Spotlight): through Accessibility, so no key is pressed anywhere.
  func menu(_ key: String) -> String? {
    guard let pid = app()?.processIdentifier,
      let bar = attr(AXUIElementCreateApplication(pid), kAXMenuBarAttribute)
    else { return "iPhone Mirroring isn't running." }
    for top in children(bar as! AXUIElement) {
      for menu in children(top) {
        for item in children(menu) {
          guard attr(item, kAXMenuItemCmdCharAttribute) as? String == key,
            (attr(item, kAXMenuItemCmdModifiersAttribute) as? Int ?? -1) == 0
          else { continue }
          if attr(item, kAXEnabledAttribute) as? Bool == false {
            return "iPhone Mirroring isn't showing the iPhone, so its \(attr(item, kAXTitleAttribute) as? String ?? "menu item") is off."
          }
          return AXUIElementPerformAction(item, kAXPressAction as CFString) == .success ? nil : "Couldn't choose it."
        }
      }
    }
    return "iPhone Mirroring has no menu item for Command-\(key)."
  }

  // Pointer input, stamped for the window as skylight.rs does.

  func mouse(_ w: Win, _ type: CGEventType, _ local: CGPoint, phase: Int64, click: Int64, gesture: Int64) {
    let screen = local == nowhere ? nowhere : CGPoint(x: w.frame.minX + local.x, y: w.frame.minY + local.y)
    guard
      let e = CGEvent(
        mouseEventSource: CGEventSource(stateID: .hidSystemState), mouseType: type,
        mouseCursorPosition: screen, mouseButton: .left)
    else { return }
    e.flags = []
    Sky.set(e, 0, phase)
    Sky.set(e, 7, 3)
    Sky.set(e, 40, Int64(w.pid))
    for f: UInt32 in [51, 91, 92] { Sky.set(e, f, Int64(w.id)) }
    Sky.set(e, 58, gesture)
    Sky.setWindowLocation?(Sky.ptr(e), local)
    Sky.set(e, 1, click)
    Sky.set(e, 3, 0)
    Sky.post(w.pid, e)
  }

  /// Hover, then a click nowhere (some apps want one before they take a background click).
  func approach(_ w: Win, _ at: CGPoint, _ gesture: Int64) {
    Sky.makeKey(w.pid, w.id)
    mouse(w, .mouseMoved, at, phase: 2, click: 0, gesture: gesture)
    Thread.sleep(forTimeInterval: 0.015)
    mouse(w, .leftMouseDown, nowhere, phase: 1, click: 1, gesture: gesture)
    Thread.sleep(forTimeInterval: 0.001)
    mouse(w, .leftMouseUp, nowhere, phase: 2, click: 1, gesture: gesture)
    Thread.sleep(forTimeInterval: 0.1)
  }

  func gesture() -> Int64 { Int64(Date().timeIntervalSince1970.truncatingRemainder(dividingBy: 1) * 1e9) }

  func point(_ w: Win, _ fx: Double, _ fy: Double) -> CGPoint {
    CGPoint(x: min(max(fx, 0), 1) * w.frame.width, y: min(max(fy, 0), 1) * w.frame.height)
  }

  func tap(_ w: Win, _ fx: Double, _ fy: Double, holdMs: Double) {
    let (at, g) = (point(w, fx, fy), gesture())
    approach(w, at, g)
    mouse(w, .leftMouseDown, at, phase: 3, click: 1, gesture: g)
    Thread.sleep(forTimeInterval: max(holdMs, 1) / 1000)
    mouse(w, .leftMouseUp, at, phase: 3, click: 1, gesture: g)
  }

  /// A press dragged from one point to another: a swipe or scroll on the phone.
  func drag(_ w: Win, from: (Double, Double), to: (Double, Double), ms: Double) {
    let (a, b, g) = (point(w, from.0, from.1), point(w, to.0, to.1), gesture())
    approach(w, a, g)
    mouse(w, .leftMouseDown, a, phase: 3, click: 1, gesture: g)
    Thread.sleep(forTimeInterval: 0.06)
    let steps = max(8, Int(ms / 12))
    for i in 1...steps {
      let t = Double(i) / Double(steps)
      let p = CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t)
      mouse(w, .leftMouseDragged, p, phase: 3, click: 1, gesture: g)
      Thread.sleep(forTimeInterval: ms / 1000 / Double(steps))
    }
    Thread.sleep(forTimeInterval: 0.04)
    mouse(w, .leftMouseUp, b, phase: 3, click: 1, gesture: g)
  }

  // Keys go to iPhone Mirroring's process alone, never to the front app.

  func key(_ w: Win, _ code: CGKeyCode, _ flags: UInt64) {
    let source = CGEventSource(stateID: .privateState)
    for down in [true, false] {
      guard let e = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else { return }
      e.flags = CGEventFlags(rawValue: flags)
      e.postToPid(w.pid)
      Thread.sleep(forTimeInterval: down ? 0.015 : 0.008)
    }
  }

  func type(_ w: Win, _ text: String) {
    let source = CGEventSource(stateID: .privateState)
    var chunk: [UniChar] = []
    func flush() {
      guard !chunk.isEmpty else { return }
      for down in [true, false] {
        guard let e = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else { return }
        e.flags = []
        e.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: chunk)
        e.postToPid(w.pid)
      }
      chunk = []
      Thread.sleep(forTimeInterval: 0.03)
    }
    for c in text {
      switch c {
      case "\n", "\r\n":
        flush()
        key(w, 36, 0)
      case "\t":
        flush()
        key(w, 48, 0)
      default:
        // One character an event: the phone's keyboard takes them as typed.
        chunk.append(contentsOf: Array(String(c).utf16))
        flush()
      }
    }
    flush()
  }

  /// The window as a PNG no longer than `maxEdge`, with its size in pixels.
  func shot(path: String, maxEdge: Int) -> [String: Any] {
    guard let w = window() else {
      return ["event": "shot", "error": "iPhone Mirroring has no window."]
    }
    guard let create = Sky.createImage,
      let cg = create(.null, 1 << 3, w.id, 1 | 1 << 3)?.takeRetainedValue()
    else {
      return ["event": "shot", "error": "Couldn't capture iPhone Mirroring's window: Trek needs Screen Recording (System Settings › Privacy & Security)."]
    }
    guard let png = screen.encodeImage(CIImage(cgImage: cg), maxEdge: maxEdge, maxWidth: nil, png: true) else {
      return ["event": "shot", "error": "Couldn't encode the window"]
    }
    do {
      try png.write(to: URL(fileURLWithPath: path))
    } catch {
      return ["event": "shot", "error": "Couldn't write \(path): \(error.localizedDescription)"]
    }
    let scale = min(1.0, Double(maxEdge) / Double(max(cg.width, cg.height)))
    return [
      "event": "shot", "path": path, "width": Int((Double(cg.width) * scale).rounded()),
      "height": Int((Double(cg.height) * scale).rounded()), "screen_width": cg.width,
      "screen_height": cg.height, "age": 0,
    ]
  }

  /// One frame of the window for the panel: the screen's size, then JPEG.
  func frame(maxWidth: Int) -> Data? {
    guard let w = window(), let create = Sky.createImage,
      let cg = create(.null, 1 << 3, w.id, 1 | 1 << 4)?.takeRetainedValue(),
      let jpeg = screen.encodeImage(CIImage(cgImage: cg), maxEdge: nil, maxWidth: maxWidth, png: false)
    else { return nil }
    var payload = Data()
    for v in [UInt16(cg.width), UInt16(cg.height)] {
      var be = v.bigEndian
      payload.append(Data(bytes: &be, count: 2))
    }
    return payload + jpeg
  }

  /// Run one `mirror` command and answer with the state after it.
  func run(_ args: [Substring]) {
    queue.async {
      var error: String?
      let num = { (i: Int) -> Double in i < args.count ? Double(args[i]) ?? 0 : 0 }
      let verb = args.first.map(String.init) ?? "look"
      let acts = ["tap", "drag", "text", "key"].contains(verb)
      let w = acts ? self.window() : nil
      if acts && w == nil {
        error = "iPhone Mirroring has no window."
      } else {
        switch verb {
        case "look": break
        case "open": error = self.open()
        case "press": error = self.press(args.dropFirst().joined(separator: " "))
        case "menu": error = self.menu(args.count > 1 ? String(args[1]) : "")
        case "tap": self.tap(w!, num(1), num(2), holdMs: num(3))
        case "drag": self.drag(w!, from: (num(1), num(2)), to: (num(3), num(4)), ms: max(num(5), 60))
        case "text":
          let bytes = args.count > 1 ? hexBytes(args[1]) ?? [] : []
          self.type(w!, String(decoding: bytes, as: UTF8.self))
        case "key": self.key(w!, CGKeyCode(num(1)), UInt64(num(2)))
        default: error = "unknown mirror command \(verb)"
        }
      }
      var out = self.look()
      if let error { out["error"] = error }
      message(out)
    }
  }
}

// MARK: - Main

let hid = HID()
let screen = Screen()
let mirror = Mirror()

/// While frames stream and come from iPhone Mirroring, its window several times a second.
let mirrorFrames = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "trek-phone.mirror-frames"))
mirrorFrames.schedule(deadline: .now(), repeating: .milliseconds(150))
mirrorFrames.setEventHandler {
  let (on, fps, last) = screen.queue.sync { (screen.streaming && screen.mirrored, min(screen.fps, 8), screen.lastSent) }
  guard on, Date().timeIntervalSince(last) >= 1.0 / fps else { return }
  screen.queue.sync { screen.lastSent = Date() }
  if let frame = mirror.frame(maxWidth: Args.maxWidth) { emit(1, frame) }
}
mirrorFrames.resume()

func status() {
  let (w, h): (Int, Int) = screen.queue.sync {
    screen.latest.map { (CVPixelBufferGetWidth($0), CVPixelBufferGetHeight($0)) } ?? (0, 0)
  }
  let age = screen.lastFrame.map { Date().timeIntervalSince($0) } ?? -1
  let bt = hid.current()
  message([
    "event": "status", "camera": screen.camera, "screen": screen.device?.localizedName ?? "",
    "width": w, "height": h, "frames": screen.frames, "frame_age": age,
    "bluetooth": bt.state, "hosts": bt.hosts, "linked": bt.linked, "elsewhere": bt.elsewhere,
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
    case "mirror": mirror.run(Array(parts.dropFirst()))
    case "mirrorshot":
      let rest = line.split(separator: " ", maxSplits: 2)
      if rest.count == 3, let edge = Int(rest[1]) {
        DispatchQueue.main.async { message(mirror.shot(path: String(rest[2]), maxEdge: edge)) }
      }
    case "mirrorframes":
      let on = parts.count > 1 && parts[1] == "on"
      screen.queue.async { screen.mirrored = on }
    case "target":
      let rest = line.split(separator: " ", maxSplits: 1)
      if rest.count == 2 { hid.aim(String(rest[1])) }
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
