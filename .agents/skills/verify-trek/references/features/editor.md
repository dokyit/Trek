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
