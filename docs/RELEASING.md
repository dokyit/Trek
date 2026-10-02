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
   certificate. `script/signing-identity.sh --check` shows which identity bundles will be signed with.

   Every release must be signed with the same certificate: macOS ties Accessibility and Screen Recording
   grants to it (the designated requirement is `identifier "dev.trek.Trek" and certificate leaf = H"…"`),
   so a new certificate means every user grants those permissions again. To publish from another Mac,
   import the same p12 there (`security import codesign.p12 -k login.keychain-db -P "$(cat
   codesign.p12.password)" -T /usr/bin/codesign`) instead of creating a new identity.
   With an Apple Developer ID you'd set `TREK_SIGN_IDENTITY` and add notarization; neither is done yet.
3. **Update signing key**: `~/.trek-signing/minisign.key` (mode 600), created with
   `minisign -G -W -p ~/.trek-signing/minisign.pub -s ~/.trek-signing/minisign.key`.
   `-W` means no password, so `release.sh` can sign unattended; the file permissions and FileVault are
   its protection, so keep it off shared machines and out of backups you don't control. Only the public
   half is in the repository (`assets/update/minisign.pub`), compiled into every build by
   `trek-core/src/update.rs`.

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
version. Versions follow semver, so `0.3.0-beta.2 < 0.3.0` and switching from nightly back to stable waits
for the next stable release rather than downgrading. Trek never installs an older or equal version.

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

1. Checks the version (semver; stable has no pre-release suffix, beta / nightly carry theirs; not older
   than the current one), that the version tag doesn't exist, and — for a real release — a clean working
   tree, `gh` logged in and a stable code-signing identity (ad-hoc releases are refused).
2. Sets `[workspace.package] version` in `Cargo.toml`.
3. Runs `script/bundle.sh`: release build of `trek` and `trek-mcp`, icon, `Info.plist`
   (`CFBundleVersion` carries the full semver; the updater checks it), code signing.
4. Packs `dist/Trek.app` without extended attributes into
   `dist/release/<version>/Trek-<version>-darwin-aarch64.app.tar.gz`, records its SHA-256, signs it with
   minisign (trusted comment `Trek <version> darwin-aarch64`; Trek refuses a signature whose trusted
   comment names another version) and verifies the signature against `assets/update/minisign.pub`, so a
   mismatched key fails here rather than on users' machines.
5. Writes `notes.md` (commit subjects since the previous tag, without merges and release commits) and the
   manifest `<channel>.json`:

   ```json
   { "version": "0.2.0", "notes": "- …", "pub_date": "2026-10-02T20:47:56Z",
     "platforms": { "darwin-aarch64": {
       "url": "https://github.com/dokyit/Trek/releases/download/v0.2.0/Trek-0.2.0-darwin-aarch64.app.tar.gz",
       "sha256": "…", "signature": "<the .minisig file>" } } }
   ```

6. Without `--dry-run`: commits `Release v<version>`, tags `v<version>` (stable and beta; nightlies only
   move the `nightly` tag), pushes the branch and tag, and creates the GitHub release with the archive,
   its `.minisig` and the manifest (or, for beta / nightly, refreshes the channel's release).
   With `--dry-run` the version bump is undone and nothing leaves the machine; the artifacts stay in
   `dist/release/<version>/`.

Environment: `TREK_MINISIGN_KEY` (secret key path), `TREK_RELEASE_REPO` (default `dokyit/Trek`),
`TREK_SIGN_IDENTITY`, and `TREK_RELEASE_DOWNLOAD_URL` to point the manifest's archive URL somewhere else
(a staging server).

## How an update installs

1. Check: on launch, then once a day (an hour after a failed check), and when the channel changes.
2. Download into `updates/` in Trek's data folder, hashing and verifying the minisign signature while
   streaming. A mismatch deletes the file. Nothing is downloaded at all if the manifest's signature is
   missing or names another version.
3. Stage: unpack into `updates/staged.noindex/`, then check the bundle id, that the version is exactly the
   advertised one and newer than the running one, and the code signature; clear quarantine. Now the
   sidebar shows **Update** and Settings → Updates shows the release notes and **Restart to update**.
4. Install, when the user restarts (immediately if no agent turn is running, otherwise as soon as the last
   one finishes) or quits Trek: move the staged app next to the running one, swap the two in one
   `renamex_np(RENAME_SWAP)` (or two renames with rollback on volumes without it), keep the previous
   version in `updates/previous.noindex/` as the one backup, relaunch with `open -n`. If the new app
   can't be opened, the relaunch helper puts the backup back and opens that.
5. The new version cleans up what's left in `updates/` and shows "Trek updated to <version>".

Development builds (`cargo run`, not inside an `.app`) never check for updates, and neither does a copy
macOS runs translocated from Downloads.

To roll back by hand: quit Trek, then
`mv /Applications/Trek.app /tmp/ && mv ~/Library/Application\ Support/dev.trek.Trek/updates/previous.noindex/Trek.app /Applications/`.

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
