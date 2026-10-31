import SwiftUI

/// A question's typed answer. A secret one (a password, a token) is masked as it's typed, as on
/// the Mac, with an eye to check it.
struct AnswerField: View {
    var secret: Bool
    @Binding var text: String
    @State private var shown = false

    var body: some View {
        HStack(spacing: 6) {
            if secret && !shown {
                SecureField("Type a secret answer", text: $text)
                    .textContentType(.password)
            } else {
                TextField(secret ? "Type a secret answer" : "Or type an answer", text: $text)
                    .textInputAutocapitalization(secret ? .never : nil)
                    .autocorrectionDisabled(secret)
            }
            if secret {
                Button {
                    shown.toggle()
                } label: {
                    Image(systemName: shown ? "eye.slash" : "eye")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(Trek.muted)
                        .frame(width: 28, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(shown ? "Hide the answer" : "Show the answer")
            }
        }
    }
}
