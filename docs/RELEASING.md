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
   With an Apple Developer ID, `TREK_SIGN_IDENTITY` names it and the release is notarized: see
   "Gatekeeper and notarization" below. Without one (today), macOS asks once on first launch.
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
version. Trek never installs an older or equal version, nor one its channel doesn't carry (by the
version's pre-release: none is stable, `beta.N` beta, anything else nightly), whichever feed has it. So
while beta and nightly are behind the latest stable release, all three channels offer that release, and
Settings → Updates says so ("It's a stable release: there's no newer beta.").

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
   as a release), code signing. With the `TREK_NOTARY_*` variables set (a Developer ID identity only),
   `script/notarize.sh` then notarizes the app and staples the ticket to it, before it's archived.
4. Packs `dist/Trek.app` without extended attributes into
   `dist/release/<version>/Trek-<version>-darwin-aarch64.app.tar.gz`, records its SHA-256, signs it with
   minisign (trusted comment `Trek <version> darwin-aarch64`; Trek refuses a signature whose trusted
   comment names another version) and verifies the signature against `assets/update/minisign.pub`, so a
   mismatched key fails here rather than on users' machines.

   Then makes the disk image people install from, `Trek-<version>-darwin-aarch64.dmg`
   (`script/dmg.sh`): a window with Trek.app on the left, an Applications link on the right, a
   background with an arrow, "Drag Trek to Applications" and the Open Anyway path, and Trek's volume
   icon. It's built with [dmgbuild](https://github.com/dmgbuild/dmgbuild), which writes the window's
   layout (`.DS_Store`) itself, so no Finder scripting and it works on a headless runner; `dmg.sh`
   installs it into a virtualenv of its own from `script/lib/dmgbuild-requirements.txt`, pinned by
   hash (Python 3.10 or later). The layout is `script/lib/dmg-settings.py`; the background is
   `assets/dmg/background.svg`, rendered to the committed `background.png` and `background@2x.png` by
   `script/dmg-background.sh` (committed so the text doesn't depend on the build machine's fonts):
   change the icon positions and the SVG together. The image is ULFO (LZFSE; opens on macOS 10.11 and
   later). It is signed only with a Developer ID identity (and then notarized): a disk image signed by
   a certificate macOS doesn't trust is assessed when opened and refused outright, while an unsigned one
   mounts, and the app inside carries the signature that matters. The updater never downloads the
   disk image, so it has no `.minisig` and isn't in the manifest.
5. Writes `notes.md` (commit subjects since the most recent tag of the channel's line — the last stable
   tag for stable, the last version tag for beta, the `nightly` tag for nightly — without merges and
   release commits) and the manifest `<channel>.json`:

   ```json
   { "version": "0.2.0", "notes": "- …", "pub_date": "2026-10-02T20:47:56Z",
     "platforms": { "darwin-aarch64": {
       "url": "https://github.com/dokyit/Trek/releases/download/v0.2.0/Trek-0.2.0-darwin-aarch64.app.tar.gz",
       "sha256": "…", "signature": "<the .minisig file>" } } }
   ```

   `release-notes.md` is the GitHub release's body: `notes.md`, a blank line and an **Install** block with
   one bullet per platform. The Mac bullet sends people to the disk image first (open it, drag Trek to
   Applications), gives the Open Anyway path for the first launch (left out when the release was
   notarized), and names the `.app.tar.gz` as the same app in an archive; the Windows one gives the
   zip, the folder rule (see "Windows" below) and the SmartScreen step. The wording lives in one place,
   `trek_install_block` in `script/lib/notes.sh`; `README.md`'s Install sections say the same things, so
   change them together. A `--notes` file that already has an install block (a `## Install` heading, or a
   `**Windows**:` line) keeps its own, and the log says so. Each publish replaces the body whole, so a
   beta's moving release never carries two.

6. Without `--dry-run`:
   - stable: commits `Release v<version>` on the current branch, tags `v<version>`, pushes both and
     creates the GitHub release (marked latest) with the disk image, the archive, its `.minisig`,
     `stable.json`, and `release-notes.md` as its body;
   - beta / nightly: records the build as a commit off the branch, tags it (`v<version>` for a beta),
     moves the channel tag to it, pushes the tags only, and refreshes the channel's prerelease (its body
     too): archive, signature and disk image first, manifest last, then older archives and disk images
     are deleted. The branch doesn't move and the version bump is undone.

   With `--dry-run` the version bump is undone and nothing leaves the machine; the artifacts stay in
   `dist/release/<version>/`.

