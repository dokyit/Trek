# Study

## What it does

Study is a screen under Notes for learning anything with an agent as a one-to-one tutor. A **Learn** session probes what the learner already knows with multiple-choice questions, plans the shortest path to their goal (a `trek-viz` flow they approve), then teaches one step at a time: predict, explain, check. A **Practice** session poses problems one at a time that adapt to what the learner's working shows, with minimal hints. Every question is a `trek-html` card with a working box (think out loud), a confidence slider and "I don't know"; it posts the answer as the learner's next message (`trek.send`). The tutor keeps a concept map per topic (claims and the links between them) that later sessions build on.

A session is an ordinary thread, in no project, that follows the guide Trek ships (`crates/trek-core/skills/study/SKILL.md`, written to the data folder's `skills/study/`), with the composer's agent and model (or the default agent when that one can't work with files). It works in the study folder, `~/Trek/Study` (the data folder's `study/` under `TREK_DATA_DIR` or tests): `learner.md` (notes about the learner), `map/<slug>.md` (concept maps: `## Claims`, `## Links`, `## Practice`), `sources/` (the learner's material), and `sessions.json` (which threads are sessions; git-ignored). The folder is a git repository whose first commit holds `learner.md` and `.gitignore`, so a session's changes are only the session's.

## Reach it as a user

Choose **Study** under Basecamp and Notes in the sidebar (⌘⇧U), or "Study" in ⌘K. The starter is the main composer (ids `attach`, `send`, `dictate`), so everything a draft takes works here: the + menu's files and photos, drag and drop, paste, `@` files, and the mic. Choose **Learn** or **Practice** in the segmented control above it (`study-kind`; the choice is kept), type or dictate what to learn, press Return. Files attached (named `@path` in the text, or images) are copied into the study folder's `sources/<topic-slug>/` and listed at the end of the first message for the tutor to read; files already in `sources/` show as chips under the composer (`study-source-<n>`) to tick for the session. The project, worktree and plan controls don't show: a session has none. **Your maps** shows each topic's map as a card (`study-map-<n>`: ideas, links, problems practised, when last studied): click it to open the map in the editor, **Learn more** (`study-map-learn-<n>`) or **Practice** (`study-map-practice-<n>`) to start a session on it. **Sessions** lists past sessions (`study-session-<thread id>`); **Study folder** (`study-folder`) opens the folder in the file manager. **How it works** is a row of four tiles.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open study
./.agents/skills/verify-trek/scripts/trek-dev screenshot study
./.agents/skills/verify-trek/scripts/trek-dev control click study-kind#1      # Practice
./.agents/skills/verify-trek/scripts/trek-dev control type "the transformer architecture, heavy math @/path/to/notes.pdf"
./.agents/skills/verify-trek/scripts/trek-dev control key enter
./.agents/skills/verify-trek/scripts/trek-dev screenshot study-session
./.agents/skills/verify-trek/scripts/trek-dev control dictate-demo "the chain rule"   # voice mode, no mic
```

The mock agent answers with its canned reply; the cards themselves need a real agent. To see maps listed, write a file into `<data_dir>/study/map/` (`status` prints `data_dir`) and open Study again; a file in `<data_dir>/study/sources/` shows as a chip.

## Code

`crates/trek-app/src/study.rs` (the screen), `crates/trek-app/src/workspace/study.rs` (`start_study_with`, `StudyDraft`: the kind and the ticked sources), `crates/trek-app/src/workspace.rs` (`send` on `Route::Study`, `scope_is_draft`), `crates/trek-app/src/composer.rs` (the Study placeholder, the hidden chips), `crates/trek-core/src/study.rs` (folder, maps, sessions, sources, first message), `crates/trek-core/src/media.rs` (`mentioned_files`), `crates/trek-core/skills/study/SKILL.md` (the method and the question card), `sidebar.rs`, `keys.rs` (`OpenStudy`), `command_palette.rs`. Tests: `crates/trek-app/tests/study.rs`, `trek-core` `study::tests`.

## Verify

Study opens from the sidebar and ⌘⇧U with the composer focused and its Study placeholder. Return opens a thread titled "Learn · <topic>" (Practice chosen: "Practice · <topic>") working in the study folder, with nothing in that folder shown as changed beforehand; the composer is empty afterwards. A file attached is in `sources/<topic-slug>/` and named in the first message. A map file in `map/` is a card with its counts, and Practice on it opens "Practice · <map title>". Check both themes.
