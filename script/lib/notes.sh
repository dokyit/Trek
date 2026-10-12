# Release notes for script/release.sh (POSIX sh: sourced by the zsh script, and by
# script/tests/release-notes.sh).
#
#   trek_write_notes <notes file, or "" for the git log> <version> <channel> <out dir>
#
# Writes two files into <out dir>:
#   notes.md          what changed in the release: the notes file as it is, else the commit subjects
#                     since the channel's last tag, else "- Maintenance release.". The manifest's
#                     "notes" come from this file, and Trek shows them under "What's new" to someone
#                     who has already installed.
#   release-notes.md  notes.md, a blank line and the install block: the GitHub release's body. It is
#                     written whole on every publish (a beta's moving release included), so the block
#                     is never there twice. Every release carries it, so whoever writes the notes
#                     doesn't have to remember the Windows folder rule. A notes file that already has
#                     an install block keeps its own.

# The install block, one bullet per platform. Its wording lives only here: README.md's Install
# sections say the same things, so change them together (docs/RELEASING.md).
trek_install_block() {
  # The quoted heredoc keeps the backticks and the backslashes of the Windows paths as they are.
  cat <<'EOF' | sed "s/@VERSION@/$1/g"
## Install

- **macOS**: `Trek-@VERSION@-darwin-aarch64.app.tar.gz`. Unpack it and move **Trek.app** to Applications (macOS 13 or later, Apple silicon). Apple hasn't notarized it, so the first launch takes one extra step: right-click Trek.app, then **Open**, then **Open** again.
- **Windows**: `Trek-@VERSION@-windows-x86_64.zip`. Unzip it into a folder of its own that Windows doesn't guard, such as `C:\Users\<you>\Trek` or `%LOCALAPPDATA%\Programs\Trek` (not Documents, Desktop, Downloads or Program Files), and run `trek.exe`. SmartScreen warns once: **More info**, then **Run anyway**. Updates install in place from Settings → Updates.
EOF
}

trek_write_notes() {
  notes_file=$1
  notes_version=$2
  notes_channel=$3
  notes_out=$4
  if [ -n "$notes_file" ]; then
    cp "$notes_file" "$notes_out/notes.md"
  else
    # The most recent tag of the channel's line, by when it was made: git's version sort would put
    # 0.3.0-beta.1 after 0.3.0. Stable skips prereleases; nightly starts from its own tag.
    case $notes_channel in
      stable) notes_since=$(git tag --list 'v[0-9]*' --sort=-creatordate | grep -v -- - | head -1 || true) ;;
      nightly)
        if git rev-parse -q --verify refs/tags/nightly >/dev/null; then notes_since=nightly
        else notes_since=$(git tag --list 'v[0-9]*' --sort=-creatordate | head -1 || true); fi ;;
      *) notes_since=$(git tag --list 'v[0-9]*' --sort=-creatordate | head -1 || true) ;;
    esac
    notes_range=HEAD
    [ -z "$notes_since" ] || notes_range="$notes_since..HEAD"
    git log --no-merges --invert-grep --grep='^Release v' --format='- %s' "$notes_range" > "$notes_out/notes.md"
    [ -s "$notes_out/notes.md" ] || echo "- Maintenance release." > "$notes_out/notes.md"
  fi

  if grep -q -E '^(## Install|\*\*Windows\*\*:)' "$notes_out/notes.md"; then
    cp "$notes_out/notes.md" "$notes_out/release-notes.md"
    echo "• release-notes.md: the notes file's own install block kept, none added"
  else
    {
      cat "$notes_out/notes.md"
      # A last line without its newline gets one, so the blank line below is a real blank line.
      [ -z "$(tail -c 1 "$notes_out/notes.md")" ] || echo
      echo
      trek_install_block "$notes_version"
    } > "$notes_out/release-notes.md"
    echo "• release-notes.md: the notes and the install block"
  fi
}