Environment: `TREK_MINISIGN_KEY` (secret key path), `TREK_MINISIGN_PASSWORD` (its password, for a run with
no terminal; otherwise it's asked for), `TREK_RELEASE_REPO` (default `dokyit/Trek`),
`TREK_SIGN_IDENTITY`, `TREK_RELEASE_DOWNLOAD_URL` to point the manifest's archive URL somewhere else
(a staging server), and `TREK_NOTARY_KEY` / `TREK_NOTARY_KEY_ID` / `TREK_NOTARY_ISSUER` to notarize
(all three, with a Developer ID `TREK_SIGN_IDENTITY`; see "Gatekeeper and notarization").

## Publishing from anywhere

`.github/workflows/release.yml` publishes a whole release without the Mac in the loop: its macOS job runs
`release.sh` on GitHub's macOS runner, exactly as above, then it calls `release-windows.yml` for the
Windows half. Start it from any machine with `gh`, or with **Run workflow** on the repository's Actions
tab:

**The first time**, in this order: add the secrets (below), run a dry run, then the real run.

```sh
gh workflow run release.yml --ref windows-port -f version=0.4.1-beta.4 -f channel=beta -f dry_run=true  # publish nothing
gh workflow run release.yml --ref windows-port -f version=0.4.1-beta.4 -f channel=beta                  # the real thing
```

`--ref` is the branch the workflow file is on and the release is built from. Once `release.yml` is on the
default branch, `--ref` can go for a release from there, and the **Run workflow** button on the Actions tab
is the sure way to start it (GitHub documents `workflow_dispatch` as a default-branch feature).

Inputs: `version`, `channel` (stable, beta or nightly; stable by default), optional `notes` (Markdown, used
as `--notes`; empty means the commit subjects as usual; the install block is added to the release body
either way) and `dry_run`. Run it on the branch you release
from: a stable release commits `Release v<version>` to that branch and pushes it (as the runner's
`github-actions[bot]`, so CI doesn't run on that commit, and a protected branch would refuse the push
before anything is published), and a beta or nightly is built from its head. The macOS job and the Windows `build` job run in
parallel — the Mac about 9 minutes and Windows about 11–13 once the release-profile cache is warm
(`warm-release.yml` rebuilds it on the first push to main after a dependency changes; the
thin-LTO link of `trek.exe` alone takes ~6.5 minutes on the 4-core runner), and only the small `publish` job that merges the Windows asset
into the release waits on both. Between the macOS publish and the Windows publish, Trek on Windows
sees the new version without a Windows build and says "the Windows build is on its way"; Mac users can
update at once.

If the Windows job fails, the release exists with only `darwin-aarch64`: re-run just that job (**Re-run
failed jobs** on the run, or `gh workflow run release-windows.yml --ref windows-port` with the same version
and channel). It only attaches to the release that exists and replaces its own assets, so it can be run
again. If the macOS job fails after pushing the tag but before the release exists, `release.sh` refuses the
same version again ("tag … already exists"): take the next version, or delete the tag it pushed
(`git push origin :refs/tags/v<version>`).

A dry run builds, code-signs and minisigns both halves, verifies the signatures against
`assets/update/minisign.pub`, shows both manifests and SHA-256s in the run's summary, keeps the Mac disk
image as the run's `mac-dmg` artifact (a week) to open and look at, and publishes nothing: no commit,
no tag, no release. Without the code-signing secrets below it signs with a throwaway
"Trek Local Signing" that `script/signing-identity.sh` makes on the runner. A dry run does not prove the
real p12, the pushes or the release upload; the first real run is the first time those happen.

Secrets, added once from the Mac that has `~/.trek-signing/` (repository Settings > Secrets and
variables > Actions, or `gh`):

```sh
gh secret set TREK_CODESIGN_P12 --body "$(base64 -i ~/.trek-signing/codesign.p12)"
gh secret set TREK_CODESIGN_P12_PASSWORD < ~/.trek-signing/codesign.p12.password
gh secret set TREK_MINISIGN_KEY < ~/.trek-signing/minisign.key   # already set for the Windows workflow
gh secret set TREK_MINISIGN_PASSWORD                              # if the key has a password; gh asks for it
```

The job imports the p12 into a keychain of its own on the runner (only `/usr/bin/codesign` may use the
key), signs with it, and deletes that keychain and the key files when it ends, failed or not. A real
release refuses to run without `TREK_CODESIGN_P12`: a different certificate would reset every user's
Accessibility and Screen Recording grants. `release.sh` reads the minisign password from
`TREK_MINISIGN_PASSWORD` when it's set, and asks at the terminal otherwise; on a runner (`CI` set) a key
with a password and no `TREK_MINISIGN_PASSWORD` stops the job before the build.

The minisign key is the one secret that matters most (see "Update signing key" above): anyone who can
push a workflow to this repository can read the secrets (true since the Windows workflow got the key), so
keep the repository's write access as tight as the Mac's. The key uploaded should carry a password, and
`TREK_MINISIGN_PASSWORD` should hold it: that protects the key where the secret alone leaks, not against
someone who can run a workflow, which sees both. A key without a password still signs, with
`release.sh`'s warning in the log.

The Mac's own way, `script/release.sh` on the Mac, works as before. Run one or the other for a release,
not both.

## Gatekeeper and notarization

**Today Trek isn't notarized, so macOS always asks once.** Releases are signed with "Trek Local
Signing", a self-signed certificate; Gatekeeper trusts only Apple-issued Developer ID signatures that
Apple has also notarized. A downloaded (quarantined) Trek is blocked on first launch whatever the
packaging: "Trek" Not Opened → **Done**, then **System Settings › Privacy & Security**, scroll to
**Security**, **Open Anyway** beside "Trek was blocked to protect your Mac", **Open Anyway** again and
the login password or Touch ID. Since macOS 15 the old right-click › Open shortcut no longer skips
this. The disk image's background and the release body's install block both give that path. macOS
asks only once per installed copy; updates Trek installs itself aren't quarantined and don't ask.

**What it takes to remove the prompt:**

1. **Apple Developer Program**: US$99 a year (individual; an organization also needs a D-U-N-S
   number). Enrollment takes a day or two to be approved.
2. **Developer ID Application certificate** (the Account Holder makes it): Xcode › Settings › Accounts ›
   Manage Certificates › + › Developer ID Application, or a CSR at developer.apple.com › Certificates.
   Export it with its private key from Keychain Access as a `.p12` with a password.
3. **App Store Connect API key** for `notarytool`: App Store Connect › Users and Access › Integrations ›
   App Store Connect API › Team Keys › +, role **Developer**. Download `AuthKey_<id>.p8` (it can be
   downloaded once) and note the key ID and the issuer ID shown above the list.
4. **Secrets** (all five, or the workflow skips notarization with a warning):

   ```sh
   gh secret set TREK_DEVELOPER_ID_P12 --body "$(base64 -i DeveloperID.p12)"
   gh secret set TREK_DEVELOPER_ID_P12_PASSWORD          # gh asks for it
   gh secret set APPLE_NOTARY_KEY < AuthKey_ABCDE12345.p8
   gh secret set APPLE_NOTARY_KEY_ID --body ABCDE12345
   gh secret set APPLE_NOTARY_ISSUER_ID --body 00000000-0000-0000-0000-000000000000
   ```

With them, `release.yml`'s "Developer ID and notarization" step imports the certificate into a keychain
of its own and sets `TREK_SIGN_IDENTITY` to it, and `release.sh` does the rest:

- `script/lib/sign.sh` signs `trek`, `trek-mcp` and the bundle with the hardened runtime, a secure
  timestamp and `script/lib/hardened-runtime.entitlements` (microphone for dictation, Apple events for
  computer use; the runtime refuses both without them). Only a Developer ID switches these on; Trek
  Local Signing builds are signed exactly as before.
- `script/notarize.sh` sends the app (zipped) to Apple with `xcrun notarytool submit --wait`, staples
  the ticket to it with `xcrun stapler staple` before the archive is made (so the updater's archive
  carries it too), then does the same for the signed disk image. A rejection prints Apple's log
  (`notarytool log`) and stops the release. Each submission usually takes a few minutes.
- The install block drops the Open Anyway step, and the run's summary says "notarized and stapled".

The same works from the Mac: `TREK_SIGN_IDENTITY="Developer ID Application: … (TEAMID)"
TREK_NOTARY_KEY=~/.trek-signing/AuthKey_<id>.p8 TREK_NOTARY_KEY_ID=… TREK_NOTARY_ISSUER=…
script/release.sh <version>`.

**Switching is a one-time break for existing users**, so do it at a release that says so: a new
certificate changes Trek's designated requirement, so macOS asks again for Accessibility, Screen
Recording and the microphone, and Keychain items an earlier Trek saved (API keys, MCP tokens) can no
longer be read without the user's login password; Trek shows them as locked in Settings instead of
asking, and re-entering a key saves it afresh. After that the requirement names the team, not the
certificate, so renewing the Developer ID certificate doesn't break it again. Keep `TREK_CODESIGN_P12`
set as well: the workflow still imports it (and falls back to it if the Developer ID secrets go).

## Windows

The Mac's `release.sh` publishes first - it creates the release and its `<channel>.json`. Windows then
attaches to the same release: `.github/workflows/release-windows.yml` runs on `windows-latest` and never
creates a release itself; it fails if the tag's release doesn't exist yet. The order matters in both
directions: `release.sh` writes `<channel>.json` with only the Mac's entry, so a Mac publish (a new
beta or nightly, or a re-run of one) drops the Windows entry until the workflow runs again.

