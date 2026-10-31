import SwiftUI

/// A fenced code block as the Mac draws it: its language over the code, and a copy button.
struct CodeBlockView: View {
    var lang: String
    var code: String
    var size: CGFloat = 16
    @State private var copied = false

    /// The fence's language as a label ("rust", "swift"), or nil for a bare fence.
    nonisolated static func label(_ lang: String) -> String? {
        let first = lang.trimmingCharacters(in: .whitespaces).split(separator: " ").first.map(String.init) ?? ""
        // `{.rust}`, `rust,ignore`: the language is the first word in it.
        let word = first.trimmingCharacters(in: CharacterSet(charactersIn: "{}.")).split(separator: ",").first.map(String.init) ?? ""
        return word.isEmpty ? nil : word.lowercased()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text(Self.label(lang) ?? "code")
                    .font(.code(size: 11.5))
                    .foregroundStyle(Trek.muted)
                    .lineLimit(1)
                Spacer(minLength: 8)
                Button {
                    UIPasteboard.general.string = code
                    withAnimation(.snappy(duration: 0.15)) { copied = true }
                    Task {
                        try? await Task.sleep(for: .seconds(1.6))
                        withAnimation(.snappy(duration: 0.2)) { copied = false }
                    }
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: copied ? "checkmark" : "doc.on.doc").font(.system(size: 11, weight: .medium))
                        Text(copied ? "Copied" : "Copy")
                    }
                    .font(.ui(.caption).weight(.medium))
                    .foregroundStyle(copied ? Trek.done : Trek.muted)
                    .padding(.horizontal, 6)
                    .frame(minHeight: 30)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(copied ? "Copied" : "Copy code")
            }
            .padding(.leading, 12)
            .padding(.trailing, 4)
            Rectangle().fill(Trek.border).frame(height: 0.5)
            ScrollView(.horizontal, showsIndicators: false) {
                Text(code)
                    .scaledFont(size * 0.8, design: .monospaced)
                    .foregroundStyle(Trek.foreground)
                    .padding(12)
            }
        }
        .background(Trek.surface, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.5))
    }
}
