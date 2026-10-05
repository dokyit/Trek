import SwiftUI
import UIKit

/// The Mac's notes, newest first: open one to read or write it, swipe to delete it (the Mac keeps
/// it in the notes folder's Deleted), + for a new one.
struct NotesView: View {
    @Environment(AppModel.self) private var model
    @Binding var path: [String]
    @State private var query = ""
    /// Notes made here this visit: left empty, they go again.
    @State private var made: Set<String> = []

    private var shown: [NoteSummary] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return model.notes }
        return model.notes.filter { $0.title.lowercased().contains(q) || $0.preview.lowercased().contains(q) }
    }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                ForEach(shown) { note in
                    NavigationLink(value: note.id) { NoteRow(note: note) }
                        .navigationLinkIndicatorVisibility(.hidden)
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                        .listRowInsets(EdgeInsets(top: 10, leading: 20, bottom: 10, trailing: 18))
                        .swipeActions {
                            Button("Delete", systemImage: "trash", role: .destructive) { model.deleteNote(note.id) }
                        }
                        .contextMenu {
                            Button("Delete", systemImage: "trash", role: .destructive) { model.deleteNote(note.id) }
                        }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(RidgeBackdrop(height: 300))
            .overlay {
                if shown.isEmpty {
                    if query.isEmpty {
                        ContentUnavailableView {
                            Label("No notes yet", systemImage: "note.text")
                        } description: {
                            Text("Jot something down here or in Notes on your Mac.")
                        } actions: {
                            Button("New note", systemImage: "square.and.pencil", action: newNote)
                                .buttonStyle(.glass)
                        }
                    } else {
                        ContentUnavailableView.search(text: query)
                    }
                }
            }
            .navigationTitle("Notes")
            .searchable(text: $query, placement: .navigationBarDrawer, prompt: "Search notes")
            .refreshable { model.loadNotes() }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("New note", systemImage: "square.and.pencil", action: newNote)
                }
            }
            .task { model.loadNotes() }
            .navigationDestination(for: String.self) { id in
                NoteEditor(noteID: id, isNew: made.contains(id))
            }
        }
    }

    private func newNote() {
        model.createNote("") { note in
            made.insert(note.id)
            path.append(note.id)
        }
    }
}

private struct NoteRow: View {
    var note: NoteSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text(note.title).scaledFont(17, weight: .semibold).lineLimit(1)
                Spacer(minLength: 8)
                Text(When.short(note.modified)).font(.subheadline.monospacedDigit()).foregroundStyle(Trek.muted.opacity(0.8))
            }
            Text(note.preview.isEmpty ? "No more text" : note.preview)
                .font(.subheadline)
                .foregroundStyle(Trek.muted)
                .lineLimit(2)
        }
        .accessibilityElement(children: .combine)
    }
}

// MARK: Editor

/// A note, to read (as the Mac's preview draws it, boxes tickable) or to write (its markdown, with
/// the Mac's formatting on a bar over the keyboard). Saved a moment after typing stops. Colour,
/// underline and anything else the bar doesn't offer stays in the text as it was.
///
/// If the note changed on the Mac since it was opened, the Mac refuses the save: the user then
/// keeps their version (saved over the Mac's) or takes the Mac's.
struct NoteEditor: View {
    var noteID: String
    /// Made here just now: opens for writing, and goes again if left empty.
    var isNew = false

    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var loaded = false
    @State private var writing = false
    @State private var selection: TextSelection?
    /// The text the Mac has, as far as the phone knows (last opened or saved).
    @State private var saved = ""
    @State private var saving = false
    @State private var pending: Task<Void, Never>?
    /// The Mac's version, when it refused a save because it had changed.
    @State private var theirs: Note?
    @State private var confirmDelete = false
    @State private var deleted = false
    @FocusState private var focused: Bool
    #if DEBUG
    @State private var fakedConflict = false
    #endif

    private var status: String {
        if saving { return "Saving…" }
        if text != saved { return "Edited" }
        if let modified = model.openNotes[noteID]?.modified { return "Saved · \(Resets.clock(modified))" }
        return ""
    }

