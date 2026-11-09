import ImageIO
import UIKit
import UniformTypeIdentifiers
import XCTest

/// The screen as a JPEG, for the stream: asked of XCTest's daemon as a JPEG directly, which is
/// what WebDriverAgent's MJPEG server does (github.com/appium/WebDriverAgent, BSD-3-Clause:
/// `FBScreenshot.m`, `FBMjpegServer.m`; the technique is theirs, the code here is Trek's). The
/// public `XCUIScreen.screenshot()` has the daemon encode a lossless picture, which the runner
/// then decodes into a `UIImage`, draws smaller and encodes again; this asks for the JPEG once.
/// None of it is public API: every call is checked for, and `xctest` (the public way) is what
/// the stream falls back to.
enum Capture {
    /// How a frame was taken, for `/status` and `/screenshot?via=`.
    enum Via: String {
        /// XCTest's daemon, asked for a JPEG.
        case daemon
        /// `XCUIScreen.screenshot()`, drawn and encoded in the runner.
        case xctest
    }

    // MARK: The daemon's JPEG

    /// The whole screen as a JPEG of `quality` (1–100) in its own pixels, straight from XCTest's
    /// daemon; nil when this XCTest hasn't the calls, the daemon refused, or it took longer than
    /// `timeout`. Blocks its thread meanwhile: never the main one.
    static func daemonJPEG(quality: Int, timeout: TimeInterval = 2) -> Data? {
        guard let proxy = daemon, let request = request(quality: quality) else { return nil }
        let done = DispatchSemaphore(value: 0)
        let box = Box()
        let reply: @convention(block) (AnyObject?, NSError?) -> Void = { image, _ in
            if let image = image as? NSObject, image.responds(to: NSSelectorFromString("data")) {
                box.set(image.value(forKey: "data") as? Data)
            }
            done.signal()
        }
        _ = proxy.perform(requestScreenshot, with: request, with: unsafeBitCast(reply, to: AnyObject.self))
        guard done.wait(timeout: .now() + timeout) == .success else { return nil }
        guard let data = box.get(), isJPEG(data) else { return nil }
        return data
    }

    /// What the daemon's reply (on a queue of its own) leaves for the thread waiting on it.
    private final class Box {
        private let lock = NSLock()
        private var data: Data?
        func set(_ data: Data?) { lock.lock(); self.data = data; lock.unlock() }
        func get() -> Data? { lock.lock(); defer { lock.unlock() }; return data }
    }

    private static let requestScreenshot = NSSelectorFromString("_XCT_requestScreenshot:withReply:")

    /// XCTest's connection to its daemon (`XCTRunnerDaemonSession.sharedSession.daemonProxy`),
    /// when it takes screenshot requests.
    private static let daemon: AnyObject? = {
        let shared = NSSelectorFromString("sharedSession")
        let proxySel = NSSelectorFromString("daemonProxy")
        guard let cls = NSClassFromString("XCTRunnerDaemonSession") as? NSObject.Type,
              cls.responds(to: shared),
              let session = cls.perform(shared)?.takeUnretainedValue() as? NSObject,
              session.responds(to: proxySel),
              let proxy = session.perform(proxySel)?.takeUnretainedValue(),
              proxy.responds(to: requestScreenshot)
        else { return nil }
        return proxy
    }()

    /// The main screen's id for the daemon.
    private static let screenID: Int64? = {
        let screen = XCUIScreen.main as NSObject
        guard screen.responds(to: NSSelectorFromString("displayID")) else { return nil }
        return (screen.value(forKey: "displayID") as? NSNumber)?.int64Value
    }()

    /// A request for the whole main screen as a JPEG of `quality`: an `XCTScreenshotRequest`
    /// with an `XCTImageEncoding`, both made through their initializers' implementations (their
    /// arguments aren't objects), and only when those still take what's passed here.
    private static func request(quality: Int) -> NSObject? {
        guard let screenID,
              let encodingClass = NSClassFromString("XCTImageEncoding") as? NSObject.Type,
              let requestClass = NSClassFromString("XCTScreenshotRequest") as? NSObject.Type
        else { return nil }
        let encodingInit = NSSelectorFromString("initWithUniformTypeIdentifier:compressionQuality:")
        let requestInit = NSSelectorFromString("initWithScreenID:rect:encoding:")
        guard let makeEncoding = class_getInstanceMethod(encodingClass, encodingInit),
              let makeRequest = class_getInstanceMethod(requestClass, requestInit),
              shape(makeEncoding) == "@@:@d",
              shape(makeRequest) == "@@:q{CGRect={CGPoint=dd}{CGSize=dd}}@",
              let blankEncoding = allocate(encodingClass),
              let blankRequest = allocate(requestClass)
        else { return nil }
        typealias EncodingInit = @convention(c) (AnyObject, Selector, NSString, Double) -> Unmanaged<NSObject>?
        let encode = unsafeBitCast(method_getImplementation(makeEncoding), to: EncodingInit.self)
        let fraction = Double(min(max(quality, 1), 100)) / 100
        guard let encoding = encode(blankEncoding, encodingInit, UTType.jpeg.identifier as NSString, fraction)?
            .takeRetainedValue()
        else { return nil }
        typealias RequestInit = @convention(c) (AnyObject, Selector, Int64, CGRect, AnyObject) -> Unmanaged<NSObject>?
        let make = unsafeBitCast(method_getImplementation(makeRequest), to: RequestInit.self)
        // A null rect: the whole screen.
        return make(blankRequest, requestInit, screenID, .null, encoding)?.takeRetainedValue()
    }

