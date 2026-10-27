#!/usr/bin/env python3
"""The Rust build cache of a CI job (.github/workflows/ci.yml): its key, and what it keeps.

    python3 script/ci-cache.py key KIND   the cache key and restore prefix, to $GITHUB_OUTPUT
    python3 script/ci-cache.py restore    right after the cache restore, before any cargo command
    python3 script/ci-cache.py prune      before saving

What's kept: the registry and target/ with the dependencies' artifacts, crates.io's and the
vendored crates' (vendor/: gpui-base and gpui-pre*, which every gpui-* crate from crates.io is
built on). Not the workspace's own crates: every release bumps their version, so the next push
compiles them all anyway, and their test executables (~120 MB each on Windows) would make the
cache several times bigger. (Swatinem/rust-cache can't do this: it deletes the artifacts of path
dependencies that aren't workspace members, which vendor/'s are.)

key: KIND (one per kind of job: each builds its own feature set), the runner, rustc, the
dependencies in Cargo.lock (not the workspace's versions, so a release keeps the key), vendor/'s
tree, and the CARGO_* / RUST* environment (a linker, say). A key is saved once; a job whose key
isn't saved yet restores the newest one with the same KIND, runner and rustc and saves its own.

restore: Cargo decides whether a path crate is fresh by mtime, and a checkout stamps every file
with the checkout's time, so after a restore the vendored crates (and all of gpui on top of them)
compiled again although their artifacts were right there. This reads the list of git blobs of
vendor/ the cached artifacts were built from (target/trek-ci/vendor-blobs, written by the job that
saved the cache) and moves each vendored file whose blob is the same now back to 2000-01-01, older
than any artifact; a file that changed, or is new, keeps the checkout's time, so Cargo rebuilds
what it would from scratch. Then it writes the list for this checkout, which the cache saves with
the artifacts. That holds because each key belongs to one kind of job, every such job builds all
of its vendored units, and a cache is saved only after the job passed. With no list (a cold
cache) every vendored file is stamped, and any vendored fingerprints dropped, so what compiles now
is reusable next time: a build script that doesn't name its inputs makes Cargo record the newest
mtime in its package, and that has to be the same next time.

prune: removes the workspace members' artifacts (libs, test and other executables, build-script
output, fingerprints) from target/<profile>/.
"""
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys

OLD = 946684800  # 2000-01-01T00:00:00Z
MANIFEST = os.path.join('trek-ci', 'vendor-blobs')
STAMPED = ('vendor/',)


def run(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True).stdout


def digest(*parts):
    h = hashlib.sha256()
    for p in parts:
        h.update(p if isinstance(p, bytes) else p.encode())
        h.update(b'\0')
    return h.hexdigest()[:12]


def key(kind):
    runner = f"{os.environ.get('RUNNER_OS', platform.system())}-{os.environ.get('RUNNER_ARCH', platform.machine())}"
    rustc = digest(run('rustc', '-vV'))
    # Dependencies only: a [[package]] without `source` is the workspace's own (or vendor/'s, which
    # the tree hash covers).
    lock = open('Cargo.lock', encoding='utf-8').read().split('[[package]]')
    deps = digest(*[p.strip() for p in lock if '\nsource = ' in p])
    vendor = run('git', 'rev-parse', 'HEAD:vendor').strip()
    env = digest(*sorted(f'{k}={v}' for k, v in os.environ.items()
                         if (k.startswith('CARGO') or k.startswith('RUST'))
                         and k not in ('CARGO_HOME', 'RUSTUP_HOME', 'CARGO_TERM_COLOR')))
    prefix = f'rust-{kind}-{runner}-{rustc}-'
    lines = [f'key={prefix}{digest(deps, vendor, env)}', f'restore={prefix}']
    out = os.environ.get('GITHUB_OUTPUT')
    if out:
        with open(out, 'a', encoding='utf-8') as f:
            f.write('\n'.join(lines) + '\n')
    print('\n'.join(lines))