    var body: some View {
        Group {
            if !loaded {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if writing {
                TextEditor(text: $text, selection: $selection)
                    .focused($focused)
                    .scaledFont(17)
                    .scrollContentBackground(.hidden)
                    .padding(.horizontal, 14)
                    .scrollDismissesKeyboard(.interactively)
            } else {
                ScrollView {
                    Group {
                        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            Text("Empty note. Tap to write.").foregroundStyle(Trek.muted)
                        } else {
                            NoteMarkdown(text: text, toggle: tick)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 12)
                }
                .contentShape(Rectangle())
                .onTapGesture { startWriting() }
            }
        }
        .safeAreaInset(edge: .bottom) {
            // Over the keyboard while writing (and at the foot of the screen with a hardware one).
            if writing {
                FormatBar(apply: format)
                    .padding(.vertical, 4)
                    .glassEffect(.regular, in: .capsule)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 8)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(.snappy, value: writing)
        .background(Trek.background)
        .navigationTitle(NoteEditing.title(of: text))
        .navigationSubtitle(status)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarVisibility(.hidden, for: .tabBar)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                if writing {
                    Button("Done", systemImage: "checkmark") { stopWriting() }
                        .buttonStyle(.glassProminent)
                        .tint(Trek.foreground)
                } else {
                    Button("Edit", systemImage: "pencil") { startWriting() }
                        .disabled(!loaded)
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu("More", systemImage: "ellipsis") {
                    Button("Copy text", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = text
                        model.show("Copied")
                    }
                    Button("Delete note", systemImage: "trash", role: .destructive) { confirmDelete = true }
                }
            }
        }
        .confirmationDialog("Delete this note?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive) {
                deleted = true
                pending?.cancel()
                model.deleteNote(noteID)
                dismiss()
            }
        } message: {
            Text("Your Mac keeps it in the notes folder's Deleted.")
        }
        .sheet(item: $theirs) { note in
            ConflictSheet(theirs: note, keepMine: keepMine, takeTheirs: { take(note) })
        }
        .task { open() }
        .onChange(of: text) { old, new in changed(old, new) }
        .onDisappear(perform: leave)
    }

    // MARK: Loading and saving

    private func open() {
        if let note = model.openNotes[noteID] { load(note) }
        model.openNote(noteID) { note in
            // Only while nothing's been typed: the text on screen is the user's.
            if text == saved { load(note) }
        }
    }

    private func load(_ note: Note) {
        text = note.body
        saved = note.body
        if !loaded {
            loaded = true
            if isNew || note.body.isEmpty { startWriting() }
        }
    }

    private func changed(_ old: String, _ new: String) {
        // Return after a list item carries the list on (or ends it, after an empty one).
        if writing, new.count == old.count + 1, let at = insertedNewline(old, new), let edit = NoteEditing.newline(new, at: at) {
            apply(edit)
            return
        }
        scheduleSave()
    }

    private func scheduleSave() {
        pending?.cancel()
        guard text != saved, loaded else { return }
        pending = Task {
            try? await Task.sleep(for: .seconds(1))
            if !Task.isCancelled { save() }
        }
    }

    /// One save at a time: each carries the version it was made from, so a second one sent
    /// before the first came back would be refused as a change on the Mac.
    private func save() {
        guard loaded, !deleted, !saving, theirs == nil, text != saved else { return }
        let body = text
        saving = true
        #if DEBUG
        // Screenshots of the conflict: the Mac "changed" the note since it was opened.
        if UserDefaults.standard.bool(forKey: "TrekNoteConflict"), !fakedConflict {
            fakedConflict = true
            model.openNotes[noteID]?.modified -= 1
        }
        #endif
        model.saveNote(noteID, body: body, done: { _ in
            saving = false
            saved = body
            if text != saved { scheduleSave() }
        }, conflict: {
            saving = false
            model.openNote(noteID) { theirs = $0 }
        })
        // Other failures say so in a toast and don't answer here: let the next change try again.
        Task {
            try? await Task.sleep(for: .seconds(10))
            if saving, saved != body { saving = false }
        }
    }

    private func keepMine() {
        theirs = nil
        // The Mac's version is the one opened now, so this save goes over it.
        save()
    }

    private func take(_ note: Note) {
        theirs = nil
        saved = note.body
        text = note.body
    }

    private func leave() {
        pending?.cancel()
        guard !deleted else { return }
        if isNew, text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            model.deleteNote(noteID)
        } else {
            save()
        }
    }

    // MARK: Writing

    private func startWriting() {
        writing = true
        focused = true
    }

    private func stopWriting() {
        focused = false
        writing = false
        pending?.cancel()
        save()
    }

    private func tick(_ line: Int) {
        if let ticked = NoteEditing.toggleCheck(text, line: line) {
            text = ticked
            UISelectionFeedbackGenerator().selectionChanged()
        }
    }

    /// The selection, in characters (the end of the text when there is none).
    private var selected: Range<Int> {
        let end = text.count
        guard let selection, case .selection(let r) = selection.indices,
              r.lowerBound >= text.startIndex, r.upperBound <= text.endIndex else { return end..<end }
        let lo = text.distance(from: text.startIndex, to: r.lowerBound)
        return lo..<(lo + text.distance(from: r.lowerBound, to: r.upperBound))
    }

    private func format(_ action: FormatBar.Action) {
        let edit: NoteEditing.Edit = switch action {
        case .wrap(let open, let close): NoteEditing.wrap(text, selected, open: open, close: close)
        case .block(let block): NoteEditing.toggle(text, selected, block)
        case .color(let css, let highlight): NoteEditing.color(text, selected, css: css, highlight: highlight)
        }
        apply(edit)
    }

    private func apply(_ edit: NoteEditing.Edit) {
        text = edit.text
        let lo = text.index(text.startIndex, offsetBy: min(edit.selection.lowerBound, text.count))
        let hi = text.index(text.startIndex, offsetBy: min(edit.selection.upperBound, text.count))
        selection = TextSelection(range: lo..<hi)
    }

    /// Where a single new line went in, as the offset just after it.
    private func insertedNewline(_ old: String, _ new: String) -> Int? {
        let a = Array(old), b = Array(new)
        var i = 0
        while i < a.count, a[i] == b[i] { i += 1 }
        guard b[i] == "\n", a[i...] == b[(i + 1)...] else { return nil }
        return i + 1
    }
}

