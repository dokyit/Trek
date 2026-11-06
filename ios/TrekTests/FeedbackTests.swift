import Foundation
import Testing
@testable import Trek

/// PostHog's stand-in: answers `status` (or fails), and keeps what it was given.
final class FakePostHog: FeedbackSending {
    var status = 200
    var offline = false
    var posted: [(URL, [String: Any])] = []

    func post(_ body: Data, to url: URL) async throws -> Int {
        if offline { throw URLError(.notConnectedToInternet) }
        posted.append((url, try JSONSerialization.jsonObject(with: body) as? [String: Any] ?? [:]))
        return status
    }
}

struct FeedbackTests {
    private let project = PostHogProject(key: "phc_0123456789abcdefABCDEF", host: "https://us.i.posthog.com")
    private let diagnostics = FeedbackDiagnostics(appVersion: "0.4.41", os: "iOS", osVersion: "26.1", arch: "aarch64")

    private func bug(_ message: String = "The thread list froze", email: String = "") -> Feedback {
        Feedback(kind: .bug, message: message, email: email, diagnostics: diagnostics)
    }

    @Test func onlyAProjectKeyIsAKey() {
        #expect(PostHogProject.from(key: "phc_0123456789abcdefABCDEF", host: "") == project)
        // A build without the settings leaves them empty, or as written.
        #expect(PostHogProject.from(key: "", host: "") == nil)
        #expect(PostHogProject.from(key: "$(TREK_POSTHOG_KEY)", host: nil) == nil)
        #expect(PostHogProject.from(key: "phx_personal_0123456789abcdef", host: nil) == nil)
        #expect(PostHogProject.from(key: "phc_0123456789abcdefABCDEF", host: "http://example.com") == nil)
        #expect(PostHogProject.from(key: "phc_0123456789abcdefABCDEF", host: "https://eu.i.posthog.com/")?.batchURL
                == URL(string: "https://eu.i.posthog.com/batch/"))
    }

    @Test func itNeedsAMessageAndAnEmailThatCouldBeOne() {
        #expect(bug("   ").problem == "Write what happened first.")
        #expect(bug(email: "me at example").problem == "That email address doesn't look right.")
        #expect(bug(email: "me@example").problem != nil)
        #expect(bug(email: " me@example.com ").problem == nil)
        #expect(bug().problem == nil)
    }

    @Test func theEventCarriesThePhoneOnlyWithDiagnostics() throws {
        let body = try JSONSerialization.jsonObject(with: bug(email: "me@example.com").body(apiKey: project.key)) as? [String: Any]
        #expect(body?["api_key"] as? String == project.key)
        let event = try #require((body?["batch"] as? [[String: Any]])?.first)
        #expect(event["event"] as? String == "feedback")
        let p = try #require(event["properties"] as? [String: Any])
        #expect(p["message"] as? String == "The thread list froze")
        #expect(p["email"] as? String == "me@example.com")
        #expect(p["$os"] as? String == "iOS")
        #expect(p["$process_person_profile"] as? Bool == false)
        var bare = bug()
        bare.diagnostics = nil
        let q = bare.properties
        #expect(q["$os"] == nil && q["$app_version"] == nil && q["email"] == nil)
        #expect(q["diagnostics"] as? Bool == false)
    }

    @Test func theReportSaysWhatWouldGoAndTheIssueLeavesTheEmailOut() throws {
        let report = bug(email: "me@example.com").report
        #expect(report.hasPrefix("Trek feedback: Bug\n\nThe thread list froze"))
        #expect(report.contains("Reply to: me@example.com"))
        #expect(report.contains("iOS 26.1, aarch64"))
        let url = bug(email: "me@example.com").githubIssueURL
        let items = try #require(URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems)
        #expect(items.first { $0.name == "title" }?.value == "The thread list froze")
        #expect(items.first { $0.name == "labels" }?.value == "bug")
        let body = try #require(items.first(where: { $0.name == "body" })?.value)
        #expect(body.contains("The thread list froze") && body.contains("iOS 26.1"), "\(body)")
        #expect(body.contains("me@example.com") == false, "an issue is public: \(body)")
    }

    @Test func itGoesToPostHogAndSaysWhenItDidnt() async throws {
        let posthog = FakePostHog()
        try await bug().send(to: project, with: posthog)
        #expect(posthog.posted.count == 1)
        #expect(posthog.posted.first?.0 == URL(string: "https://us.i.posthog.com/batch/"))

        posthog.status = 503
        await #expect(throws: FeedbackError.failed("PostHog answered 503")) {
            try await bug().send(to: project, with: posthog)
        }
        posthog.offline = true
        await #expect(throws: FeedbackError.failed("no connection")) {
            try await bug().send(to: project, with: posthog)
        }
        // Without a key nothing goes at all.
        let untouched = FakePostHog()
        await #expect(throws: FeedbackError.noKey) {
            try await bug().send(to: nil, with: untouched)
        }
        #expect(untouched.posted.isEmpty)
    }
}
