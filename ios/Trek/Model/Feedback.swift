import Foundation

/// Feedback from the phone (Settings › Send feedback…), as the Mac's form sends it: what happened,
/// a bug, an idea or something else, an email if the user would like a reply, and, unless they turn
/// it off, the app's version, iOS version and architecture. It goes straight to PostHog as a
/// `feedback` event with the project key the app was built with (docs/PRIVACY.md); without one, or
/// in the demo, nothing is sent and the report is copied instead.
enum FeedbackKind: String, CaseIterable, Identifiable {
    case bug, idea, other
    var id: String { rawValue }
    var label: String {
        switch self {
        case .bug: "Bug"
        case .idea: "Idea"
        case .other: "Other"
        }
    }
    var placeholder: String {
        switch self {
        case .bug: "What happened, and what did you expect?"
        case .idea: "What would you like Trek to do?"
        case .other: "What's on your mind?"
        }
    }
}

/// What "Include diagnostics" adds.
struct FeedbackDiagnostics: Equatable {
    var appVersion: String
    var os: String
    var osVersion: String
    var arch: String

    static var here: FeedbackDiagnostics {
        #if arch(arm64)
        let arch = "aarch64"
        #else
        let arch = "x86_64"
        #endif
        let v = ProcessInfo.processInfo.operatingSystemVersion
        return FeedbackDiagnostics(
            appVersion: ClientMessage.appVersion,
            os: "iOS",
            osVersion: "\(v.majorVersion).\(v.minorVersion)" + (v.patchVersion > 0 ? ".\(v.patchVersion)" : ""),
            arch: arch
        )
    }
}

struct Feedback: Equatable {
    var kind: FeedbackKind = .bug
    var message = ""
    var email = ""
    var diagnostics: FeedbackDiagnostics? = .here

    var trimmedMessage: String { message.trimmingCharacters(in: .whitespacesAndNewlines) }
    var trimmedEmail: String { email.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// What's wrong with it as typed, if anything.
    var problem: String? {
        if trimmedMessage.isEmpty { return "Write what happened first." }
        if !trimmedEmail.isEmpty && !Feedback.plausible(email: trimmedEmail) {
            return "That email address doesn't look right."
        }
        return nil
    }

    /// One `@`, a dot after it, no spaces.
    static func plausible(email: String) -> Bool {
        let parts = email.split(separator: "@", omittingEmptySubsequences: false)
        guard parts.count == 2, !parts[0].isEmpty, !email.contains(where: \.isWhitespace) else { return false }
        let domain = parts[1]
        return domain.contains(".") && !domain.hasPrefix(".") && !domain.hasSuffix(".")
    }

    /// The `feedback` event's properties: the phone's details only with diagnostics.
    var properties: [String: Any] {
        var p: [String: Any] = [
            "$lib": "trek-ios",
            "$process_person_profile": false,
            "$geoip_disable": true,
            "kind": kind.rawValue,
            "message": trimmedMessage,
            "diagnostics": diagnostics != nil,
        ]
        if !trimmedEmail.isEmpty { p["email"] = trimmedEmail }
        if let d = diagnostics {
            p["$app_version"] = d.appVersion
            p["$os"] = d.os
            p["$os_version"] = d.osVersion
            p["arch"] = d.arch
        }
        return p
    }

    /// The request body PostHog's batch API takes. A one-off id: feedback is tied to nothing.
    func body(apiKey: String, at: Date = Date(), id: UUID = UUID()) throws -> Data {
        let stamp = ISO8601DateFormatter()
        stamp.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let event: [String: Any] = [
            "event": "feedback",
            "distinct_id": id.uuidString.lowercased(),
            "properties": properties,
            "timestamp": stamp.string(from: at),
            "uuid": UUID().uuidString.lowercased(),
        ]
        return try JSONSerialization.data(withJSONObject: ["api_key": apiKey, "batch": [event]], options: [.sortedKeys])
    }

    /// The report as text: what the form shows before sending, and what Copy report copies.
    var report: String {
        var out = "Trek feedback: \(kind.label)\n\n\(trimmedMessage)\n"
        if !trimmedEmail.isEmpty { out += "\nReply to: \(trimmedEmail)\n" }
        if let d = diagnostics {
            out += "\nTrek \(d.appVersion) for iPhone\n\(d.os) \(d.osVersion), \(d.arch)\n"
        } else {
            out += "\nNo diagnostics.\n"
        }
        return out
    }

    /// A new GitHub issue, filled in; never the email (an issue is public).
    var githubIssueURL: URL {
        var c = URLComponents(string: "https://github.com/dokyit/Trek/issues/new")!
        let title = String(trimmedMessage.split(separator: "\n").first ?? "").prefix(80)
        var body = "\(trimmedMessage)\n"
        if let d = diagnostics {
            body += "\n**Trek** \(d.appVersion) for iPhone · \(d.os) \(d.osVersion), \(d.arch)\n"
        }
        var items = [URLQueryItem(name: "title", value: String(title)), URLQueryItem(name: "body", value: body)]
        switch kind {
        case .bug: items.append(URLQueryItem(name: "labels", value: "bug"))
        case .idea: items.append(URLQueryItem(name: "labels", value: "enhancement"))
        case .other: break
        }
        c.queryItems = items
        return c.url!
    }
}

/// The PostHog project the app was built with: `TrekPostHogKey` and `TrekPostHogHost` in its
/// Info.plist, from the `TREK_POSTHOG_KEY` and `TREK_POSTHOG_HOST` build settings (the same as the
/// Mac's). Only a project key (`phc_…`), which can add events and read nothing.
struct PostHogProject: Equatable {
    var key: String
    var host: String