/// The Mac's formatting, on a glass bar over the keyboard: bold, italics, underline, strikethrough, headings,
/// lists, quote, code, and text colour and highlights.
private struct FormatBar: View {
    enum Action {
        case wrap(String, String)
        case block(NoteEditing.Block)
        case color(String, highlight: Bool)
    }

    var apply: (Action) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 2) {
                tool("bold", "Bold") { apply(.wrap("**", "**")) }
                tool("italic", "Italic") { apply(.wrap("*", "*")) }
                tool("underline", "Underline") { apply(.wrap("<u>", "</u>")) }
                tool("strikethrough", "Strikethrough") { apply(.wrap("~~", "~~")) }
                divider
                Menu {
                    Button("Heading") { apply(.block(.heading(1))) }
                    Button("Subheading") { apply(.block(.heading(2))) }
                } label: {
                    icon("textformat.size")
                }
                .accessibilityLabel("Heading")
                tool("list.bullet", "Bullet list") { apply(.block(.bullets)) }
                tool("list.number", "Numbered list") { apply(.block(.numbers)) }
                tool("checklist", "Checklist") { apply(.block(.checklist)) }
                tool("text.quote", "Quote") { apply(.block(.quote)) }
                tool("chevron.left.forwardslash.chevron.right", "Code") { apply(.wrap("`", "`")) }
                divider
                Menu {
                    Section("Text colour") {
                        ForEach(NoteEditing.colors, id: \.name) { c in
                            Button { apply(.color(c.text, highlight: false)) } label: { swatch(c.name, c.text, symbol: "textformat") }
                        }
                    }
                    Section("Highlight") {
                        ForEach(NoteEditing.colors, id: \.name) { c in
                            Button { apply(.color(c.mark, highlight: true)) } label: { swatch(c.name, c.text, symbol: "highlighter") }
                        }
                    }
                } label: {
                    icon("paintpalette")
                }
                .accessibilityLabel("Colour and highlight")
            }
            .padding(.horizontal, 4)
        }
    }

    private var divider: some View {
        Rectangle().fill(Trek.foreground.opacity(0.12)).frame(width: 1, height: 20).padding(.horizontal, 4)
    }

    private func icon(_ symbol: String) -> some View {
        Image(systemName: symbol)
            .font(.system(size: 16, weight: .medium))
            .frame(width: 38, height: 34)
            .contentShape(Rectangle())
    }

    private func tool(_ symbol: String, _ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { icon(symbol) }
            .buttonStyle(.plain)
            .accessibilityLabel(label)
    }

    /// A menu item in its colour (menus draw template images in one colour, so it's drawn as is).
    private func swatch(_ name: String, _ css: String, symbol: String) -> some View {
        let color = NoteInline.color(css).map { UIColor($0) } ?? .label
        let image = UIImage(systemName: symbol)?.withTintColor(color, renderingMode: .alwaysOriginal) ?? UIImage()
        return Label { Text(name) } icon: { Image(uiImage: image) }
    }
}

/// The note changed on the Mac since the phone opened it: keep this version (saved over the
/// Mac's) or take the Mac's.
private struct ConflictSheet: View {
    var theirs: Note
    var keepMine: () -> Void
    var takeTheirs: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 6) {
                Label("Changed on your Mac", systemImage: "exclamationmark.arrow.trianglehead.2.clockwise.rotate.90")
                    .font(.title3.weight(.semibold))
                Text("This note was edited on your Mac after you opened it here. Here's the Mac's version, \(Resets.clock(theirs.modified)):")
                    .font(.subheadline)
                    .foregroundStyle(Trek.muted)
            }
            ScrollView {
                NoteMarkdown(text: theirs.body, size: 15)
                    .padding(14)
            }
            .frame(maxHeight: .infinity)
            .background(Trek.foreground.opacity(0.04), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            VStack(spacing: 10) {
                Button(action: keepMine) {
                    Text("Keep mine").fontWeight(.semibold).foregroundStyle(Trek.background).frame(maxWidth: .infinity)
                }
                .buttonStyle(.glassProminent)
                .tint(Trek.foreground)
                Button(action: takeTheirs) {
                    Text("Use the Mac's version").frame(maxWidth: .infinity)
                }
                .buttonStyle(.glass)
            }
            .controlSize(.large)
            Text("Keep mine saves this iPhone's version over the Mac's.")
                .font(.footnote)
                .foregroundStyle(Trek.muted)
                .frame(maxWidth: .infinity)
        }
        .padding(20)
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .interactiveDismissDisabled()
    }
}