def tracked_blobs():
    """{path: blob sha} for tracked files under STAMPED; files edited since checkout are 'dirty'."""
    entries = {}
    for rec in run('git', 'ls-files', '-s', '-z', '--', *STAMPED).decode('utf-8', 'surrogateescape').split('\0'):
        if rec:
            meta, path = rec.split('\t', 1)
            entries[path] = meta.split(' ')[1]
    for path in run('git', 'diff', '--name-only', '-z', 'HEAD', '--', *STAMPED).decode('utf-8', 'surrogateescape').split('\0'):
        if path in entries:
            entries[path] = 'dirty'
    return entries


def vendored_packages():
    """The package names under STAMPED (vendor/<dir>/Cargo.toml's [package] name)."""
    names = set()
    for top in STAMPED:
        for d in sorted(os.listdir(top)) if os.path.isdir(top) else []:
            manifest = os.path.join(top, d, 'Cargo.toml')
            if not os.path.isfile(manifest):
                continue
            section = None
            for line in open(manifest, encoding='utf-8'):
                line = line.strip()
                if line.startswith('['):
                    section = line
                elif section == '[package]':
                    m = re.match(r'name\s*=\s*"([^"]+)"', line)
                    if m:
                        names.add(m.group(1))
                        break
    return names


HASHED = re.compile(r'^(.*)-([0-9a-f]{16})(\..*)?$')


def remove(path):
    if os.path.isdir(path) and not os.path.islink(path):
        shutil.rmtree(path, ignore_errors=True)
    else:
        try:
            os.remove(path)
        except OSError:
            return 0
    return 1


def forget(target, packages):
    """Remove these packages' units from target/<profile>/: fingerprints, build-script output, and
    the files in deps/ that carry the same hash (matched by hash, not name: a crates.io crate can
    share a name with one of our test targets, `windows`)."""
    removed = 0
    for profile in ('debug', 'release'):
        root = os.path.join(target, profile)
        ours = set()
        for sub in ('.fingerprint', 'build'):
            d = os.path.join(root, sub)
            for entry in (os.listdir(d) if os.path.isdir(d) else []):
                m = HASHED.match(entry)
                if m and m.group(1) in packages and not m.group(3):
                    ours.add(m.group(2))
                    removed += remove(os.path.join(d, entry))
        d = os.path.join(root, 'deps')
        for entry in (os.listdir(d) if os.path.isdir(d) else []):
            m = HASHED.match(entry)
            if m and m.group(2) in ours:
                removed += remove(os.path.join(d, entry))
    return removed


def restore(target):
    manifest = os.path.join(target, MANIFEST)
    now = tracked_blobs()
    before = {}
    if os.path.exists(manifest):
        with open(manifest, encoding='utf-8', errors='surrogateescape') as f:
            for line in f:
                sha, _, path = line.rstrip('\n').partition(' ')
                before[path] = sha
    forgotten = None if before else forget(target, vendored_packages())
    same = 0
    for path, sha in now.items():
        if sha != 'dirty' and (forgotten is not None or before.get(path) == sha):
            try:
                os.utime(path, (OLD, OLD))
                same += 1
            except OSError:
                pass  # left with the checkout's time: a rebuild, never a stale artifact
    os.makedirs(os.path.dirname(manifest), exist_ok=True)
    with open(manifest, 'w', encoding='utf-8', errors='surrogateescape', newline='\n') as f:
        for path, sha in sorted(now.items()):
            f.write(f'{sha} {path}\n')
    if forgotten is None:
        print(f'ci-cache: {same} of {len(now)} vendored files unchanged since the cached build')
    else:
        print(f'ci-cache: no list of what vendor/ was built from: {forgotten} cached vendor '
              f'artifacts dropped, all {same} files stamped for the build to come')


def prune(target):
    meta = json.loads(run('cargo', 'metadata', '--no-deps', '--format-version', '1'))
    removed = forget(target, {p['name'] for p in meta['packages']})
    print(f'ci-cache: pruned {removed} workspace artifacts from {target}')


def main():
    if len(sys.argv) == 3 and sys.argv[1] == 'key':
        return key(sys.argv[2])
    if len(sys.argv) < 2 or sys.argv[1] not in ('restore', 'prune'):
        sys.exit(__doc__)
    target = sys.argv[2] if len(sys.argv) > 2 else 'target'
    (restore if sys.argv[1] == 'restore' else prune)(target)


if __name__ == '__main__':
    main()
