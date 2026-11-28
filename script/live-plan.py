#!/usr/bin/env python3
"""Does a change touch app code, so the live checks (.github/workflows/live.yml) must run on it?

    python3 script/live-plan.py [FILE...]      (no files: git diff --name-only --no-renames HEAD^1 HEAD)

Prints `true` or `false`. App code is anything but what no build of Trek reads: the docs (but
docs/parity/, the ledger the live checks run), the site, the iPhone app, media, the brand files,
the workflows other than live.yml, CI's own scripts and the scripts' tests, Markdown and the
licence files. And even those count when a
crate names the file, or a folder of it two levels deep or more, in a string that ends there:
crates/trek-core/skills/*/SKILL.md (inside a crate), ios/scripts/phone-update.sh and
ios/TrekDriver/… (read by crates), assets/brand (read by tests). A file in crates/ always counts.
Used by live.yml's first job and by script/merge-pr; script/tests/live-plan.py tests it.
"""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

ALWAYS = ('crates/', 'vendor/', 'docs/parity/', '.github/workflows/live.yml', '.agents/skills/verify-trek/')
# Never built into Trek, whatever a crate's example strings say (file_kind's tests name ci.yml).
NEVER = ('.github/', 'script/ci-', 'script/tests/')
NOT_APP = re.compile(r'^(docs/|site/|ios/|media/|evidence/|assets/brand/|\.github/|script/(ci-|tests/|site/))|\.md$|'
                     r'^(LICENSE|NOTICE|\.gitignore|\.gitattributes)$')


def crate_sources(root):
    out = []
    for base, dirs, files in os.walk(os.path.join(root, 'crates')):
        dirs[:] = [d for d in dirs if d != 'target']
        for f in files:
            if f.endswith('.rs') or f == 'Cargo.toml':
                try:
                    with open(os.path.join(base, f), encoding='utf-8', errors='replace') as fh:
                        out.append(fh.read())
                except OSError:
                    pass
    return out


def named_by_a_crate(path, sources):
    parts = path.split('/')
    needles = [path] + ['/'.join(parts[:i]) for i in range(len(parts) - 1, 1, -1)]
    pattern = re.compile('(' + '|'.join(re.escape(n) for n in needles) + ')/?"')
    return any(pattern.search(t) for t in sources)


def app_code(files, root=ROOT):
    sources = None
    for f in files:
        if f.startswith(ALWAYS) or not NOT_APP.search(f):
            return True
        if f.startswith(NEVER):
            continue
        if sources is None:
            sources = crate_sources(root)
        if named_by_a_crate(f, sources):
            return True
    return False


def changed_files(cwd=None):
    out = subprocess.run(['git', 'diff', '--name-only', '--no-renames', 'HEAD^1', 'HEAD'],
                         check=True, capture_output=True, text=True, cwd=cwd).stdout
    return [l for l in out.splitlines() if l]


if __name__ == '__main__':
    files = sys.argv[1:] or changed_files()
    print('true' if app_code(files) else 'false')
