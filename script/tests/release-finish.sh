#!/usr/bin/env bash
# Tests for script/lib/release-finish.sh, the end of a stable release: the atomic push of the
# "Release v<version>" commit and its tag, carrying on from what an earlier run left, the release
# workflow's "is this only the version bump" check, and finishing a GitHub release. Each case runs
# against throwaway repositories (a bare "origin" and clones of it) in a temporary folder; gh is a
# stand-in that keeps the release in files. Runs with the Mac's bash 3.2:
#
#   bash script/tests/release-finish.sh
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
LIB="$ROOT/script/lib/release-finish.sh"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
fails=0

ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1"; fails=$((fails + 1)); }
# passes / refuses <what> <dir> <release-finish.sh args...>: run in <dir>, keep its output in $OUT.
OUT="$WORK/out"
passes() {
  local what=$1 dir=$2
  shift 2
  if (cd "$dir" && "$LIB" "$@") > "$OUT" 2>&1; then ok "$what"; else bad "$what"; sed 's/^/      /' "$OUT"; fi
}
refuses() {
  local what=$1 dir=$2
  shift 2
  if (cd "$dir" && "$LIB" "$@") > "$OUT" 2>&1; then bad "$what (it passed)"; sed 's/^/      /' "$OUT"; else ok "$what"; fi
}
same() { # what, actual, expected
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', want '$3'"; fi
}
said() { # what, text the last run printed
  if grep -qF -- "$2" "$OUT"; then ok "$1"; else bad "$1: no \"$2\" in:"; sed 's/^/      /' "$OUT"; fi
}

# Git that reads no one's own settings (signing, hooks, a default branch).
export GIT_CONFIG_GLOBAL="$WORK/gitconfig" GIT_CONFIG_NOSYSTEM=1
git config --global user.name 'Release Test'
git config --global user.email 'release-test@localhost'
git config --global init.defaultBranch main
git config --global advice.detachedHead false
export TREK_PUSH_DELAY=0 TREK_PUSH_ATTEMPTS=3

at() { git -C "$1" rev-parse -q --verify "$2" 2> /dev/null || echo none; }

# A workspace at 0.4.60: two crates of its own, and a registry dependency that happens to be 0.4.60
# too (a bump must leave it alone).
seed() { # dir
  mkdir -p "$1/src"
  cat > "$1/Cargo.toml" << 'EOF'
[workspace]
members = ["crates/a", "crates/b"]

[workspace.package]
version = "0.4.60"
edition = "2024"

[workspace.dependencies.other]
version = "0.4.60"
EOF
  cat > "$1/Cargo.lock" << 'EOF'
version = 4

[[package]]
name = "a"
version = "0.4.60"

[[package]]
name = "b"
version = "0.4.60"
dependencies = [
 "other",
]

[[package]]
name = "other"
version = "0.4.60"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111"
EOF
  echo 'pub fn a() {}' > "$1/src/lib.rs"
}

# A fresh origin with main at one commit, and a clone of it: $WORK/<name>/{origin.git,dev}.
fresh() { # name → prints the clone's path; P is main's commit
  local d="$WORK/$1"
  git init -q --bare "$d/origin.git"
  git init -q "$d/dev"
  seed "$d/dev"
  git -C "$d/dev" add -A
  git -C "$d/dev" commit -q -m "Some work"
  git -C "$d/dev" remote add origin "$d/origin.git"
  git -C "$d/dev" push -q origin HEAD:refs/heads/main
  echo "$d/dev"
}

