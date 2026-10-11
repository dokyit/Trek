# Releasing Trek

Releases are GitHub releases of [dokyit/Trek](https://github.com/dokyit/Trek). Every Trek build from this
repository trusts the minisign public key in `assets/update/minisign.pub`, checks the feed of the channel
the user picked, and installs a release only if its archive matches that key.

## One-time setup (on the Mac that publishes)

1. **Tools**: `brew install minisign resvg`, `gh auth login` (with push access to the repo).
2. **Code-signing identity**: `script/signing-identity.sh`. It creates a self-signed certificate called
   "Trek Local Signing" (RSA 2048, ten years), keeps the key and certificate in
   `~/.trek-signing/codesign.p12` (mode 600; its password is in `codesign.p12.password` next to it) and
   imports it into the login keychain with only `/usr/bin/codesign` allowed to use the key. No trust
   settings are changed and no password prompt appears; codesign signs fine with an untrusted
   certificate. `script/signing-identity.sh --check` shows which identity bundles will be signed with:
   `$TREK_SIGN_IDENTITY`, else "Trek Local Signing", else "Shelf Dev" (an older self-signed identity on
   the maintainer's Mac), else ad-hoc. A real release refuses anything but Trek Local Signing unless
   `TREK_SIGN_IDENTITY` names another identity on purpose.

   Every release must be signed with the same certificate: macOS ties Accessibility and Screen Recording
   grants to it (the designated requirement is `identifier "dev.trek.Trek" and certificate leaf = H"…"`),
   so a new certificate means every user grants those permissions again. To publish from another Mac,
   import the same p12 there (`security import codesign.p12 -k login.keychain-db -P "$(cat
   codesign.p12.password)" -T /usr/bin/codesign`) instead of creating a new identity.
   With an Apple Developer ID you'd set `TREK_SIGN_IDENTITY` and add notarization; neither is done yet.
3. **Update signing key**: `~/.trek-signing/minisign.key` (mode 600), created with
   `minisign -G -p ~/.trek-signing/minisign.pub -s ~/.trek-signing/minisign.key`, which asks for a
   password. Only the public half is in the repository (`assets/update/minisign.pub`), compiled into
   every build by `trek-core/src/update.rs`.

   **Give the key a password.** Every installed Trek accepts an update signed with it, and this Mac
   runs coding agents as you: one with Full access (or a prompt injection that reaches a shell) can
   read any file you can, and file permissions and FileVault don't stop that. `release.sh` asks for
   the password at the signing step and warns when the key has none. To add one to an existing key
   without changing it: `minisign -C -s ~/.trek-signing/minisign.key` (the public key stays the
   same). Better still, sign on a machine that runs no agents, or keep the key on a hardware token.
   An older key made with `-W` has no password; `release.sh` still signs with it, after the warning.

   **Back up both files in `~/.trek-signing/`.** If the minisign secret key is lost, installed copies of
   Trek can't verify anything newer: publish a release signed with a new key, and users have to download
   that one by hand. If it leaks, rotate the same way. A fork that publishes its own releases builds with
   `TREK_UPDATE_PUBKEY=<base64 key line>` and points `feed_url` at its own releases.

## Channels

| Channel | GitHub release | Manifest URL Trek reads | Versions |
|---|---|---|---|
| Stable | `v<version>`, a normal release marked latest | `releases/latest/download/stable.json` | `0.3.0` |
| Beta | the prerelease under the fixed tag `beta` | `releases/download/beta/beta.json` | `0.3.0-beta.1` |
| Nightly | the prerelease under the fixed tag `nightly` | `releases/download/nightly/nightly.json` | `0.3.0-nightly.20261002` |

GitHub's `latest/download/` URL always resolves to the newest non-prerelease release, so stable needs no
moving parts. Beta and nightly can't use it (prereleases are never "latest"), so each lives in one
prerelease whose tag moves to the newest build and whose assets are replaced: the archive is uploaded first
and the manifest that points at it last, then older archives are deleted. Beta users also get stable
releases and nightly users get both: Trek checks every feed at or above its channel and takes the newest
version. Trek never installs an older or equal version.

Versions follow semver, which decides who gets what:

- `0.3.0-beta.2 < 0.3.0`: a stable release supersedes its betas, so beta users move to it.
- `0.3.0-beta.2 < 0.3.0-nightly.20261002` (`nightly` sorts after `beta`): nightly users stay on nightlies
  of 0.3.0 and don't step back to a 0.3.0 beta, which a nightly built later already contains. They move
  to stable once 0.3.0 itself ships.
- Switching from nightly back to stable waits for the next stable release rather than downgrading.
  Switching channels discards an update the old channel already downloaded, so it is never installed.

`release.sh` enforces this: a new version must be newer than the last release of its own channel and of
the channels its users also get (beta: stable; nightly: stable and beta), and not older than the
workspace version.

Only stable releases move the branch. `[workspace.package] version` in `Cargo.toml` is always the last
stable version; a stable release commits `Release v<version>` on the branch and tags it. A beta or nightly
is built from `HEAD` with its version set for the build only, and the exact sources it was built from
(`HEAD` plus the version bump) are recorded as a commit off the branch that its tags point at: `v<version>`
and the moving `beta` tag for a beta, only the moving `nightly` tag for a nightly (its message names the
version). Afterwards the working tree is back at the workspace version, so prereleases never pile up
version commits on `main` and a nightly never blocks the next beta.

`feed_url` in settings.toml is `https://github.com/dokyit/Trek/releases` by default (settings files that
still name the old `trek-app/trek` placeholder are migrated on load). It can also be a template with
`{channel}` (`http://127.0.0.1:8765/{channel}.json`) or one manifest URL ending in `.json`.

## Publishing

```sh
script/release.sh 0.2.0 --dry-run           # build everything, publish nothing
script/release.sh 0.2.0                      # stable
script/release.sh 0.3.0-beta.1 --channel beta
script/release.sh 0.3.0-nightly.20261002 --channel nightly
script/release.sh 0.2.1 --notes notes.md     # hand-written notes instead of the git log
```

What it does:

1. For a real release, fetches tags from `origin`. Checks the version (semver; stable has no pre-release
   suffix, beta / nightly carry theirs; newer than the channels' last releases as above), that the
   version tag doesn't exist, and — for a real release — a clean working tree, `gh` logged in and a stable
   code-signing identity (ad-hoc releases are refused).
2. Sets `[workspace.package] version` in `Cargo.toml` for the build (undone afterwards unless a stable
   release commits it).
3. Runs `script/bundle.sh` with `TREK_RELEASE_BUILD=1`: release build of `trek` and `trek-mcp`, icon,
   `Info.plist` (`CFBundleVersion` carries the full semver; the updater checks it; `TrekRelease` marks it
   as a release), code signing.
4. Packs `dist/Trek.app` without extended attributes into
   `dist/release/<version>/Trek-<version>-darwin-aarch64.app.tar.gz`, records its SHA-256, signs it with
   minisign (trusted comment `Trek <version> darwin-aarch64`; Trek refuses a signature whose trusted
   comment names another version) and verifies the signature against `assets/update/minisign.pub`, so a
   mismatched key fails here rather than on users' machines.
5. Writes `notes.md` (commit subjects since the most recent tag of the channel's line — the last stable
   tag for stable, the last version tag for beta, the `nightly` tag for nightly — without merges and
   release commits) and the manifest `<channel>.json`:

   ```json
   { "version": "0.2.0", "notes": "- …", "pub_date": "2026-10-02T20:47:56Z",
     "platforms": { "darwin-aarch64": {
       "url": "https://github.com/dokyit/Trek/releases/download/v0.2.0/Trek-0.2.0-darwin-aarch64.app.tar.gz",
       "sha256": "…", "signature": "<the .minisig file>" } } }
   ```

6. Without `--dry-run`:
   - stable: commits `Release v<version>` on the current branch, tags `v<version>`, pushes both and
     creates the GitHub release (marked latest) with the archive, its `.minisig` and `stable.json`;
   - beta / nightly: records the build as a commit off the branch, tags it (`v<version>` for a beta),
     moves the channel tag to it, pushes the tags only, and refreshes the channel's prerelease: archive
     first, manifest last, then older archives are deleted. The branch doesn't move and the version
     bump is undone.

   With `--dry-run` the version bump is undone and nothing leaves the machine; the artifacts stay in
   `dist/release/<version>/`.

Environment: `TREK_MINISIGN_KEY` (secret key path), `TREK_RELEASE_REPO` (default `dokyit/Trek`),
`TREK_SIGN_IDENTITY`, and `TREK_RELEASE_DOWNLOAD_URL` to point the manifest's archive URL somewhere else
(a staging server).

## Windows

The Mac's `release.sh` publishes first - it creates the release and its `<channel>.json`. Windows then
attaches to the same release: `.github/workflows/release-windows.yml` runs on `windows-latest` and never
creates a release itself; it fails if the tag's release doesn't exist yet. The order matters in both
directions: `release.sh` writes `<channel>.json` with only the Mac's entry, so a Mac publish (a new
beta or nightly, or a re-run of one) drops the Windows entry until the workflow runs again.

- **Stable**: publishing the `v<version>` release triggers the workflow automatically.
- **Beta / nightly**: run it by hand once the Mac's prerelease exists under the moving `beta`/`nightly`
  tag: `gh workflow run release-windows.yml -f version=0.3.0-beta.1 -f channel=beta`.

The job checks out the version's tag (`v<version>`; the moving `nightly` tag for nightlies), builds
`cargo build --release --locked -p trek-app -p trek-mcp`, smokes it with `cargo test -p trek-core
--locked`, packs `Trek-<version>-windows-x86_64.zip` - flat: `trek.exe`, `trek-mcp.exe` and
`trek-update.exe`, nothing else - and signs it with the same minisign key (trusted comment `Trek
<version> windows-x86_64`). It then downloads the release's `<channel>.json`, adds or replaces the
`windows-x86_64` entry (keeping the Mac's; a version mismatch, or a manifest with no `darwin-*` entry
because the Mac hasn't published, fails the job before anything is uploaded) and uploads the zip, its
`.minisig` and finally the manifest, with `--clobber`. On beta and nightly it also deletes the previous
build's Windows zip, as `release.sh` does for the Mac's. The merge lives in
`script/lib/release-manifest.ps1`, shared between the workflow and `script/release.ps1`;
`script/tests/release-manifest.Tests.ps1` covers it and runs in CI's Windows job.

Secrets the workflow needs (repo Settings > Secrets and variables > Actions):

- `TREK_MINISIGN_KEY`: the contents of `~/.trek-signing/minisign.key`, pasted whole (it is all ASCII).
- `TREK_MINISIGN_PASSWORD`: that key's password. minisign reads it from stdin, so a non-interactive
  runner works.

The job downloads `minisign-0.12-win64.zip` from jedisct1/minisign's GitHub release and checks it
against a pinned SHA-256 before running it; bump the version and hash together.

Local run: `pwsh script/release.ps1 0.4.0` dry-runs on a Windows machine (build, zip, sign if a key is
configured, manifest in `dist/release/0.4.0/`; nothing is uploaded). `-Publish` does what the workflow
does against the release that must already exist. `-Key`, `-PubKey` and `-Minisign` point at other keys
or binaries for testing; publishing always verifies against `assets/update/minisign.pub`. `BUILD=0`
reuses the binaries already in the release target dir for exercising the script (refused with
`-Publish`).

Windows binaries are **not Authenticode-signed** - there is no certificate yet - so SmartScreen shows a
one-time warning on first launch (**More info** > **Run anyway**). The minisign signature still gates
updates: the updater verifies it before touching the install folder. To install, the user unzips
anywhere their account can write (not `Program Files`) and runs `trek.exe`; `trek-update.exe` swaps
the install folder on update, which is why it must stay user-writable.

## How an update installs

1. Check: on launch, then once a day by the wall clock (sleep counts), and when the channel changes. A
   failed check, or a download that failed for network reasons, is retried after an hour; a release that
   doesn't verify waits for the next daily check. With automatic downloads off, the sidebar shows a
   neutral **Update** pill and the popover offers **Download**.
2. Download into a folder of its own, `updates/download-<pid>-<n>.noindex/` in Trek's data folder,
   hashing and verifying the minisign signature while streaming. A mismatch deletes the folder. Nothing
   is downloaded at all if the manifest's signature is missing or names another version. Changing the
   channel stops a download and deletes anything the old channel downloaded or staged.
3. Stage: unpack in that folder, then check the bundle id, that the version is exactly the advertised one
   and newer than the running one, and the code signature; clear quarantine. Now the sidebar shows an
   ember **Update** pill and Settings → Updates shows the release notes and **Restart to update**.
4. Install, when the user restarts (immediately if no agent turn is running, otherwise as soon as the last
   one finishes) or quits Trek. If the app on disk is already at least that version (another copy of
   Trek updated it, or it was replaced by hand), nothing is installed. Otherwise: move the staged app next
   to the running one, swap the two in one `renamex_np(RENAME_SWAP)` (or two renames on volumes without
   it; if the second fails the current app is put back, and if even that fails it's left where the error
   says rather than deleted), keep the previous version in `updates/previous.noindex/` as the one backup,
   relaunch with `open -n`. If the new app can't be opened, the relaunch helper puts the version this
   install replaced back, writes the new version to `updates/skip-version` so the restored one doesn't
   install it again, and opens the restored one (never an older backup when nothing was replaced). A
   newer release is offered as usual.
5. The new version deletes download folders of Trek processes that are gone and shows "Trek updated to
   <version>". Download folders are named after their process so that a second Trek sharing the data
   folder never deletes one that's in use.

Development builds never check for, download, install or clean up updates: `cargo run`, and bundles built
with `script/bundle.sh` on its own (no `TrekRelease` key), which would otherwise swap fresh code for an
older published build. `TREK_UPDATE_LOCAL_BUNDLE=1` lets a local bundle update anyway. Neither does a copy
macOS runs translocated from Downloads, nor one in a folder Trek can't write to (a standard account with
Trek in `/Applications`): Settings → Updates says to move it to a folder you own.

### Windows

The Windows artifact is `Trek-<version>-windows-x86_64.zip` (`windows-aarch64` on Arm), listed in the
same channel manifests under the platform key `windows-x86_64`, signed the same way (trusted comment
`Trek <version> windows-x86_64`). The zip is **flat**: exactly `trek.exe`, `trek-mcp.exe` and
`trek-update.exe` (built with `cargo build --release -p trek-app -p trek-mcp -p trek-update`), nothing
else, no folder around them; assets are embedded. Trek is installed as the folder holding them, and
`trek.exe`'s version resource (`ProductVersion`, written by trek-app's `build.rs`) must be the release's
full semver, as `CFBundleVersion` is on macOS.

The steps above, with these differences:

- **Stage**: the zip unpacks into `updates\download-<pid>-<n>.noindex\stage\` and is refused unless it
  holds exactly those three files (no folders, links or other names, at most 1 GiB) and `trek.exe`'s
  version is the advertised one and newer than the running one. A refusal deletes the download folder.
- **Install**: Windows won't let Trek replace the folder it runs from. On **Restart to update** (or on
  quit) Trek starts `trek-update.exe` from the staged folder and exits. The helper waits up to a minute
  for Trek to exit (then gives up, changing nothing), brings the staged folder next to the install (a
  rename, or a copy if it's on another drive: `<install>.old-incoming-<version>`), renames the install to
  `<install>.old-<version>` and the update into its place, checks `trek.exe` is there
  and, on a restart, starts it. If any step fails, or the new Trek exits with an error in its first five
  seconds, it puts the previous folder back, starts it, and leaves `updates\install-failed`, which Trek
  shows as a toast; a new version that wouldn't start also goes into `updates\skip-version`. Renames are
  retried for about two seconds (antivirus scans hold files). The helper deletes nothing; it logs to
  `%LOCALAPPDATA%\Trek\logs\update.log` (`<TREK_DATA_DIR>\logs\update.log` when that's set).
- **After**: half a minute after it starts (the helper watches it for five seconds and may need the
  previous folder back), the new Trek deletes `<install>.old-*` (only folders holding nothing but Trek's
  files, never a link) and, at once, the download folders of Trek processes that are gone.
- **When it can't**: a `cargo` build (the exe is in a folder with `.fingerprint`), Trek in Program Files
  or WindowsApps, a folder it or its parent can't be written to, and a folder that holds anything besides
  Trek's three files (unzipped straight into Downloads, say: the whole folder is what gets replaced), or
  one that is a junction or symlink. Settings → Updates says which and what to do.

A file added to the Windows release must first be added to `TREK_FILES` (trek-core `update.rs` and
`trek-update`) in an earlier release: older versions refuse a zip, and a folder, holding a file they
don't know.

To roll back by hand on Windows: quit Trek, rename the install folder, rename `<install>.old-<version>`
back if it's still there (or unzip the previous release in its place), and write the version you left
into `skip-version` in `%APPDATA%\trek\Trek\data\updates`.

To roll back by hand: quit Trek, then

```sh
cd ~/Library/Application\ Support/dev.trek.Trek/updates
plutil -extract CFBundleVersion raw -o skip-version /Applications/Trek.app/Contents/Info.plist
mv /Applications/Trek.app /tmp/ && mv previous.noindex/Trek.app /Applications/
```

`skip-version` keeps the version you rolled back to from downloading and installing the one you left
again; a newer release is still offered. Delete the file to be offered that version again.

## Testing an update without publishing

```sh
script/release.sh 0.2.0 --dry-run
script/release.sh 0.2.1 --dry-run
OLD=dist/release/0.2.0/Trek-0.2.0-darwin-aarch64.app.tar.gz
script/update-e2e.sh $OLD dist/release/0.2.1
script/update-e2e.sh $OLD dist/release/0.2.1 --tamper checksum
script/update-e2e.sh $OLD dist/release/0.2.1 --tamper signature
```

`update-e2e.sh` unpacks the old version into `/tmp/trek-update-e2e`, serves the new release's manifest
and archive from `python3 -m http.server` on localhost, and runs the old Trek with its own `TREK_DATA_DIR`,
`TREK_BACKGROUND=1` (no focus stealing) and `TREK_UPDATE_AUTO_RESTART=1`. That last flag exists only for
this test: it restarts as soon as an update is ready instead of waiting for a click or a quit (agent turns
are still waited for). The script passes when the old version downloads, verifies, swaps itself and the
new version starts — or, with `--tamper`, when the damaged archive is refused and the app is untouched —
and then stops everything it started and deletes its folder.

On Windows, `script/update-e2e.ps1` does the same with zips, plus `-Lock`, which holds a file in the
install open so the swap fails and must roll back. Without the release key, sign test builds with a
key of your own (`fixture` makes it; Trek trusts it when built with `TREK_UPDATE_PUBKEY`):

```powershell
script/update-e2e.ps1 -NewKey $env:TEMP\trek-e2e-key      # prints the key line
$env:TREK_UPDATE_PUBKEY = '<that line>'
cargo build --release -p trek-app -p trek-mcp -p trek-update  # once at each version, copying the three .exe files out
script/update-e2e.ps1 -Pack <old exes> -Key $env:TEMP\trek-e2e-key -Out <old dir>
script/update-e2e.ps1 -Pack <new exes> -Key $env:TEMP\trek-e2e-key -Out <new dir>
script/update-e2e.ps1 <old dir>\Trek-<old>-windows-x86_64.zip <new dir>
script/update-e2e.ps1 <old zip> <new dir> -Tamper checksum
script/update-e2e.ps1 <old zip> <new dir> -Tamper signature
script/update-e2e.ps1 <old zip> <new dir> -Lock
```

The feed is `fixture serve` on 127.0.0.1 (no Python needed), and the test Trek runs from
`%TEMP%\trek-update-e2e` with its own data folder. Where several checkouts share one target folder,
each building its own `fixture.exe` into it, copy yours out and name it in `TREK_E2E_FIXTURE`. The
release builds can skip LTO (`CARGO_PROFILE_RELEASE_LTO=false`) to save time; it changes nothing the
updater does.
