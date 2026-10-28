# Editor and deep links

## What it does

A project file opens in the in-app editor (center surface, `Route::Editor`): syntax-highlighted
by extension, line-numbered, searchable (the editor's own ⌘F), dirty-marked, and saved back with
⌘S or the header's Save button. Explorer preview headers have an "Edit in Trek" (FilePen) button.

`trek://` links land in it: `trek://edit?path=…&line=…` opens the file at the line;
`trek://ask?path=…&line=…&end=…&selection=…` drafts a thread with the selection quoted into the
composer. The `trek` URL scheme is registered by the bundle (CFBundleURLTypes).

## Reach it as a user

Explorer → select a file → preview header → "Edit in Trek". Or `open "trek://edit?path=/abs/file&line=42"`
from anywhere once the app registers the scheme.

## Reach it in a capture

`trek-dev control editor /abs/path/file.ext [line]` opens the file in the editor without a
pointer. Files larger than 2 MB or unreadable open read-only with their reason in the header.

# Editor mode

## What it does

The title bar's **Agents | Editor** switch (⌥⌘E; ids `mode-agents`, `mode-editor`) turns the main
window into a code editor of its own (`crates/trek-app/src/ide/`): the primary side bar's
Explorer, Search and Source Control (`ide-view-explorer`, `ide-view-search`, `ide-view-scm`),
editor tabs, the bottom panel (Problems, Output, Terminal, Tasks), the status bar (`ide-status`:
branch, problems, caret, language) and the AI side bar on the right.

The AI side bar keeps **editor chats**: chats about the code open there, with their own tabs and
history (`Thread::editor_chat`, saved with the thread). They never show in Agents mode: not in the
sidebar, inbox, Dock badge, Basecamp's lists and thread counts, search, the command palette or the
phone. Agents threads never show in the editor: its history lists the folder's editor chats only,
switching into the editor brings the thread's folder but not the thread, and the editor has no
Agents view, Explorer agents section or status-bar agent count. Asked to show an editor chat (an
alert, a link), the window goes to the editor; asked for an Agents thread from the editor, it goes
to Agents. Threads made from the editor before editor chats existed stay Agents threads, and their
saved tabs don't come back on the side bar.

Its parts: chat tabs (`ai-tab`, `ai-tab-close`, `ai-tabs-menu`), + (`ai-new-chat`, ⌘N), history
(`ai-history`), ⋯ (`ai-more`: rename, open in a new window, fork, copy, archive), the new chat's
note (`ai-draft`), the transcript, the pending-changes bar (Keep / Undo / Review), and the input
(`ai-input`) with context chips, Agent/Plan/Ask and the model. ⌘K in a file asks for an inline edit
in that file's own editor chat ("Inline edits · name").

## Reach it in a capture

```sh
./.agents/skills/verify-trek/scripts/trek-dev control ide            # toggle Editor mode
./.agents/skills/verify-trek/scripts/trek-dev control editor root /abs/folder
./.agents/skills/verify-trek/scripts/trek-dev control click ai-input
./.agents/skills/verify-trek/scripts/trek-dev control type "mock:write notes.md"
./.agents/skills/verify-trek/scripts/trek-dev control key enter
./.agents/skills/verify-trek/scripts/trek-dev control click ai-history
```

`control editor view explorer|search|scm`, `control editor panel terminal|problems|output|tasks|off`
and the rest are listed in `shots.rs` (`verb == "editor"`).

## Verify

An editor chat sent from the side bar answers there and is listed by its history, but not in the
Agents sidebar or search; an Agents thread is not in the side bar's history; the activity row has
three views and no agents count shows in the Explorer or the status bar.
