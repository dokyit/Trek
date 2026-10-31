# Study

## What it does

Study is a screen under Notes for learning anything with an agent as a one-to-one tutor. A **Learn** session probes what the learner already knows with multiple-choice questions, plans the shortest path to their goal (a `trek-viz` flow they approve), then teaches one step at a time: predict, explain, check. A **Practice** session poses problems one at a time that adapt to what the learner's working shows, with minimal hints. Every question is a `trek-html` card (the guide's one card, `study::card_with`): where the learner is in the plan as a progress bar ("Step 3 of 8"), the prompt, options picked by click or A–D / 1–4, a thinking box, how sure they are (Guessing, Unsure, Fairly sure, Sure) and "I don't know"; ⌘↩ (Ctrl+Enter on Windows) sends, and it posts the answer as the learner's next message (`trek.send`), then shows what was sent. It has no frame of its own inside Trek's artifact card. The tutor keeps a concept map per topic (claims and the links between them) that later sessions build on.

A session is an ordinary thread, in no project, that follows the guide Trek ships (`crates/trek-core/skills/study/SKILL.md`, written to the data folder's `skills/study/`), with the composer's agent and model (or the default agent when that one can't work with files). It works in the study folder, `~/Trek/Study` (the data folder's `study/` under `TREK_DATA_DIR` or tests): `learner.md` (notes about the learner), `map/<slug>.md` (concept maps: `## Claims`, `## Links`, `## Practice`), `sources/` (the learner's material), and `sessions.json` (which threads are sessions; git-ignored). The folder is a git repository whose first commit holds `learner.md` and `.gitignore`, so a session's changes are only the session's.

## Reach it as a user

Choose **Study** under Basecamp and Notes in the sidebar (⌘⇧U), or "Study" in ⌘K. The starter is the main composer (ids `attach`, `send`, `dictate`), so everything a draft takes works here: the + menu's files and photos, drag and drop, paste, `@` files, and the mic. Choose **Learn** or **Practice** in the segmented control above it (`study-kind`; the choice is kept, and the line beside it and the placeholder say what that kind does), type or dictate what to learn, press Return. Under the composer (`study-context`): **Files or folders** (`study-add-files`, the + menu's picker), **A note** (`study-add-note`, a menu of Trek notes; a chosen one shows as a chip, `study-note-<n>`) and **Talk** (`study-talk`, the composer's voice mode; only where the mic shows). Files attached (named `@path` in the text, or images) and notes are copied into the study folder's `sources/<topic-slug>/` (a note under its title) and listed at the end of the first message for the tutor to read; folders named `@path` are listed to read where they are, and `https://` links in the text as pages to read (both left out of the title). **From your study folder** (`study-library`) has earlier maps (`study-use-map-<n>`) and files already in `sources/` (`study-source-<n>`) to tick for the session: they're read where they are; past 8 chips, `study-library-more` shows the rest. The project, worktree and plan controls don't show: a session has none. **Your maps** (`study-maps`) lists each topic's map as a row (`study-map-<n>`: ideas, links, problems practised, when last studied): click it to open the map in the editor, **Learn more** (`study-map-learn-<n>`) or **Practice** (`study-map-practice-<n>`) to start a session on it. **Sessions** lists past sessions (`study-session-<thread id>`); **Study folder** (`study-folder`) opens the folder in the file manager. Until there's a map or a session, **How a session goes** (`study-how`) lists the chosen kind's steps.

In a session's thread, the first message shows as what was asked (`study-intro`: the kind, the topic, and chips for the files, maps, folders and pages that came with it), not as the instructions Trek wrote the tutor. The composer there has no branch, changes or plan chips, and its placeholder invites thinking out loud.

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

The mock agent plays the tutor with the guide's own card (it reads the guide at the path the first message names): a session opens with a probe question (Practice: a problem); a card's answer gets the plan as a flow and the first step's prediction card ("Step 1 of 4"). To see maps listed, write a file into `<data_dir>/study/map/` (`status` prints `data_dir`) and open Study again; a file in `<data_dir>/study/sources/` shows as a chip. Start the app with `TREK_DICTATE=1` in the environment to see the mic and **Talk** in an unbundled build.

## Code

`crates/trek-app/src/study.rs` (the screen), `crates/trek-app/src/workspace/study.rs` (`start_study_with`, `StudyDraft`: the kind and the ticked sources), `crates/trek-app/src/workspace.rs` (`send` on `Route::Study`, `scope_is_draft`), `crates/trek-app/src/composer.rs` (the Study and session placeholders, the hidden chips), `crates/trek-app/src/thread_view.rs` (the first message as `study::intro_bubble`), `crates/trek-core/src/study.rs` (folder, maps, sessions, sources, first message and `intro` reading it back, `card_with`), `crates/trek-core/src/media.rs` (`mentioned_files`, `mentioned_dirs`), `crates/trek-core/skills/study/SKILL.md` (the method and the question card), `crates/trek-agents/src/mock.rs` (`Tutor`), `sidebar.rs`, `keys.rs` (`OpenStudy`), `command_palette.rs`. Tests: `crates/trek-app/tests/study.rs`, `trek-core` `study::tests`, the mock's parse test.

## Verify

Study opens from the sidebar and ⌘⇧U with the composer focused and its Study placeholder. Return opens a thread titled "Learn · <topic>" (Practice chosen: "Practice · <topic>") working in the study folder, with nothing in that folder shown as changed beforehand; the composer is empty afterwards. A file attached is in `sources/<topic-slug>/` and named in the first message. A map file in `map/` is a row with its counts, and Practice on it opens "Practice · <map title>". The session's first message is the intro, not the guide's path; the mock's card shows with no second frame, its options and sure-ness pick, and its answer comes back as "Question / Answer / Confidence / Working". Check both themes.
