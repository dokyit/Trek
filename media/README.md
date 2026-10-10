# Capture manifests

Deterministic marketing-media capture. One command — or an agent — runs a suite; no real
agents, credentials, data or clicking are involved (a shots build is isolated and only the
mock agent runs).

```sh
script/capture.sh list                    # suites
script/capture.sh check                   # lint manifests without launching
script/capture.sh mac/readme              # macOS suite → dist/media/readme/
script/capture.sh ios/store               # iOS suite   → dist/media/store/
```

On Windows the same driver is `script/capture.ps1` (`pwsh script/capture.ps1 mac/parity`); iOS
suites stay macOS-only.

Knobs (env): `BUILD=0` skip the build · `SIZE=1280x820` window points · `FPS=15` video frame
rate · `OUT=dist/media` · `KEEP=1` keep the scratch dir · `TREK_SHOT_UNDER=<png>` backdrop under
glass · `SIM="iPhone 17 Pro Max"` for App Store-sized iOS shots (the default iPhone 18 Pro is
docs-size, not a store size).

## mac/`*.cmds`

Literally the `shots.rs` command file (`crates/trek-app/src/shots.rs` documents every verb);
`#` lines are comments. Highlights:

- `send <prompt>` drives the mock agent; pick scripts by keyword (`tools`, `permission`,
  `plan`, `mock:stream 8s`, `mock:consult`, `error` …).
- `wait idle [cap]` waits for turns to end; `wait permission|question|plan [cap]` waits for a
  card to appear — use it *before* shooting a card, and remember such turns stay open, so they
  break a later `wait idle`.
- `record <name> <ms> [fps]` starts frame capture; the `send`/`route` lines after it are what
  gets filmed; `record wait` finishes it (`<name>.ffconcat` + `<name>.frames/`).
- `pace 0` plays scripts instantly (still shots), `pace 1` is demo speed (clips).
- `shot <name>` writes a PNG; `quit` exits cleanly.

## ios/`*.list`

One artifact per line: `NAME [record=SECS] [wait=N] [demo=0|1|live] -- <launch args>`.

Stills go through `ios/scripts/shoot.sh`, `record=` through `ios/scripts/record.sh`
(`xcrun simctl io recordVideo`). Launch args are the app's own (`-TrekDemo YES`, `-TrekOpen`,
`-TrekAppearance light|dark`, `-TrekTab`, `-TrekSheet`, `-TrekLink`, `-TrekPair`).

Output lands in `OUT/<suite>/` (gitignored `dist/` by default); copy keepers into
`ios/screenshots/` or `docs/media/` to commit them.
