#!/usr/bin/env bash
# The irreversible end of a stable release, and the release workflow's checks of it (script/release.sh,
# .github/workflows/release.yml; docs/RELEASING.md, "When a release run fails"). Tests:
# script/tests/release-finish.sh.
#
#   release-finish.sh is-bump <commit> <version>
#       Is <commit> "Release v<version>" made by release.sh: one parent, and nothing changed but the
#       workspace version (Cargo.toml's [workspace.package] line and Cargo.lock's version lines, old
#       version to <version>)? Proved from the diff, not the message. Says why on stdout (yes, exit 0)
#       or stderr (no, exit 1).
#   release-finish.sh adopt <version> <branch>
#       For a resumed release: when origin's <branch> is such a bump of the checked-out HEAD, move the
#       branch up to it, so the run carries on from the bump an earlier run (or a person) pushed instead
#       of making a second one. Anything else on origin is left for the push to refuse.
#   release-finish.sh push <version> <branch>
#       Tag HEAD v<version> (unless it is) and push HEAD to <branch> and the tag in one atomic push:
#       both land or neither does. Retried when refused, re-reading origin each time; done when origin
#       has both on HEAD, whoever pushed them. A tag on another commit, or a branch that has moved on,
#       is refused at once.
#   release-finish.sh github <version> <notes file> <asset>...
#       Create the stable GitHub release v<version> with the assets (the <channel>.json among them), or
#       finish one an earlier run started: upload this platform's files and merge its manifest entry
#       when they aren't all there, leave them when they are, and publish a draft.
#   release-finish.sh state <version> <channel> <start commit> <run id> [<branch>]
#       What a failed run left on origin and GitHub, and the command that resumes it, as Markdown.
#   release-finish.sh rehearse <version> <branch>
#       For a dry run, with release.sh's bump in the working tree: commit it as release.sh would, in a
#       throwaway copy of this repository, check it with is-bump, and push it there twice with push.
#
# Environment: TREK_PUSH_ATTEMPTS (pushes before giving up, default 4), TREK_PUSH_DELAY (seconds
# before the first retry, doubling each time, default 5), TREK_RELEASE_REPO (default dokyit/Trek).
set -euo pipefail

die() { echo "release-finish: $*" >&2; exit 1; }
SEMVER='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'

# The [workspace.package] version in <rev>'s Cargo.toml (release.sh reads and sets the same line).
workspace_version() {
  git show "$1:Cargo.toml" 2>/dev/null | sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' | head -n 1
}

# Why a commit isn't a bump (the caller returns 1).
no() { echo "$*" >&2; }

