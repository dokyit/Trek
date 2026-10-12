# Release notes for script/release.sh (POSIX sh: sourced by the zsh script, and by
# script/tests/release-notes.sh).
#
#   trek_write_notes <notes file, or "" for the git log> <version> <channel> <out dir>
#
# Writes <out dir>/notes.md: what changed in the release. That is the notes file as it is, else the
# commit subjects since the channel's last tag, else "- Maintenance release." when there are none.
# The manifest's "notes" come from this file, and Trek shows them under "What's new" to someone who
# has already installed.
trek_write_notes() {
  notes_file=$1
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
}