# release.sh's bump of the working tree: [workspace.package] and the workspace's own lock entries.
bump_tree() { # dir version
  sed -i.bak '/^\[workspace\.package\]/,/^\[/s/^version = ".*"/version = "'"$2"'"/' "$1/Cargo.toml"
  awk -v v="$2" '/^name = / { own = ($3 == "\"a\"" || $3 == "\"b\"") } own && /^version = / { $0 = "version = \"" v "\"" } { print }' \
    "$1/Cargo.lock" > "$1/Cargo.lock.new"
  mv "$1/Cargo.lock.new" "$1/Cargo.lock"
  rm -f "$1/Cargo.toml.bak"
}
bump() { # dir version [message]: commit it
  bump_tree "$1" "$2"
  git -C "$1" commit -q -am "${3:-Release v$2}"
}
# A clone of <name>'s origin with main checked out at <commit>, as the workflow's checkout does for
# a run (or a re-run) that started there.
clone_at() { # name commit → path
  local d="$WORK/$1/run$RANDOM"
  git clone -q "$WORK/$1/origin.git" "$d"
  git -C "$d" checkout -q -B main "$2"
  echo "$d"
}

echo "# is-bump: the gate's check"
dev=$(fresh gate)
P=$(at "$dev" HEAD)
bump "$dev" 0.4.61
passes "a pure bump of the version is one" "$dev" is-bump HEAD 0.4.61
said "it says so, and from what" "0.4.60 → 0.4.61"
refuses "a bump to another version than the one asked for isn't" "$dev" is-bump HEAD 0.4.62
refuses "nor is its parent" "$dev" is-bump HEAD^ 0.4.61
git -C "$dev" checkout -q -b with-source "$P"
bump_tree "$dev" 0.4.61
echo 'pub fn b() {}' >> "$dev/src/lib.rs"
git -C "$dev" commit -q -am "Release v0.4.61"
refuses "a bump that also changes a source file isn't" "$dev" is-bump HEAD 0.4.61
said "and it names the file" "src/lib.rs"
git -C "$dev" checkout -q -b dependency "$P"
bump_tree "$dev" 0.4.61
sed -i.bak 's/^version = "0.4.60"$/version = "0.4.61"/; s/^checksum = "1111"$/checksum = "2222"/' "$dev/Cargo.lock"
rm -f "$dev/Cargo.lock.bak"
git -C "$dev" commit -q -am "Release v0.4.61"
refuses "a bump that moves a registry dependency (and its checksum) isn't" "$dev" is-bump HEAD 0.4.61
git -C "$dev" checkout -q -b dependency-version-only "$P"
bump_tree "$dev" 0.4.61
awk '/^name = "other"/ { o = 1 } o && /^version = / { $0 = "version = \"0.4.61\""; o = 0 } { print }' "$dev/Cargo.lock" > "$dev/l" && mv "$dev/l" "$dev/Cargo.lock"
git -C "$dev" commit -q -am "Release v0.4.61"
refuses "nor one that moves only a dependency's version line, same old version or not" "$dev" is-bump HEAD 0.4.61
said "it names the dependency" "other"
git -C "$dev" checkout -q -b toml-elsewhere "$P"
bump_tree "$dev" 0.4.61
sed -i.bak '/^\[workspace\.dependencies\.other\]/,/^\[/s/^version = ".*"/version = "0.4.61"/' "$dev/Cargo.toml"
rm -f "$dev/Cargo.toml.bak"
git -C "$dev" commit -q -am "Release v0.4.61"
refuses "nor one whose Cargo.toml changes a version line outside [workspace.package]" "$dev" is-bump HEAD 0.4.61
git -C "$dev" checkout -q -b message "$P"
bump "$dev" 0.4.61 "Bump to 0.4.61"
refuses "nor a pure bump under another message" "$dev" is-bump HEAD 0.4.61
git -C "$dev" checkout -q -b mode "$P"
bump_tree "$dev" 0.4.61
chmod +x "$dev/Cargo.toml"
git -C "$dev" commit -q -am "Release v0.4.61"
refuses "nor one that also changes Cargo.toml's mode" "$dev" is-bump HEAD 0.4.61
git -C "$dev" checkout -q -b merged "$P"
git -C "$dev" commit -q --allow-empty -m "Other work"
git -C "$dev" merge -q --no-ff -m "Release v0.4.61" main 2> /dev/null
refuses "nor a merge" "$dev" is-bump HEAD 0.4.61
# The workflow's gate checks out two commits deep; one deep has no parent to compare with.
git -C "$dev" push -q origin main
git clone -q --depth 1 "file://$WORK/gate/origin.git" "$WORK/gate/shallow1"
refuses "a clone one commit deep can't tell" "$WORK/gate/shallow1" is-bump HEAD 0.4.61
git clone -q --depth 2 "file://$WORK/gate/origin.git" "$WORK/gate/shallow2"
passes "two commits deep, as the gate checks out, it can" "$WORK/gate/shallow2" is-bump HEAD 0.4.61

