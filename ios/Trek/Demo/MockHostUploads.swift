import Foundation

/// The demo Mac taking videos and files sent with a message (`uploads`): each piece answered with
/// how far the file got, a moment later, as over Wi-Fi; the message then mentions each where a Mac
/// keeps them, so the bubble shows its chips.
extension MockHost {
    /// Upload ids the demo Mac has whole, and their names.
    private static var received: [String: String] = [:]

    func takeUpload(_ message: ClientMessage, id: String?) {
        guard case .uploadPart(let uploadId, let name, let total, let offset, let data) = message else { return }
        let bytes = Int64(data.count / 4 * 3)
        let got = min(total, offset + bytes)
        let key = uploadId.lowercased()
        if got >= total { Self.received[key] = name }
        // A piece takes a moment on Wi-Fi; the strip's progress shows it.
        deliver(.uploadProgress(re: id, UploadProgress(uploadId: key, received: got, total: total, done: got >= total ? true : nil)),
                after: 0.25)
    }

    /// `text` with a mention of each file after it, as the Mac adds them (`uploads::mentioning`).
    func mentioning(_ text: String, _ files: [String]) -> String {
        let stamp = Date().formatted(.verbatim("\(year: .defaultDigits)-\(month: .twoDigits)-\(day: .twoDigits) at \(hour: .twoDigits(clock: .twentyFourHour, hourCycle: .zeroBased)).\(minute: .twoDigits).\(second: .twoDigits)",
                                               timeZone: .current, calendar: .current))
        let mentions = files.map { id -> String in
            let name = Self.received[id.lowercased()] ?? "file"
            return "@\"/Users/demo/Library/Application Support/dev.trek.Trek/uploads/iPhone \(stamp) \(name)\""
        }
        guard !mentions.isEmpty else { return text }
        let words = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return ([words].filter { !$0.isEmpty } + mentions).joined(separator: " ")
    }
}