    static let defaultHost = "https://us.i.posthog.com"

    static func from(key: String?, host: String?) -> PostHogProject? {
        guard let key = key?.trimmingCharacters(in: .whitespaces), key.hasPrefix("phc_"), key.count >= 20,
              !key.contains("$(") else { return nil }
        var host = host?.trimmingCharacters(in: .whitespaces) ?? ""
        if host.isEmpty || host.contains("$(") { host = defaultHost }
        guard host.hasPrefix("https://") else { return nil }
        while host.hasSuffix("/") { host.removeLast() }
        return PostHogProject(key: key, host: host)
    }

    /// The app's, if it was built with one; never in the demo, which sends nothing.
    static var compiled: PostHogProject? {
        if Launch.demo { return nil }
        let info = Bundle.main.infoDictionary
        return from(key: info?["TrekPostHogKey"] as? String, host: info?["TrekPostHogHost"] as? String)
    }

    var hostName: String { host.replacingOccurrences(of: "https://", with: "") }
    var batchURL: URL { URL(string: "\(host)/batch/")! }
}

enum FeedbackError: Error, Equatable {
    /// The app was built without a key.
    case noKey
    case failed(String)

    var message: String {
        switch self {
        case .noKey: "This build has nowhere to send reports yet. Copy the report and email it, or report it on GitHub."
        case .failed(let why): "Couldn't send it (\(why)). Try again, or copy the report."
        }
    }
}

/// What posts a report: PostHog over the network, or a stand-in in tests.
protocol FeedbackSending {
    func post(_ body: Data, to url: URL) async throws -> Int
}

struct URLSessionFeedback: FeedbackSending {
    func post(_ body: Data, to url: URL) async throws -> Int {
        var request = URLRequest(url: url, timeoutInterval: 20)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        let (_, response) = try await URLSession.shared.data(for: request)
        return (response as? HTTPURLResponse)?.statusCode ?? 0
    }
}

extension Feedback {
    /// Send it to `project` through `sender`.
    func send(to project: PostHogProject?, with sender: FeedbackSending = URLSessionFeedback()) async throws(FeedbackError) {
        guard let project else { throw .noKey }
        let body: Data
        do { body = try self.body(apiKey: project.key) } catch { throw .failed("it couldn't be written") }
        let status: Int
        do { status = try await sender.post(body, to: project.batchURL) } catch {
            throw .failed((error as? URLError)?.code == .notConnectedToInternet ? "no connection" : error.localizedDescription)
        }
        guard (200..<300).contains(status) else { throw .failed("PostHog answered \(status)") }
    }
}
