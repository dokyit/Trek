# Basecamp and Notes

## What it does

Basecamp summarizes today, this week, or all work and highlights items ready for review. Notes is a persistent Markdown scratchpad with editing history and links.

## Reach it as a user

Choose **Basecamp** or **Notes** at the top of the sidebar (⌘⇧H and ⌘⇧J). In Basecamp choose Today, Week, or All and mark items read. Type directly in Notes.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open basecamp
./.agents/skills/verify-trek/scripts/trek-dev control range week
./.agents/skills/verify-trek/scripts/trek-dev screenshot basecamp-week
./.agents/skills/verify-trek/scripts/trek-dev open notes
./.agents/skills/verify-trek/scripts/trek-dev screenshot notes
```

The CLI can route and change Basecamp’s range; it cannot type into Notes.

## Code

`crates/trek-app/src/basecamp.rs`, `notes.rs`, `sidebar.rs`; aggregation in `crates/trek-core/src/basecamp.rs` and notes paths in `paths.rs`.

## Verify

Basecamp headings/counts respond to range changes and open the selected work. Notes renders, accepts Markdown, supports undo/redo, and persists in the active data profile.

