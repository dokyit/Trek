import SwiftUI

/// Search: the threads on the phone filter as you type; where the Mac searches (`search`), what
/// it finds follows a moment later: titles, then messages (with the words that matched), every
/// thread the Mac has, settled long ago or a sub-agent's, and each one opens.
struct SearchView: View {
    @Environment(AppModel.self) private var model
    @State private var query = ""
    @State private var path: [String] = []
    /// The Mac's answer, and the text it answers.
    @State private var found: [SearchResult] = []
    @State private var foundFor = ""
    @State private var searching = false
    @Environment(\.compactRows) private var compact

    private var trimmed: String { query.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var onMac: Bool { model.serves(.search) }

    /// What the phone has that matches: titles, projects, agents, branches.
    private var local: [ThreadSummary] {
        let q = trimmed.lowercased()
        let sorted = model.threads.sorted { $0.updatedAt > $1.updatedAt }
        guard !q.isEmpty else { return sorted }
        return sorted.filter {
            $0.title.lowercased().contains(q) || ($0.project?.name.lowercased().contains(q) ?? false)
                || $0.agent.name.lowercased().contains(q) || ($0.branch?.lowercased().contains(q) ?? false)
        }
    }

    /// The Mac's results (while they answer what's typed, or a text it extends), then what only
    /// the phone matched.
    private var results: [SearchResult] {
        let mine = local.map { SearchResult(thread: $0) }
        guard onMac, !trimmed.isEmpty, !foundFor.isEmpty,
              trimmed.lowercased().hasPrefix(foundFor.lowercased()) || foundFor.lowercased().hasPrefix(trimmed.lowercased()) else { return mine }
        let ids = Set(found.map(\.id))
        return found + mine.filter { !ids.contains($0.id) }
    }

    var body: some View {
        let results = results
        NavigationStack(path: $path) {
            List {
                if searching {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Searching messages on \(model.host?.name ?? "your Mac")…").font(.ui(.footnote)).foregroundStyle(Trek.muted)
                    }
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                }
                ForEach(results) { r in
                    NavigationLink(value: r.thread.id) {
                        VStack(alignment: .leading, spacing: 5) {
                            ThreadRow(thread: model.thread(r.thread.id) ?? r.thread)
                            SearchHitLine(result: r, parent: r.thread.parentId.flatMap { model.thread($0)?.title })
                                .padding(.leading, compact ? 28 : 37)
                        }
                    }
                    .navigationLinkIndicatorVisibility(.hidden)
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: compact ? 7 : 9, leading: 20, bottom: compact ? 7 : 9, trailing: 18))
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(Trek.background)
            .overlay {
                if results.isEmpty, !searching {
                    ContentUnavailableView.search(text: query)
                }
            }
            .navigationTitle("Search")
            .searchable(text: $query, prompt: onMac ? "Threads and messages" : "Threads, projects, agents, branches")
            .navigationDestination(for: String.self) { ThreadView(threadID: $0) }
            // Typed: the Mac is asked once the typing pauses (and again once it says it can).
            .task(id: "\(onMac) \(trimmed)") { await askTheMac(trimmed) }
            .onAppear { if let q = Launch.search, query.isEmpty { query = q } }
        }
        .environment(\.openThread, OpenThreadAction { path.append($0) })
    }

    private func askTheMac(_ q: String) async {
        guard onMac, q.count >= 2 else {
            searching = false
            if q.isEmpty { found = []; foundFor = "" }
            return
        }
        try? await Task.sleep(for: .milliseconds(300))
        guard !Task.isCancelled else { return }
        searching = true
        model.search(q) { answer in
            guard q == trimmed else { return }
            searching = false
            if let answer {
                found = answer
                foundFor = q
            }
        }
    }
}

/// Under a result: the message that matched, its words in bold; a sub-agent says whose it is.
private struct SearchHitLine: View {
    var result: SearchResult
    var parent: String?

    var body: some View {
        if result.snippet != nil || result.thread.parentId != nil {
            VStack(alignment: .leading, spacing: 2) {
                if result.thread.parentId != nil {
                    HStack(spacing: 5) {
                        Image(systemName: "person.2")
                        Text(parent.map { "Sub-agent of \($0)" } ?? "Sub-agent")
                    }
                    .font(.ui(.caption)).foregroundStyle(Trek.muted).lineLimit(1)
                }
                if let snippet = result.snippet {
                    Text(Self.highlighted(snippet, result.ranges ?? []))
                        .font(.ui(.footnote))
                        .foregroundStyle(Trek.muted)
                        .lineLimit(2)
                }
            }
        }
    }

    /// `text` with `ranges` (in Unicode scalars, as the Mac counts) set in the text's own colour.
    static func highlighted(_ text: String, _ ranges: [[Int]]) -> AttributedString {
        var out = AttributedString(text)
        let scalars = text.unicodeScalars
        let count = scalars.count
        for r in ranges where r.count == 2 && 0 <= r[0] && r[0] < r[1] && r[1] <= count {
            let start = scalars.index(scalars.startIndex, offsetBy: r[0])
            let end = scalars.index(start, offsetBy: r[1] - r[0])
            guard let lo = AttributedString.Index(start, within: out), let hi = AttributedString.Index(end, within: out) else { continue }
            out[lo..<hi].foregroundColor = Trek.foreground
            out[lo..<hi].inlinePresentationIntent = .stronglyEmphasized
        }
        return out
    }
}
