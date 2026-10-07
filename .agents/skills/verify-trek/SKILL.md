---
name: verify-trek
description: Drive, debug, and verify Trek in a disposable mock-only profile. Use it to check any Trek change works before calling it done.
metadata:
  trek: verification
  cli: ./.agents/skills/verify-trek/scripts/trek-dev
---

# Verify Trek

Run the CLI from the checkout or worktree being verified. Its state is keyed to `git rev-parse --show-toplevel`, so it builds that checkout. JSON is the default output; add `--human` for readable text.

The driver always launches with `TREK_SHOT_DIR`, a throwaway `TREK_DATA_DIR`, imports and notifications off, and `direct:mock` as the only agent. Never replace those safeguards or point the driver at a normal Trek process: imported threads can resume real sessions.

## Workflow

1. Read the relevant entry in the [Feature Map](references/features/README.md).
2. Check the machine, start the isolated app, and reach the feature.
3. Capture the result and inspect logs when behavior is wrong.
4. Run `check` before calling the change verified; stop or reset the profile afterward.

```sh
./.agents/skills/verify-trek/scripts/trek-dev preflight
./.agents/skills/verify-trek/scripts/trek-dev start
./.agents/skills/verify-trek/scripts/trek-dev status
./.agents/skills/verify-trek/scripts/trek-dev open settings:appearance
./.agents/skills/verify-trek/scripts/trek-dev project .
./.agents/skills/verify-trek/scripts/trek-dev send "Show a short streamed answer"
./.agents/skills/verify-trek/scripts/trek-dev control theme paper
./.agents/skills/verify-trek/scripts/trek-dev screenshot appearance --output /tmp/trek-appearance.png
./.agents/skills/verify-trek/scripts/trek-dev logs --lines 120 --grep error
./.agents/skills/verify-trek/scripts/trek-dev check
./.agents/skills/verify-trek/scripts/trek-dev stop
./.agents/skills/verify-trek/scripts/trek-dev reset --dry-run
./.agents/skills/verify-trek/scripts/trek-dev reset --yes
```

Use `--help` after any command for its routes, arguments, dry-run behavior, and examples. `check --quick` is for iteration only: the full check parses shell scripts, runs the same Rust package tests as CI, builds the `shots` feature, then launches isolated Trek and captures Basecamp and General Settings.

## Development environment

Trek development requires Apple-silicon macOS 13+, stable Rust, full Xcode, and Xcode's Metal Toolchain. On Xcode 26+, install a missing Metal compiler once with:

```sh
xcodebuild -downloadComponent MetalToolchain
```

A fresh checkout needs no secrets, database seed, test account, or vendor-agent login for verification. Cargo fetches Rust dependencies. The CLI writes its disposable settings, SQLite data, logs, command files, and screenshots beneath the macOS temp directory shown by `status`; it never uses `~/Library/Application Support/dev.trek.Trek` or the Keychain.

For ordinary manual development, `cargo run -p trek-app` uses the normal profile and may discover real agents and imports. Do not use that mode for automated verification.