    /// A method's arguments and return as Objective-C encodes them, without the stack offsets
    /// (`@32@0:8@16d24` is `@@:@d`).
    static func shape(_ method: Method) -> String {
        guard let types = method_getTypeEncoding(method) else { return "" }
        return shape(String(cString: types))
    }

    static func shape(_ encoding: String) -> String {
        String(encoding.filter { !$0.isNumber })
    }

    private static func allocate(_ cls: NSObject.Type) -> AnyObject? {
        let alloc = NSSelectorFromString("alloc")
        guard let method = class_getClassMethod(cls, alloc) else { return nil }
        typealias Alloc = @convention(c) (AnyClass, Selector) -> Unmanaged<AnyObject>?
        let make = unsafeBitCast(method_getImplementation(method), to: Alloc.self)
        // +1, handed to init, which consumes it.
        return make(cls, alloc)?.takeUnretainedValue()
    }

    static func isJPEG(_ data: Data) -> Bool {
        data.count > 3 && data[data.startIndex] == 0xFF && data[data.startIndex + 1] == 0xD8
    }

    // MARK: Smaller

    /// A JPEG's size in pixels, from its header.
    static func size(_ jpeg: Data) -> (width: Int, height: Int)? {
        guard let source = CGImageSourceCreateWithData(jpeg as CFData, nil),
              let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = (props[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue,
              let height = (props[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue,
              width > 0, height > 0
        else { return nil }
        return (width, height)
    }

    /// `jpeg` at most `width` pixels wide, as a JPEG of `quality`: itself when it's no wider (or
    /// `width` is 0). ImageIO decodes it at the smaller size in one pass and encodes it once,
    /// with no `UIImage` between.
    static func scaled(_ jpeg: Data, width: Int, quality: Int) -> Data? {
        guard width > 0 else { return jpeg }
        let lazy = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let source = CGImageSourceCreateWithData(jpeg as CFData, lazy),
              let (w, h) = size(jpeg)
        else { return nil }
        guard w > width else { return jpeg }
        let longest = (Double(max(w, h)) * Double(width) / Double(w)).rounded()
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceThumbnailMaxPixelSize: Int(longest),
            kCGImageSourceShouldCacheImmediately: true,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else {
            return nil
        }
        let out = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(out, UTType.jpeg.identifier as CFString, 1, nil)
        else { return nil }
        let fraction = Double(min(max(quality, 1), 100)) / 100
        let properties = [kCGImageDestinationLossyCompressionQuality: fraction] as CFDictionary
        CGImageDestinationAddImage(destination, image, properties)
        guard CGImageDestinationFinalize(destination) else { return nil }
        return out as Data
    }

    // MARK: The public way

    /// The screen through `XCUIScreen.screenshot()`, at most `width` pixels wide (0: as it is):
    /// a JPEG of `quality`, or with a `quality` of 0 a PNG. What `/screenshot` has always
    /// answered with.
    static func xctest(width: Int, quality: Int) -> (type: String, data: Data)? {
        let shot = XCUIScreen.main.screenshot()
        var image = shot.image
        let pixels = image.size.width * image.scale
        guard quality > 0 || (width > 0 && pixels > CGFloat(width)) else {
            return ("image/png", shot.pngRepresentation)
        }
        if width > 0, pixels > CGFloat(width) {
            let factor = CGFloat(width) / pixels
            let size = CGSize(width: (image.size.width * image.scale * factor).rounded(),
                              height: (image.size.height * image.scale * factor).rounded())
            let format = UIGraphicsImageRendererFormat()
            format.scale = 1
            image = UIGraphicsImageRenderer(size: size, format: format).image { _ in
                image.draw(in: CGRect(origin: .zero, size: size))
            }
        }
        let (type, data) = quality > 0
            ? ("image/jpeg", image.jpegData(compressionQuality: CGFloat(min(quality, 100)) / 100))
            : ("image/png", image.pngData())
        return data.map { (type, $0) }
    }

    /// The daemon's JPEG when the picture is made smaller afterwards is asked for at this
    /// quality at least, so encoding it twice doesn't show.
    static let sourceQuality = 90

    /// How many times a picture `native` pixels wide is halved to be at most `width` wide (1:
    /// not at all; at most 8). Halving is what a JPEG decodes to cheaply: on the way to 900
    /// pixels a 1206-wide screen took 85 ms through ImageIO, and 21 ms to half its size.
    static func halving(native: Int, width: Int) -> Int {
        guard width > 0, native > width else { return 1 }
        var by = 2
        while by < 8, native / by > width { by *= 2 }
        return by
    }

    /// `jpeg` at a `by`th of its size (a power of two), as a JPEG of `quality`.
    static func halved(_ jpeg: Data, by: Int, quality: Int) -> Data? {
        guard by > 1 else { return jpeg }
        guard let (w, _) = size(jpeg) else { return nil }
        return scaled(jpeg, width: Int((Double(w) / Double(by)).rounded(.up)), quality: quality)
    }
}