- **Stable**: publishing the `v<version>` release triggers the workflow automatically.
- **Beta / nightly**: run it by hand once the Mac's prerelease exists under the moving `beta`/`nightly`
  tag: `gh workflow run release-windows.yml -f version=0.3.0-beta.1 -f channel=beta`.
- **From `release.yml`**: it runs right after the macOS job, whatever the channel. A release that run
  creates raises no `release: published` event (GitHub doesn't start workflows for its own token's
  events), so a stable release isn't attached twice.

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
updates: the updater verifies it before touching the install folder. To install, the user unzips into a
folder of its own that Windows doesn't guard, such as `C:\Users\<you>\Trek` or
`%LOCALAPPDATA%\Programs\Trek` (not Documents, Desktop, Downloads or `Program Files`), and runs
`trek.exe`; `trek-update.exe` swaps the install folder on update, which is why it must stay
user-writable. Documents and Desktop look like folders the user owns, but Defender's Controlled folder
access (ransomware protection), when it's on, lets only programs it trusts write there, and judges the
`trek-update.exe` each update brings on its own.

The install line comes from `release.sh`, not from whoever writes the notes. Before that, each release's
body depended on its `--notes` file: beta.3's said "Unzip it into a folder you own (not Program Files)",
which put Tobias's own install under `OneDrive\Documents`, where Controlled folder access blocked the
updater. Now every body carries the block from `trek_install_block`, and a `--notes` file that has its own
keeps it.

