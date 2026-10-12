#!/usr/bin/env bash
# Tests for script/lib/notes.sh, the notes script/release.sh writes. Each case runs
# trek_write_notes in a throwaway git repository (or on a --notes file) and diffs what it wrote
# against the fixtures in script/tests/fixtures/notes/. Runs on the Mac and under Git Bash:
#
#   bash script/tests/release-notes.sh
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
FIX="$ROOT/script/tests/fixtures/notes"
# shellcheck source=script/lib/notes.sh
. "$ROOT/script/lib/notes.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
fails=0

check() { # actual, expected, what
  if diff -u "$2" "$1" > "$WORK/diff"; then
    echo "ok    $3"
  else
    echo "FAIL  $3"
    cat "$WORK/diff"
    fails=$((fails + 1))
  fi
}

# A repository with two release tags and a "Release v" commit, which the notes leave out. The
# dates are fixed and distinct: the tags sort by when their commits were made.
REPO="$WORK/repo"
git init -q "$REPO"
cd "$REPO"
git config user.name "release notes test"
git config user.email "test@example.invalid"
commit() { # subject, date
  GIT_AUTHOR_DATE="$2" GIT_COMMITTER_DATE="$2" git -c commit.gpgsign=false commit -q --allow-empty -m "$1"
}
commit "Old change" 2026-10-01T10:00:00Z
git tag v0.1.0
commit "Fix the thing" 2026-10-02T10:00:00Z
git tag v0.2.0-beta.1
commit "Release v0.2.0" 2026-10-03T10:00:00Z
commit "Add the other thing" 2026-10-04T10:00:00Z
commit "Tidy the notes" 2026-10-05T10:00:00Z

# Beta: the commits since the newest tag, v0.2.0-beta.1.
out="$WORK/auto-beta"; mkdir "$out"
trek_write_notes "" 0.2.0-beta.2 beta "$out"
check "$out/notes.md" "$FIX/auto-beta.notes.md" "auto notes, beta: since the last tag, release commit left out"

# Stable: the newest tag that isn't a prerelease, so the beta's commits are in too.
out="$WORK/auto-stable"; mkdir "$out"
trek_write_notes "" 0.2.0 stable "$out"
check "$out/notes.md" "$FIX/auto-stable.notes.md" "auto notes, stable: since v0.1.0, the beta's commit included"

# A --notes file goes into notes.md unchanged.
out="$WORK/notes-file"; mkdir "$out"
trek_write_notes "$FIX/notes-without-block.md" 0.2.0-beta.2 beta "$out"
check "$out/notes.md" "$FIX/notes-without-block.md" "--notes file: copied as it is"

# Nothing since the last tag: the maintenance line.
git tag v0.2.1
out="$WORK/maintenance"; mkdir "$out"
trek_write_notes "" 0.2.1-beta.1 beta "$out"
check "$out/notes.md" "$FIX/maintenance.notes.md" "auto notes, nothing new: the maintenance line"

if [ "$fails" -ne 0 ]; then
  echo "$fails failed"
  exit 1
fi
echo "all passed"