echo "# push: one atomic push of the bump and its tag"
dev=$(fresh fresh)
P=$(at "$dev" HEAD)
bump "$dev" 0.4.61
B=$(at "$dev" HEAD)
passes "a fresh release pushes main and the tag" "$dev" push 0.4.61 main
same "origin's main is the bump" "$(at "$WORK/fresh/origin.git" main)" "$B"
same "origin's tag is on it" "$(at "$WORK/fresh/origin.git" 'v0.4.61^{commit}')" "$B"
same "and it's annotated, as release.sh always made it" "$(git -C "$WORK/fresh/origin.git" cat-file -t v0.4.61)" "tag"
passes "pushing again is done at once" "$dev" push 0.4.61 main
said "it says origin has both" "origin has v0.4.61 on"

dev=$(fresh atomic)
P=$(at "$dev" HEAD)
cat > "$WORK/atomic/origin.git/hooks/pre-receive" << 'EOF'
#!/bin/sh
# Refuses every tag, as GitHub refused v0.4.61's.
while read -r old new ref; do
  case $ref in refs/tags/*) echo "no tags today" >&2; exit 1 ;; esac
done
EOF
chmod +x "$WORK/atomic/origin.git/hooks/pre-receive"
bump "$dev" 0.4.61
refuses "a tag origin won't take fails the push, after the retries" "$dev" push 0.4.61 main
said "it gave up after the attempts" "gave up after 3 pushes"
same "and main didn't get the bump without its tag" "$(at "$WORK/atomic/origin.git" main)" "$P"
same "nor is there a tag" "$(at "$WORK/atomic/origin.git" v0.4.61)" "none"

dev=$(fresh retry)
cat > "$WORK/retry/origin.git/hooks/pre-receive" << EOF
#!/bin/sh
# Refuses the first push only.
if [ ! -e "$WORK/retry/refused-once" ]; then touch "$WORK/retry/refused-once"; echo "try again" >&2; exit 1; fi
EOF
chmod +x "$WORK/retry/origin.git/hooks/pre-receive"
bump "$dev" 0.4.61
B=$(at "$dev" HEAD)
passes "a push refused once goes through on the retry" "$dev" push 0.4.61 main
said "it says it tried again" "refused (attempt 1 of 3)"
same "main is the bump" "$(at "$WORK/retry/origin.git" main)" "$B"
same "the tag is on it" "$(at "$WORK/retry/origin.git" 'v0.4.61^{commit}')" "$B"

echo "# resuming: the bump on origin without its tag (what run 44812702201 left)"
dev=$(fresh notag)
P=$(at "$dev" HEAD)
bump "$dev" 0.4.61
B=$(at "$dev" HEAD)
git -C "$dev" push -q origin HEAD:refs/heads/main
run=$(clone_at notag "$P")
passes "a re-run on the parent adopts the bump" "$run" adopt 0.4.61 main
same "its HEAD is now the bump" "$(at "$run" HEAD)" "$B"
passes "and pushes only the tag" "$run" push 0.4.61 main
same "origin's main is still the one bump" "$(at "$WORK/notag/origin.git" main)" "$B"
same "with one Release v0.4.61 on it" "$(git -C "$WORK/notag/origin.git" log --format=%s main | grep -c '^Release v0.4.61$')" "1"
same "and the tag on it" "$(at "$WORK/notag/origin.git" 'v0.4.61^{commit}')" "$B"
dev=$(fresh notag2)
bump "$dev" 0.4.61
B=$(at "$dev" HEAD)
git -C "$dev" push -q origin HEAD:refs/heads/main
run=$(clone_at notag2 "$B")
passes "a new run dispatched on the bump has nothing to adopt" "$run" adopt 0.4.61 main
said "it says so" "is this commit"
passes "and pushes the tag" "$run" push 0.4.61 main
same "on the bump" "$(at "$WORK/notag2/origin.git" 'v0.4.61^{commit}')" "$B"
dev=$(fresh ontop)
bump "$dev" 0.4.61
B=$(at "$dev" HEAD)
git -C "$dev" push -q origin HEAD:refs/heads/main
git -C "$dev" commit -q --allow-empty -m "Work after the bump"
git -C "$dev" push -q origin HEAD:refs/heads/main
Y=$(at "$dev" HEAD)
run=$(clone_at ontop "$B")
passes "with work pushed on top of the bump, the tag still goes on the bump" "$run" push 0.4.61 main
same "on the bump" "$(at "$WORK/ontop/origin.git" 'v0.4.61^{commit}')" "$B"
same "and main keeps the work" "$(at "$WORK/ontop/origin.git" main)" "$Y"

echo "# resuming: the bump and its tag on origin, no release (what attempt 2 met)"
for kind in annotated lightweight; do
  dev=$(fresh "tagged-$kind")
  P=$(at "$dev" HEAD)
  bump "$dev" 0.4.61
  B=$(at "$dev" HEAD)
  if [ "$kind" = annotated ]; then git -C "$dev" tag -a v0.4.61 -m "Trek 0.4.61"; else git -C "$dev" tag v0.4.61; fi
  git -C "$dev" push -q origin HEAD:refs/heads/main refs/tags/v0.4.61
  run=$(clone_at "tagged-$kind" "$P")
  git -C "$run" fetch -q --force origin 'refs/tags/*:refs/tags/*'
  passes "a re-run on the parent adopts the bump ($kind tag)" "$run" adopt 0.4.61 main
  passes "and finds nothing to push" "$run" push 0.4.61 main
  said "it says origin has both" "origin has v0.4.61 on"
  same "origin's main is unchanged" "$(at "$WORK/tagged-$kind/origin.git" main)" "$B"
done

echo "# refused at once"
dev=$(fresh taken)
P=$(at "$dev" HEAD)
git -C "$dev" tag -a v0.4.61 -m "Trek 0.4.61"
git -C "$dev" push -q origin refs/tags/v0.4.61
git -C "$dev" tag -d v0.4.61 > /dev/null
bump "$dev" 0.4.61
refuses "a tag origin has on another commit" "$dev" push 0.4.61 main
said "it says where it is" "taken by another commit"
same "main is left alone" "$(at "$WORK/taken/origin.git" main)" "$P"
dev=$(fresh moved)
P=$(at "$dev" HEAD)
git -C "$dev" commit -q --allow-empty -m "Pushed during the release"
git -C "$dev" push -q origin HEAD:refs/heads/main
X=$(at "$dev" HEAD)
run=$(clone_at moved "$P")
passes "adopt leaves a main that moved on to other work" "$run" adopt 0.4.61 main
same "HEAD stays the run's commit" "$(at "$run" HEAD)" "$P"
bump "$run" 0.4.61
refuses "and the push refuses it, without retrying" "$run" push 0.4.61 main
said "it says main moved on" "has moved on"
same "main still has the other work" "$(at "$WORK/moved/origin.git" main)" "$X"
dev=$(fresh impure)
P=$(at "$dev" HEAD)
bump_tree "$dev" 0.4.61
echo 'pub fn c() {}' >> "$dev/src/lib.rs"
git -C "$dev" commit -q -am "Release v0.4.61"
git -C "$dev" push -q origin HEAD:refs/heads/main
run=$(clone_at impure "$P")
passes "adopt leaves a \"Release v0.4.61\" that changes more than the version" "$run" adopt 0.4.61 main
same "HEAD stays the run's commit" "$(at "$run" HEAD)" "$P"

echo "# state: the failed run's summary"
mkdir -p "$WORK/bin" "$WORK/gh"
cat > "$WORK/bin/gh" << 'EOF'
#!/usr/bin/env bash
# A stand-in for gh's release commands: the release lives in $GH_STATE (exists, draft, assets/).
set -eu
echo "$*" >> "$GH_STATE/log"
[ "$1" = release ] || exit 2
cmd=$2; shift 2
tag=$1; shift
case $cmd in
  view)
    [ -e "$GH_STATE/exists" ] || { echo "release not found" >&2; exit 1; }
    if [[ "$*" == *tostring* ]]; then
      if [ -e "$GH_STATE/draft" ]; then echo true; else echo false; fi
      ls "$GH_STATE/assets"
    else
      echo "$(ls "$GH_STATE/assets" | wc -l | tr -d ' ') assets"
    fi ;;
  create)
    [ ! -e "$GH_STATE/create-fails" ] || { echo "HTTP 502" >&2; exit 1; }
    touch "$GH_STATE/exists"
    while [ $# -gt 0 ]; do
      case $1 in --repo|--title|--notes-file) shift 2 ;; -*) shift ;; *) cp "$1" "$GH_STATE/assets/"; shift ;; esac
    done ;;
  upload)
    for f in "$@"; do [ -f "$f" ] && cp "$f" "$GH_STATE/assets/"; done; true ;;
  download)
    pattern="" output=""
    while [ $# -gt 0 ]; do
      case $1 in --pattern) pattern=$2; shift 2 ;; --output) output=$2; shift 2 ;; *) shift ;; esac
    done
    [ -f "$GH_STATE/assets/$pattern" ] || exit 1
    cp "$GH_STATE/assets/$pattern" "$output" ;;
  edit) rm -f "$GH_STATE/draft" ;;
  *) exit 2 ;;
esac
EOF
chmod +x "$WORK/bin/gh"
export PATH="$WORK/bin:$PATH" GH_STATE="$WORK/gh"
gh_reset() { rm -rf "$GH_STATE"; mkdir -p "$GH_STATE/assets"; : > "$GH_STATE/log"; }
gh_reset
dev=$(fresh state)
P=$(at "$dev" HEAD)
bump "$dev" 0.4.61
git -C "$dev" push -q origin HEAD:refs/heads/main
run=$(clone_at state "$P")
passes "state reads a bump without its tag" "$run" state 0.4.61 stable "$P" 123
said "it names the bump" "Release v0.4.61"
said "and the missing tag" "not on origin"
said "and the missing release" "not there"
said "and says to re-run the failed jobs" "gh run rerun 123 --failed"
start=$(at "$WORK/moved/dev" HEAD~1)
run=$(clone_at moved "$start")
passes "state reads a main that moved on" "$run" state 0.4.61 stable "$start" 123
said "and says to dispatch again" "gh workflow run release.yml --ref main -f version=0.4.61 -f channel=stable"

echo "# github: creating the release, or finishing one"
out="$WORK/out-release"
mkdir -p "$out"
for f in Trek-0.4.61-darwin-aarch64.dmg Trek-0.4.61-darwin-aarch64.app.tar.gz Trek-0.4.61-darwin-aarch64.app.tar.gz.minisig; do echo "$f" > "$out/$f"; done
cat > "$out/stable.json" << 'EOF'
{"version": "0.4.61", "notes": "mac notes", "pub_date": "2026-11-20T00:00:00Z",
 "platforms": {"darwin-aarch64": {"url": "u-mac", "sha256": "s-mac", "signature": "sig-mac"}}}
EOF
echo "notes" > "$out/release-notes.md"
assets=("$out/Trek-0.4.61-darwin-aarch64.dmg" "$out/Trek-0.4.61-darwin-aarch64.app.tar.gz" "$out/Trek-0.4.61-darwin-aarch64.app.tar.gz.minisig" "$out/stable.json")
gh_reset
passes "no release yet: it's created" "$ROOT" github 0.4.61 "$out/release-notes.md" "${assets[@]}"
same "with one create and its four assets" "$(grep -c '^release create v0.4.61' "$GH_STATE/log") $(find "$GH_STATE/assets" -type f | wc -l | tr -d ' ')" "1 4"
gh_reset
touch "$GH_STATE/exists"
cp "${assets[@]}" "$GH_STATE/assets/"
echo "rebuilt" > "$WORK/rebuilt.tar.gz"
passes "a release that has this platform whole is left as it is" "$ROOT" github 0.4.61 "$out/release-notes.md" "${assets[@]}"
same "nothing uploaded or created" "$(grep -cE '^release (upload|create)' "$GH_STATE/log" || true)" "0"
gh_reset
touch "$GH_STATE/exists" "$GH_STATE/draft"
cat > "$GH_STATE/assets/stable.json" << 'EOF'
{"version": "0.4.61", "notes": "the release's notes", "pub_date": "2026-11-20T01:00:00Z",
 "platforms": {"windows-x86_64": {"url": "u-win", "sha256": "s-win", "signature": "sig-win"}}}
EOF
echo zip > "$GH_STATE/assets/Trek-0.4.61-windows-x86_64.zip"
passes "a release a failed create left (a draft, no Mac files) is finished" "$ROOT" github 0.4.61 "$out/release-notes.md" "${assets[@]}"
same "the Mac's files are there" "$(find "$GH_STATE/assets" -name '*darwin-aarch64*' | wc -l | tr -d ' ')" "3"
same "the manifest has both platforms" "$(python3 -I -c 'import json,sys; print(",".join(sorted(json.load(open(sys.argv[1]))["platforms"])))' "$GH_STATE/assets/stable.json")" "darwin-aarch64,windows-x86_64"
same "Windows' entry is as it was" "$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["platforms"]["windows-x86_64"]["sha256"])' "$GH_STATE/assets/stable.json")" "s-win"
same "the release's notes stay" "$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["notes"])' "$GH_STATE/assets/stable.json")" "the release's notes"
same "the manifest went up after the files it points at" "$(grep '^release upload' "$GH_STATE/log" | tail -1 | grep -c 'stable.json$')" "1"
same "and the draft is published" "$(grep -c '^release edit v0.4.61 .*--draft=false' "$GH_STATE/log")" "1"
gh_reset
touch "$GH_STATE/create-fails"
refuses "a create that keeps failing fails, after three tries" "$ROOT" github 0.4.61 "$out/release-notes.md" "${assets[@]}"
same "three tries" "$(grep -c '^release create' "$GH_STATE/log")" "3"

echo "# rehearse: a dry run's push, on a copy"
dev=$(fresh rehearse)
P=$(at "$dev" HEAD)
bump_tree "$dev" 0.4.61
passes "the bump in the working tree is committed, checked and pushed on a copy" "$dev" rehearse 0.4.61 main
said "it says what it did" "rehearsed the push on a copy"
same "the repository's HEAD hasn't moved" "$(at "$dev" HEAD)" "$P"
same "it has no tag" "$(at "$dev" v0.4.61)" "none"
same "and origin is untouched" "$(at "$WORK/rehearse/origin.git" main) $(at "$WORK/rehearse/origin.git" v0.4.61)" "$P none"
sed -i.bak 's/^checksum = "1111"$/checksum = "3333"/' "$dev/Cargo.lock"
rm -f "$dev/Cargo.lock.bak"
refuses "a build that changed more of Cargo.lock than the versions fails the dry run" "$dev" rehearse 0.4.61 main

echo
if [ "$fails" -eq 0 ]; then echo "release-finish: all passed"; else echo "release-finish: $fails failed"; exit 1; fi
