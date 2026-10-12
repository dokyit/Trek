# Basecamp and Notes

## What it does

Basecamp summarizes today, this week, or all work and highlights items ready for review. Notes is a persistent Markdown scratchpad with editing history, links and pictures.

## Reach it as a user

Choose **Basecamp** or **Notes** at the top of the sidebar (⌘⇧H and ⌘⇧J). In Basecamp choose Today, Week, or All and mark items read. Type directly in Notes.

Pictures go into a note by pasting (⌘V with a screenshot or image files on the clipboard), dropping image files on the note, or the toolbar's **Add image** (image-plus icon, after the colour palette). PNG, JPEG, GIF, WebP and SVG are kept as they are; HEIC, TIFF and BMP become a PNG copy (macOS `sips`). Each is stored in the notes folder's `attachments/` under a content hash and linked as `![alt](attachments/<hash>.<ext>)` on a line of its own at the cursor. Split and Preview draw them fit to the preview's width (a 144-dpi retina screenshot at its on-screen size; SVGs at the window's scale); Write shows a strip of tiles under the text. A click opens Trek's image viewer (←/→ through the note's pictures). `https:` images (fetched by Trek for notes only; GPUI's desktop default has no HTTP client, and answers still don't load web images) and paths elsewhere on disk show too; a missing or unreadable one is a dashed placeholder saying so. Deleting a note moves it to `Deleted/` and its pictures to `Deleted/attachments/` (copied instead when another note still shows them).

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open basecamp
./.agents/skills/verify-trek/scripts/trek-dev control range week
./.agents/skills/verify-trek/scripts/trek-dev screenshot basecamp-week
./.agents/skills/verify-trek/scripts/trek-dev open notes
./.agents/skills/verify-trek/scripts/trek-dev screenshot notes
```

The CLI can route and change Basecamp’s range. Notes take raw shot-driver commands (`control` passes them through): `note text <markdown>` sets the note on screen (`\n` for new lines; starts a note when there is none), `note image <abs-path>…` adds image files at the cursor as a drop does, and `note mode write|split|preview` switches the view.

```sh
./.agents/skills/verify-trek/scripts/trek-dev control note text "# Trip\nThe whiteboard:\n"
./.agents/skills/verify-trek/scripts/trek-dev control note image /tmp/whiteboard.png
./.agents/skills/verify-trek/scripts/trek-dev control note mode preview
./.agents/skills/verify-trek/scripts/trek-dev screenshot notes-image-preview
```

## Code

`crates/trek-app/src/basecamp.rs`, `notes.rs` (adding pictures: paste, drop, picker), `note_images.rs` (drawing them: preview plugin, Write strip, placeholders), `md.rs` (`md::note`), `sidebar.rs`; aggregation in `crates/trek-core/src/basecamp.rs`, notes storage and images (`add_image`, `images`, `delete`) in `crates/trek-core/src/notes.rs`, paths in `paths.rs`. The phone gets a note's markdown as is (image links show as text there; no image data is sent).

## Verify

Basecamp headings/counts respond to range changes and open the selected work. Notes renders, accepts Markdown, supports undo/redo, and persists in the active data profile. A note with a picture shows it in Split and Preview (fit to width, proportions kept) and as a tile in Write, in both themes; a link to a missing file shows the placeholder.
