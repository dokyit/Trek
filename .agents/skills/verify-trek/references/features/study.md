# Study

## What it does

Study is a screen under Notes for learning anything with an agent as a one-to-one tutor. A **Learn** session probes what the learner already knows with multiple-choice questions, plans the shortest path to their goal (a `trek-viz` flow they approve), then teaches one step at a time: predict, explain, check. A **Practice** session poses problems one at a time that adapt to what the learner's working shows, with minimal hints. Every question is a `trek-html` card with a working box (think out loud), a confidence slider and "I don't know"; it posts the answer as the learner's next message (`trek.send`). The tutor keeps a concept map per topic (claims and the links between them) that later sessions build on.

A session is an ordinary thread, in no project, that follows the guide Trek ships (`crates/trek-core/skills/study/SKILL.md`, written to the data folder's `skills/study/`), with the composer's agent and model (or the default agent when that one can't work with files). It works in the study folder, `~/Trek/Study` (the data folder's `study/` under `TREK_DATA_DIR` or tests): `learner.md` (notes about the learner), `map/<slug>.md` (concept maps: `## Claims`, `## Links`, `## Practice`), `sources/` (the learner's material), and `sessions.json` (which threads are sessions; git-ignored). The folder is a git repository whose first commit holds `learner.md` and `.gitignore`, so a session's changes are only the session's.

## Reach it as a user

Choose **Study** under Basecamp and Notes in the sidebar (⌘⇧U), or "Study" in ⌘K. Type what to learn and press Return or **Start learning**; **Practice** starts a practice session on the same text. **Your maps** lists each topic's map (ideas, links, problems practised): click a row to open the map in the editor, **Learn more** or **Practice** to start a session on it. **Sessions** lists past sessions; **Study folder** opens the folder in Finder.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open study
./.agents/skills/verify-trek/scripts/trek-dev screenshot study
./.agents/skills/verify-trek/scripts/trek-dev control type "the transformer architecture, heavy math"
./.agents/skills/verify-trek/scripts/trek-dev control key enter
./.agents/skills/verify-trek/scripts/trek-dev screenshot study-session
```

The mock agent answers with its canned reply; the cards themselves need a real agent. To see maps listed, write a file into `<data_dir>/study/map/` (`status` prints `data_dir`) and open Study again.

## Code

`crates/trek-app/src/study.rs` (the screen), `crates/trek-app/src/workspace/study.rs` (`start_study`), `crates/trek-core/src/study.rs` (folder, maps, sessions, first message), `crates/trek-core/skills/study/SKILL.md` (the method and the question card), `sidebar.rs`, `keys.rs` (`OpenStudy`), `command_palette.rs`. Tests: `crates/trek-app/tests/study.rs`, `trek-core` `study::tests`.

## Verify

Study opens from the sidebar and ⌘⇧U with its field focused; the buttons wake up once something is typed. Return opens a thread titled "Learn · <topic>" working in the study folder, with nothing in that folder shown as changed beforehand. A map file in `map/` is listed with its counts, and Practice on it opens "Practice · <map title>". Check both themes.
