import SwiftUI

/// Settings › Send feedback…: the Mac's form, on the phone. What it sends is said in a line, and
/// shown whole on request, before it goes; without a key the button copies the report instead.
struct FeedbackView: View {
    var project: PostHogProject? = .compiled
    var sender: FeedbackSending = URLSessionFeedback()

    @State private var feedback = Feedback()
    @State private var includeDiagnostics = true
    @State private var sending = false
    @State private var sent = false
    @State private var problem: String?
    @State private var failure: FeedbackError?
    @State private var copied = false
    @Environment(\.openURL) private var openURL

    private var shown: Feedback {
        var f = feedback
        f.diagnostics = includeDiagnostics ? .here : nil
        return f
    }

    var body: some View {
        Form {
            if sent {
                Section {
                    Label("Thanks, it's sent", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                } footer: {
                    Text(feedback.trimmedEmail.isEmpty
                         ? "It went to Trek's developer."
                         : "It went to Trek's developer, who may write back to the email you gave.")
                        .font(.ui(.footnote))
                }
            } else {
                Section {
                    Picker("Kind", selection: $feedback.kind) {
                        ForEach(FeedbackKind.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
                }
                Section {
                    TextField(feedback.kind.placeholder, text: $feedback.message, axis: .vertical)
                        .lineLimit(4...10)
                        .accessibilityIdentifier("feedback-message")
                    TextField("Email, if you'd like a reply (optional)", text: $feedback.email)
                        .keyboardType(.emailAddress)
                        .textContentType(.emailAddress)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } footer: {
                    if let problem {
                        Text(problem).font(.ui(.footnote)).foregroundStyle(.red)
                    }
                }
                Section {
                    Toggle("Include diagnostics", isOn: $includeDiagnostics)
                    DisclosureGroup(summary) {
                        Text(shown.report)
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(Trek.muted)
                            .textSelection(.enabled)
                    }
                    .font(.ui(.footnote))
                } footer: {
                    Text("Diagnostics are the app's version, the iOS version and the architecture.")
                        .font(.ui(.footnote))
                }
                Section {
                    Button(project == nil ? "Copy report" : (failure == nil ? "Send" : "Try again")) { submit() }
                        .disabled(sending)
                    if project != nil {
                        Button("Copy report") { copy() }
                    }
                    Button("Report a bug on GitHub", systemImage: "arrow.up.right.square") {
                        if check() { openURL(shown.githubIssueURL) }
                    }
                } footer: {
                    Text(status).font(.ui(.footnote)).foregroundStyle(failure == nil ? Trek.muted : .red)
                }
            }
        }
        .scrollContentBackground(.hidden)
        .fadesUnderBottomBars(Trek.grouped)
        .background(Trek.grouped)
        .navigationTitle("Send feedback")
        .navigationBarTitleDisplayMode(.inline)
        .onChange(of: feedback) { problem = nil }
    }

    private var summary: String {
        let you = shown.trimmedEmail.isEmpty ? "your message" : "your message and email"
        guard let d = shown.diagnostics else { return "Sends \(you), and nothing else." }
        return "Sends \(you), Trek \(d.appVersion) on \(d.os) \(d.osVersion) (\(d.arch))."
    }

    private var status: String {
        if let failure { return failure.message }
        if copied { return project == nil ? "Report copied: email it, or report it on GitHub." : "Report copied." }
        guard let project else { return FeedbackError.noKey.message }
        return "Goes to Trek's developer through PostHog (\(project.hostName))."
    }

    private func check() -> Bool {
        problem = shown.problem
        return problem == nil
    }

    private func copy() {
        guard check() else { return }
        UIPasteboard.general.string = shown.report
        copied = true
    }

    private func submit() {
        guard check() else { return }
        guard project != nil else { return copy() }
        sending = true
        let report = shown
        Task {
            do throws(FeedbackError) {
                try await report.send(to: project, with: sender)
                failure = nil
                sent = true
            } catch {
                failure = error
            }
            sending = false
        }
    }
}