is_bump() {
  local version=${2#v} commit parent old new subject raw line added count
  [[ $version =~ $SEMVER ]] || { no "$version is not a version"; return 1; }
  commit=$(git rev-parse -q --verify "$1^{commit}") || { no "no commit $1"; return 1; }
  # One parent, and it's here (the workflow's gate checks out two commits deep).
  [[ $(git rev-list --parents -n 1 "$commit" | wc -w | tr -d ' ') == 2 ]] || { no "${commit:0:8} isn't a commit with one parent"; return 1; }
  parent=$(git rev-parse "$commit^1")
  git cat-file -e "$parent^{commit}" 2>/dev/null || { no "${commit:0:8}'s parent ${parent:0:8} isn't fetched"; return 1; }
  subject=$(git log -1 --format=%s "$commit")
  [[ $subject == "Release v$version" ]] || { no "${commit:0:8} is \"$subject\", not \"Release v$version\""; return 1; }
  old=$(workspace_version "$parent")
  new=$(workspace_version "$commit")
  [[ -n $old ]] || { no "${parent:0:8} has no [workspace.package] version"; return 1; }
  [[ $new == "$version" ]] || { no "${commit:0:8} sets version ${new:-(none)}, not $version"; return 1; }
  [[ $old != "$new" ]] || { no "${commit:0:8} doesn't change the version (it was $old already)"; return 1; }
  # Only Cargo.toml and Cargo.lock, modified in place (no new, deleted or renamed file, no mode
  # change), and Cargo.toml among them.
  raw=$(git diff --raw --no-renames --no-abbrev "$parent" "$commit")
  [[ -n $raw ]] || { no "${commit:0:8} changes nothing"; return 1; }
  while IFS= read -r line; do
    case $line in
      ":100644 100644 "*$'\t'Cargo.toml | ":100644 100644 "*$'\t'Cargo.lock) ;;
      *) no "${commit:0:8} changes more than the version: ${line##*$'\t'} (${line%%$'\t'*})"; return 1 ;;
    esac
  done <<< "$raw"
  grep -q $'\tCargo.toml$' <<< "$raw" || { no "${commit:0:8} doesn't change Cargo.toml"; return 1; }
  # Cargo.toml is exactly the parent's with release.sh's own edit: the [workspace.package] line.
  if ! cmp -s <(git show "$commit:Cargo.toml") \
      <(git show "$parent:Cargo.toml" | sed '/^\[workspace\.package\]/,/^\[/s/^version = ".*"/version = "'"$version"'"/'); then
    no "${commit:0:8}'s Cargo.toml changes more than the [workspace.package] version"
    return 1
  fi
  # Every changed line of both files is a version line, $old before and $version after, pair for pair.
  # (Cargo.lock's are the workspace's own packages: a dependency that moved would carry other lines,
  # its checksum, or another old version.)
  if ! count=$(git diff -U0 --no-color --no-ext-diff --no-renames "$parent" "$commit" -- Cargo.toml Cargo.lock | awk \
      -v old="version = \"$old\"" -v new="version = \"$version\"" '
        /^diff --git / { hunk = 0; next }
        /^@@/ { hunk = 1; next }
        !hunk || /^\\/ { next }
        /^-/ { minus++; if (substr($0, 2) != old) bad = $0; next }
        /^\+/ { plus++; if (substr($0, 2) != new) bad = $0; next }
        END {
          if (bad != "") { print "a changed line that isn'\''t the version: " bad; exit 1 }
          if (minus != plus || minus == 0) { print minus + 0 " version lines removed, " plus + 0 " added"; exit 1 }
          print minus
        }'); then
    no "${commit:0:8}: $count"
    return 1
  fi
  # And each of Cargo.lock's is in one of the workspace's own [[package]] entries (no source line),
  # not a dependency that happens to have had the same version.
  added=$(git diff -U0 --no-color --no-ext-diff --no-renames "$parent" "$commit" -- Cargo.lock | awk '
    /^diff --git / { hunk = 0; next }
    /^@@/ { hunk = 1; split($3, a, ","); n = substr(a[1], 2) + 0; next }
    hunk && /^\+/ { print n; n++ }' | tr '\n' ' ')
  if [[ -n ${added// /} ]] && ! line=$(git show "$commit:Cargo.lock" | awk -v lines="$added" '
      BEGIN { k = split(lines, l, " "); for (i = 1; i <= k; i++) want[l[i]] = 1 }
      /^\[\[package\]\]/ { blk++ }
      { at[NR] = blk }
      /^name = / { name[blk] = $3 }
      /^source = / { src[blk] = 1 }
      END { for (n in want) if (src[at[n]]) { print "Cargo.lock moves " name[at[n]] ", a dependency, not one of the workspace'\''s packages"; exit 1 } }'); then
    no "${commit:0:8}: $line"
    return 1
  fi
  echo "${commit:0:8} \"Release v$version\" is its parent ${parent:0:8} with only the version changed ($old → $version, $count lines in Cargo.toml and Cargo.lock)"
}

# Origin's <branch> tip and the commit its tag v<version> is on (peeled; empty when absent) into
# REMOTE_BRANCH and REMOTE_TAG. A failed ls-remote leaves both empty.
read_origin() {
  local branch=$1 tag=$2 out sha ref tag_sha="" peeled=""
  REMOTE_BRANCH=""
  out=$(git ls-remote origin "refs/heads/$branch" "refs/tags/$tag" "refs/tags/$tag^{}" 2>/dev/null) || out=""
  while IFS=$'\t' read -r sha ref; do
    case $ref in
      "refs/heads/$branch") REMOTE_BRANCH=$sha ;;
      "refs/tags/$tag") tag_sha=$sha ;;
      "refs/tags/$tag^{}") peeled=$sha ;;
    esac
  done <<< "$out"
  # A lightweight tag (pushed by hand) has no peeled line: it names the commit itself.
  REMOTE_TAG=${peeled:-$tag_sha}
}

adopt() {
  local version=${1#v} branch=$2 head remote
  [[ $(git symbolic-ref -q --short HEAD) == "$branch" ]] || die "adopt: HEAD isn't on $branch"
  head=$(git rev-parse HEAD)
  if [[ -z $(git ls-remote origin "refs/heads/$branch") ]]; then
    echo "• origin has no $branch yet"
    return 0
  fi
  git fetch -q origin "+refs/heads/$branch:refs/remotes/origin/$branch" || die "couldn't fetch $branch from origin"
  remote=$(git rev-parse "refs/remotes/origin/$branch")
  if [[ $remote == "$head" ]]; then
    echo "• origin's $branch is this commit (${head:0:8})"
  elif [[ $(git rev-parse -q --verify "$remote^1" || true) == "$head" ]] && is_bump "$remote" "$version" > /dev/null 2>&1; then
    git merge -q --ff-only "$remote"
    echo "• resuming: origin's $branch already has Release v$version on this commit (${remote:0:8}); carrying on from it"
  else
    echo "• origin's $branch is at ${remote:0:8}, not this commit (${head:0:8}) or its Release v$version"
  fi
}

push() {
  local version=${1#v} branch=$2 tag head attempts delay i=0 refs has
  tag=v$version
  head=$(git rev-parse HEAD)
  if git rev-parse -q --verify "refs/tags/$tag" > /dev/null; then
    [[ $(git rev-parse "refs/tags/$tag^{commit}") == "$head" ]] || die "tag $tag is on $(git rev-parse --short "refs/tags/$tag^{commit}"), not HEAD (${head:0:8})"
  else
    git tag -a "$tag" -m "Trek $version"
  fi
  attempts=${TREK_PUSH_ATTEMPTS:-4}
  delay=${TREK_PUSH_DELAY:-5}
  while :; do
    read_origin "$branch" "$tag"
    if [[ -n $REMOTE_TAG && $REMOTE_TAG != "$head" ]]; then
      die "origin's $tag is on ${REMOTE_TAG:0:8}, not ${head:0:8}: taken by another commit"
    fi
    # Whether origin's branch has HEAD already: it is HEAD, or HEAD is under work pushed on top of
    # it since (an earlier run's bump landed without its tag). A branch that has neither HEAD nor
    # only HEAD's ancestors has moved on, and nothing is pushed.
    has=0
    if [[ $REMOTE_BRANCH == "$head" ]]; then
      has=1
    elif [[ -n $REMOTE_BRANCH ]]; then
      git cat-file -e "$REMOTE_BRANCH^{commit}" 2> /dev/null || git fetch -q origin "refs/heads/$branch" 2> /dev/null || true
      if git merge-base --is-ancestor "$head" "$REMOTE_BRANCH" 2> /dev/null; then
        has=1
      elif git cat-file -e "$REMOTE_BRANCH^{commit}" 2> /dev/null && ! git merge-base --is-ancestor "$REMOTE_BRANCH" "$head"; then
        die "origin's $branch has moved on to ${REMOTE_BRANCH:0:8}, which doesn't have ${head:0:8}: nothing pushed (fetch first)"
      fi
    fi
    if (( has )) && [[ $REMOTE_TAG == "$head" ]]; then
      echo "• origin has $tag on ${head:0:8}, and its $branch has that commit"
      return 0
    fi
    (( i < attempts )) || die "gave up after $attempts pushes: origin's $branch is ${REMOTE_BRANCH:-unknown}, $tag is ${REMOTE_TAG:-not there}"
    if (( i > 0 )); then
      echo "• the push was refused (attempt $i of $attempts); trying again in ${delay}s"
      sleep "$delay"
      delay=$((delay * 2))
    fi
    i=$((i + 1))
    refs=()
    (( has )) || refs+=("HEAD:refs/heads/$branch")
    [[ $REMOTE_TAG == "$head" ]] || refs+=("refs/tags/$tag")
    (( has )) && echo "• origin's $branch has ${head:0:8} already; pushing $tag"
    # One push, one pack: the commit and the tag that points at it travel together (a tag pushed on
    # its own leans on origin already having the commit, and v0.4.11 and v0.4.61 were refused that
    # way), and with --atomic origin takes both refs or neither, so the branch never has the bump
    # without its tag.
    git push --atomic origin "${refs[@]}" || true
  done
}

github() {
  local version=${1#v} notes=$2 repo tag manifest="" platform asset info draft names have tmp i
  shift 2
  repo=${TREK_RELEASE_REPO:-dokyit/Trek}
  tag=v$version
  local files=()
  for asset in "$@"; do
    if [[ $asset == *.json ]]; then manifest=$asset; else files+=("$asset"); fi
  done
  [[ -n $manifest && ${#files[@]} -gt 0 ]] || die "github: the assets are the archive, its signature and disk image, and the <channel>.json"
  platform=$(python3 -I -c 'import json, sys; print(" ".join(json.load(open(sys.argv[1]))["platforms"]))' "$manifest")
  for i in 1 2 3; do
    if info=$(gh release view "$tag" --repo "$repo" --json isDraft,assets --jq '[(.isDraft | tostring), ([.assets[].name] | join("\n"))] | join("\n")' 2> /dev/null); then
      break
    fi
    info=""
    if gh release create "$tag" --repo "$repo" --verify-tag --latest \
        --title "Trek $version" --notes-file "$notes" "$@"; then
      return 0
    fi
    # A create that failed half way may have left the release (a draft, some assets): finish it.
    echo "• creating release $tag failed (attempt $i of 3)"
    sleep "${TREK_PUSH_DELAY:-5}"
  done
  [[ -n $info ]] || die "couldn't create release $tag"
  draft=${info%%$'\n'*}
  names=${info#*$'\n'}
  echo "• release $tag exists already: finishing it"
  tmp=$(mktemp -d)
  have=1
  for asset in "${files[@]}"; do
    grep -qxF "${asset##*/}" <<< "$names" || have=0
  done
  if grep -qxF "${manifest##*/}" <<< "$names" \
      && gh release download "$tag" --repo "$repo" --pattern "${manifest##*/}" --output "$tmp/release.json" --clobber; then
    python3 -I -c 'import json, sys; m = json.load(open(sys.argv[1])); sys.exit(0 if m.get("version") == sys.argv[2] and sys.argv[3] in m.get("platforms", {}) else 1)' \
      "$tmp/release.json" "$version" "$platform" || have=0
  else
    rm -f "$tmp/release.json"
    have=0
  fi
  if (( have )); then
    # What users may already be downloading stays as it is: a rebuild is the same version, other bytes.
    echo "• $tag already has $platform's files and manifest entry: left as they are"
  else
    # This platform's files as one set (archive, signature, disk image), then the manifest that
    # points at them, merged over what the other platforms put there.
    gh release upload "$tag" --repo "$repo" --clobber "${files[@]}"
    mkdir -p "$tmp/out"
    python3 -I -c '
import json, os, sys
ours = json.load(open(sys.argv[1]))
theirs = json.load(open(sys.argv[2])) if os.path.exists(sys.argv[2]) else {}
if theirs.get("version") == ours["version"]:
    ours = {**theirs, "platforms": {**theirs.get("platforms", {}), **ours["platforms"]}}
print(json.dumps(ours, indent=2, ensure_ascii=False))
' "$manifest" "$tmp/release.json" > "$tmp/out/${manifest##*/}"
    gh release upload "$tag" --repo "$repo" --clobber "$tmp/out/${manifest##*/}"
    echo "• uploaded $platform's files and merged its entry into ${manifest##*/}"
  fi
  if [[ $draft == true ]]; then
    gh release edit "$tag" --repo "$repo" --draft=false --latest
    echo "• published the draft $tag"
  fi
  rm -rf "$tmp"
}

state() {
  local version=${1#v} channel=$2 start=$3 run=$4 branch=${5:-main} tag repo moved=0 release
  repo=${TREK_RELEASE_REPO:-dokyit/Trek}
  tag=v$version
  [[ $channel == stable ]] || tag=$channel
  echo "### What this run left"
  echo
  if [[ $channel == stable ]]; then
    read_origin "$branch" "$tag"
    git fetch -q origin "+refs/heads/$branch:refs/remotes/origin/$branch" 2> /dev/null || true
    if [[ -z $REMOTE_BRANCH ]]; then
      echo "- origin's \`$branch\`: couldn't be read"
    elif [[ $REMOTE_BRANCH == "$start" ]]; then
      echo "- origin's \`$branch\`: still the commit this run started from (\`${start:0:8}\`)$( [[ $(git log -1 --format=%s "$start" 2> /dev/null) == "Release v$version" ]] && echo ", its Release v$version" || echo "; no version bump was pushed")"
    elif [[ $(git rev-parse -q --verify "$REMOTE_BRANCH^1" 2> /dev/null || true) == "$start" ]] && is_bump "$REMOTE_BRANCH" "$version" > /dev/null 2>&1; then
      echo "- origin's \`$branch\`: Release v$version (\`${REMOTE_BRANCH:0:8}\`) on the commit this run started from"
    else
      moved=1
      echo "- origin's \`$branch\`: moved on to \`${REMOTE_BRANCH:0:8}\` since this run started at \`${start:0:8}\`"
    fi
    if [[ -z $REMOTE_TAG ]]; then
      echo "- tag \`$tag\`: not on origin"
    else
      echo "- tag \`$tag\`: on \`${REMOTE_TAG:0:8}\`"
    fi
  fi
  if release=$(gh release view "$tag" --repo "$repo" --json isDraft,assets --jq '"\(.assets | length) assets" + (if .isDraft then ", a draft" else "" end)' 2> /dev/null); then
    echo "- GitHub release \`$tag\`: exists ($release)"
  else
    echo "- GitHub release \`$tag\`: not there"
  fi
  echo
  echo "### To resume"
  echo
  if (( moved )); then
    echo "\`$branch\` has commits this run doesn't, so it can't be pushed on: wait for CI on the new head, then dispatch again (with the same notes):"
    echo
    echo '```sh'
    echo "gh workflow run release.yml --ref $branch -f version=$version -f channel=$channel -F notes=@notes.md"
    echo '```'
  else
    echo "Re-run the failed jobs. release.sh builds again and carries on from what is on origin: a pushed Release v$version and its tag are used, not made twice; whatever is missing goes in one atomic push; a release that exists is finished without replacing what it has."
    echo
    echo '```sh'
    echo "gh run rerun $run --failed"
    echo '```'
  fi
}

rehearse() {
  local version=${1#v} branch=$2 top base current built tmp commit tree toml lock
  top=$(git rev-parse --show-toplevel)
  base=$(git rev-parse HEAD)
  current=$(workspace_version "$base")
  built=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$top/Cargo.toml" | head -n 1)
  [[ $built == "$version" ]] || die "rehearse: Cargo.toml says ${built:-no version}, not $version (run it while release.sh's bump is in place)"
  tmp=$(mktemp -d)
  # shellcheck disable=SC2064 # $tmp is meant to be expanded now
  trap "rm -rf '$tmp'" EXIT
  # origin.git stands in for GitHub: this repository as it is, with <branch> at HEAD. work.git is
  # the runner's clone of it.
  git clone -q --bare "$(git rev-parse --path-format=absolute --git-common-dir)" "$tmp/origin.git"
  GIT_DIR=$tmp/origin.git git update-ref "refs/heads/$branch" "$base"
  git clone -q --bare "$tmp/origin.git" "$tmp/work.git"
  # Whoever this repository commits as (the workflow sets it in the checkout's own config).
  GIT_AUTHOR_NAME=$(git config user.name || echo 'Trek release rehearsal')
  GIT_AUTHOR_EMAIL=$(git config user.email || echo 'rehearsal@localhost')
  export GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME=$GIT_AUTHOR_NAME GIT_COMMITTER_EMAIL=$GIT_AUTHOR_EMAIL
  (
    export GIT_DIR=$tmp/work.git
    git symbolic-ref HEAD "refs/heads/$branch"
    commit=$base
    if [[ $current != "$version" ]]; then
      # release.sh's commit: HEAD's tree with the bumped Cargo.toml and Cargo.lock it built with.
      toml=$(git hash-object -w "$top/Cargo.toml")
      lock=$(git hash-object -w "$top/Cargo.lock")
      GIT_INDEX_FILE=$tmp/index git read-tree "$base"
      GIT_INDEX_FILE=$tmp/index git update-index --cacheinfo "100644,$toml,Cargo.toml" --cacheinfo "100644,$lock,Cargo.lock"
      tree=$(GIT_INDEX_FILE=$tmp/index git write-tree)
      commit=$(git commit-tree "$tree" -p "$base" -m "Release v$version")
      git update-ref "refs/heads/$branch" "$commit" "$base"
      # What the release workflow's gate would say of it, were this run resumed on that commit.
      is_bump "$commit" "$version" || die "rehearse: the release commit isn't only the version bump, so the gate wouldn't take CI on its parent for a resumed run"
    fi
    TREK_PUSH_DELAY=${TREK_PUSH_DELAY:-1} push "$version" "$branch"
    # And again, as a resumed run would find it.
    TREK_PUSH_DELAY=${TREK_PUSH_DELAY:-1} push "$version" "$branch"
    [[ $(GIT_DIR=$tmp/origin.git git rev-parse "refs/heads/$branch") == "$commit" &&
       $(GIT_DIR=$tmp/origin.git git rev-parse "refs/tags/v$version^{commit}") == "$commit" ]] \
      || die "rehearse: the stand-in origin doesn't have $branch and v$version on ${commit:0:8}"
    echo "• rehearsed the push on a copy: $branch and v$version on ${commit:0:8} in one atomic push; a second push found them there"
  )
}

cmd=${1:-}
[[ -n $cmd ]] || { awk 'NR > 1 && !/^#/ { exit } NR > 1 { sub(/^# ?/, ""); print }' "$0"; exit 2; }
shift
case $cmd in
  is-bump) (( $# == 2 )) || die "is-bump <commit> <version>"; is_bump "$@" ;;
  adopt) (( $# == 2 )) || die "adopt <version> <branch>"; adopt "$@" ;;
  push) (( $# == 2 )) || die "push <version> <branch>"; push "$@" ;;
  github) (( $# >= 3 )) || die "github <version> <notes file> <asset>..."; github "$@" ;;
  state) (( $# >= 4 )) || die "state <version> <channel> <start commit> <run id> [<branch>]"; state "$@" ;;
  rehearse) (( $# == 2 )) || die "rehearse <version> <branch>"; rehearse "$@" ;;
  *) die "unknown command $cmd" ;;
esac