## How an update installs

1. Check: on launch, then every hour by the wall clock (sleep counts) on every channel, and at once when
   the channel changes. A failed check, or a download that failed for network reasons, is retried after
   an hour; a newer version without this platform's build yet after 15 minutes; a release that doesn't
   verify waits a day. Checks go on while an update is staged and waiting for a restart: a newer release
   replaces it (the staged copy is deleted). Changing the channel keeps an update the new channel also
   offers (a stable release, on any channel; a beta, going to Nightly) and drops the rest. With automatic
   downloads off, the sidebar shows a neutral **Update** pill and the popover offers **Download**.
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
- **Ransomware protection**: inside Documents, Desktop, Pictures, Videos, Music or Favorites (where
  `SHGetKnownFolderPath` says they are, OneDrive included, and in OneDrive either way) while Controlled
  folder access is on (`EnableControlledFolderAccess` = 1 under `HKLM\SOFTWARE\Microsoft\Windows
  Defender\Windows Defender Exploit Guard\Controlled Folder Access`, or its twin under
  `HKLM\SOFTWARE\Policies\…`, which wins; both read without administrator rights) **and** Defender is
  scanning in real time (the setting enforces nothing otherwise: `DisableRealtimeMonitoring`,
  `DisableAntiSpyware` and `DisableAntiVirus`, setting and policy, aren't 1, and Windows Security's
  list of antivirus products, `HKLM\SOFTWARE\Microsoft\Security Center\Provider\Av`, has Defender on
  and no other product on beside it), Settings → Updates says to move Trek out. With real-time
  protection switched off, nothing is guarded, whatever the setting says. If any of that can't be read
  or is ambiguous (Defender passive under another antivirus, say), a refused write there decides, and
  no refused write means no blocker. A refused write
  (this, or a folder Trek can't write to) is asked again on **Check now**: Defender judges each program
  as it runs, and the user may turn it off. If the helper is still turned away (access denied bringing
  the update next to the install), its toast says the same.
- **Moving out in one click**: for ransomware protection, Program Files, a folder holding more than
  Trek's files and a folder Trek can't write to, Settings → Updates offers **Move Trek to its own
  folder** (and so does the launch toast, from a guarded folder, and the toast for an update that
  ransomware protection turned away). Reading is never refused, only writing, so Trek copies
  `trek.exe`, `trek-mcp.exe` and `trek-update.exe` to `%LOCALAPPDATA%\Programs\Trek` (an own folder
  of Trek's with nothing running from it is replaced; otherwise `Trek-2`, …), checks each copy's size
  and SHA-256 against the original, writes `relocated-from` (the old and the new folder) in the
  updates folder and starts the copy's `trek-update.exe --relaunch`, which waits for Trek to exit
  and starts the copy; then Trek quits, after agents finish their turns as for "Restart to update".
  The copy says where it runs and, on **Remove old copy**, deletes the old folder (only Trek's files
  from one that holds more); if Windows refuses it says so, the one thing left to the user. Nothing
  in the old folder is written or changed before that. `TREK_RELOCATE_TO=<folder>` sends the copy
  elsewhere (testing), `TREK_RELOCATE_KEEP=TREK_SHOT_DIR,…` lets named `TREK_*` variables follow it.
  - **Check now** stays beside the move for a block Windows may lift (ransomware protection, a
    read-only place): Defender judges each program, and the user may have told it to let Trek be.
  - **The old copy stays on Settings → Updates** (a row above the page's own status, **Remove old
    copy** / **Dismiss**), not only on the launch toast, which lasts seconds. `relocated-from` is
    read once, so the copy writes a second note, `old-copy` (old and new folder), in the updates
    folder; every launch from the new folder offers the copy again until it's removed, gone, no
    longer holds `trek.exe`, or dismissed, and opening the page looks again. Another Trek sharing the
    data folder leaves the note alone.
  - **Shortcuts**: at the copy's first launch, the `.lnk` files in the user's own Start menu
    (`%APPDATA%\Microsoft\Windows\Start Menu\Programs`, two folders deep), Desktop (one deep) and
    taskbar pins (`…\Internet Explorer\Quick Launch\User Pinned\TaskBar`) whose target is exactly the
    old `trek.exe` (any case, `\\?\` ignored) get the new `trek.exe` as target, and the new folder as
    the working folder and the new exe as the icon when those were the old copy's (an icon or folder
    the user chose stays). Nothing else is touched or made; the shortcuts every user shares
    (`ProgramData`, the public Desktop) are the administrator's. The toast and the page say how many
    changed. The Desktop is a folder ransomware protection guards, so a change there may be refused:
    they say which shortcut still starts the old copy, in words. A taskbar pin may need Explorer
    restarted (or a re-pin) before the taskbar shows the change. `trek://` follows the move by itself:
    the copy registers it at launch, as every launch does (a process with its own `TREK_DATA_DIR` or
    `TREK_SHOT_DIR` doesn't).
- **A release without this platform's build**: the manifest lists each platform's build as it's
  published, the Mac's first. A newer version with no `windows-x86_64` entry yet shows "Trek <version> is
  out; the Windows build is on its way. Check again in a while." and is checked again within the hour.

A file added to the Windows release must first be added to `TREK_FILES` (trek-core `update.rs` and
`trek-update`) in an earlier release: older versions refuse a zip, and a folder, holding a file they
don't know.

To roll back by hand on Windows: quit Trek, rename the install folder, rename `<install>.old-<version>`
back if it's still there (or unzip the previous release in its place), and write the version you left
into `skip-version` in `%APPDATA%\trek\Trek\data\updates`.

To roll back by hand: quit Trek first (a bundle replaced under a running Trek is refused its
Accessibility and Screen Recording permissions until it restarts), then

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
