#!/usr/bin/env python3
"""What a CI run has to build and test (.github/workflows/ci.yml's first job, "Plan").

    python3 script/ci-plan.py [--files FILE] [--metadata FILE] [--event NAME] [--labels a,b]

A push, a dispatch or a merge queue runs the full suite. A pull request runs what its change can
reach: the workspace crates its files belong to plus every crate that depends on them (cargo
metadata's dependency lists, followed in reverse), on the platforms that can compile those files.
Everything else is skipped, and reported as passed under its own name.

The full suite runs anyway when the change touches something that can affect any crate (FULL
below: the workspace manifest and lock, the toolchain, vendor/, .github/, a build script, Cargo
config), a file outside the crates that no crate names and that isn't known to be read by none
(UNREAD), or when the pull request carries the label `ci:full`.

A platform is left out only when every changed file of the crates is provably compiled without
it: a file is a module whose `mod` line (or an enclosing module's) is gated by a `#[cfg(...)]`
naming target_os / windows / unix that is false there. Anything the check can't follow (a
#[path], cfg_if!, a module it can't find, a cfg it can't read) counts for every platform.

Without --files, the changed files are `git diff --name-only HEAD^1 HEAD`: for a pull request's
merge commit (refs/pull/N/merge) that is exactly what the pull request brings to its base.
Without --metadata it runs `cargo metadata --no-deps`. Results go to $GITHUB_OUTPUT (and stdout):

  full      true / false            scope    full / partial (the release gate reads it)
  packages  affected crates, space separated       app      trek-app affected
  selected  `-p` options for the affected crates      others   the same without trek-app ('' when none)
  macos, windows, linux      that platform runs     scripts  the script tests run
  audit     Cargo.lock changed (or full)            arm      trek-app or trek-mcp affected
  rust      anything to build at all
"""
import argparse
import json
import os
import re
import subprocess
import sys

APP = 'trek-app'
PLATFORMS = ('macos', 'windows', 'linux')
LABEL_FULL = 'ci:full'

# Changes that can affect every crate: the full suite.
FULL_EXACT = {'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'rust-toolchain', 'clippy.toml',
              'rustfmt.toml', '.rustfmt.toml', 'deny.toml'}
FULL_PREFIX = ('vendor/', '.github/', '.cargo/', 'crates/test-fixtures/',
               # The scripts CI itself runs to build and split the tests.
               'script/ci-', 'script/linux/', 'script/bundle-linux.sh')
# Files outside the crates that no Rust build or test reads, unless a crate names them (checked).
UNREAD_PREFIX = ('docs/', 'site/', 'ios/', 'media/', 'evidence/', '.agents/', 'assets/brand/', 'bin/')
UNREAD_SUFFIX = ('.md',)
UNREAD_EXACT = {'LICENSE', 'NOTICE', '.gitignore', '.gitattributes'}
# Crates whose tests walk folders by building the path piece by piece, which no string in their
# sources names whole: a change under one of these prefixes reaches that crate. (`tests` in
# script/tests/ci-plan.py fails when a test leaves its crate with `CARGO_MANIFEST_DIR` and `..`
# from a file not listed in WALKER_FILES, so a new one can't go unnoticed.)
WALKERS = {
    # The scale check reads every UI source of these crates.
    'trek-palette': ('crates/trek-app/src/', 'crates/trek-line-breakdown/src/', 'crates/trek-motion/src/'),
    # The gate count reads every crate's source; the ledger and its pages are its own.
    'trek-parity': ('crates/', 'docs/parity/', 'docs/PARITY.md'),
}
# The files that leave their crate to read others with `CARGO_MANIFEST_DIR` and `..`, and what
# catches them: 'walker' (in WALKERS above) or 'named' (the whole path is in a string, which the
# reader check finds).
WALKER_FILES = {
    'crates/trek-palette/src/scale.rs': 'walker',
    'crates/trek-parity/src/bin/parity-live.rs': 'walker',
    'crates/trek-parity/tests/parity.rs': 'walker',
    'crates/trek-words/src/lib.rs': 'named',
    'crates/trek-agents/src/opencode.rs': 'named',
    'crates/trek-app/src/winsys.rs': 'named',
    'crates/trek-brand/src/small_icon_tests.rs': 'named',
    'crates/trek-core/src/artifact.rs': 'named',
}
# Programs a crate starts without depending on them: a change to the key is a change to what the
# crates listed run (trek-app finds the trek-mcp dev build beside it, workspace.rs trek_mcp_binary).
LAUNCHES = {
    'trek-mcp': ('trek-app',),
}

# A path named inside one of these is compiled into the crate (include_str!, include_bytes!,
# rust-embed's folder), so what depends on that crate is reached too; a path read at run time (a
# test's fs::read, a walk) reaches only the crate that reads it.
EMBED = re.compile(r'(include_str!|include_bytes!|#\[folder\s*=)\s*\((?:[^;]*)$|#\[folder\s*=\s*[^\]]*$', re.S)

# File names too common to mean a crate reads this one when it names a file of that name.
GENERIC_NAMES = {'README.md', 'SKILL.md', 'CHANGELOG.md', 'LICENSE.md', 'mod.rs', 'lib.rs', 'main.rs',
                 'Cargo.toml', 'package.json', 'Contents.json', 'config.toml', 'settings.json',
                 'manifest.json', 'styles.css', 'style.css', 'script.js', 'main.js'}
# Files the script tests cover (each platform's "Scripts" job).
SCRIPT_PREFIX = ('script/', 'ios/scripts/', 'media/', 'site/', 'assets/update/', '.agents/skills/verify-trek/scripts/')


def git(*args):
    return subprocess.run(['git', *args], check=True, capture_output=True, text=True).stdout


class Workspace:
    def __init__(self, meta, root):
        self.root = root
        members = set(meta['workspace_members'])
        self.packages = {}
        for p in meta['packages']:
            if p['id'] in members:
                self.packages[p['name']] = p
        self.dirs = {}
        for name, p in self.packages.items():
            d = os.path.relpath(os.path.dirname(p['manifest_path']), root).replace(os.sep, '/')
            self.dirs[d] = name
        self.rdeps = {n: set() for n in self.packages}
        for name, p in self.packages.items():
            for d in p['dependencies']:
                if d['name'] in self.packages and d['name'] != name:
                    self.rdeps[d['name']].add(name)
        for launched, by in LAUNCHES.items():
            if launched in self.rdeps:
                self.rdeps[launched] |= {b for b in by if b in self.packages}

    def package_of(self, path):
        """(package, path within it) for a file in a crate, or (None, None)."""
        best = None
        for d, name in self.dirs.items():
            if path.startswith(d + '/') and (best is None or len(d) > len(best)):
                best = d
        if best is None:
            return None, None
        return self.dirs[best], path[len(best) + 1:]

    def closure(self, names):
        out, todo = set(names), list(names)
        while todo:
            for r in self.rdeps.get(todo.pop(), ()):
                if r not in out:
                    out.add(r)
                    todo.append(r)
        return out

    def sources(self):
        """(package, text) of each crate's Rust files and manifest, read once."""
        if not hasattr(self, '_sources'):
            self._sources = []
            for d, name in self.dirs.items():
                for base, dirs, files in os.walk(os.path.join(self.root, d)):
                    dirs[:] = [x for x in dirs if x not in ('target', '.git')]
                    for f in files:
                        if f.endswith('.rs') or f in ('Cargo.toml', 'build.rs'):
                            try:
                                with open(os.path.join(base, f), encoding='utf-8', errors='replace') as fh:
                                    self._sources.append((name, fh.read()))
                            except OSError:
                                pass
        return self._sources

    def readers(self, path):
        """The crates whose sources name `path`, a folder it's in (two levels deep or more), or its
        file name, in a string that ends there ("../../assets/fonts", "docs/parity/ledger.toml",
        "LivePage.swift", include_str!("../../trek-assets/assets/themes/trek.json"),
        join("../trek-app/src")): a crate that names a deeper path in the same folder doesn't read
        this. A path in crates/ is looked for with and without `crates/`, as a crate reaches
        another's files by `../<crate>/…`. Then the crates in WALKERS whose folders hold it."""
        forms = [path]
        if path.startswith('crates/'):
            forms.append(path[len('crates/'):])
        needles = []
        for form in forms:
            parts = form.split('/')
            needles += [form] + ['/'.join(parts[:i]) for i in range(len(parts) - 1, 1, -1)]
        parts = path.split('/')
        base = parts[-1]
        # By its name alone only outside the crates (a crate reaches another's file by its path).
        if (not path.startswith('crates/') and '.' in base and len(base) >= 6 and base not in GENERIC_NAMES
                and not base.startswith('index.')):
            needles.append(base)
        pattern = re.compile('(' + '|'.join(re.escape(n) for n in needles) + ')/?"')
        embedded, read = set(), set()
        for name, text in self.sources():
            for m in pattern.finditer(text):
                (embedded if EMBED.search(text[max(0, m.start() - 160):m.start()]) else read).add(name)
        read |= {crate for crate, prefixes in WALKERS.items()
                 if crate in self.packages and any(path.startswith(p) for p in prefixes)}
        return embedded, read - embedded


# --- cfg gates -------------------------------------------------------------------------------

def _split_args(s):
    out, depth, cur = [], 0, ''
    for ch in s:
        if ch == '(':
            depth += 1
        elif ch == ')':
            depth -= 1
        if ch == ',' and depth == 0:
            out.append(cur.strip())
            cur = ''
        else:
            cur += ch
    if cur.strip():
        out.append(cur.strip())
    return out


def cfg_platforms(pred):
    """The platforms where a cfg predicate can be true; None when it can't be read."""
    pred = pred.strip()
    m = re.fullmatch(r'(all|any|not)\s*\((.*)\)', pred, re.S)
    if m:
        args = [cfg_platforms(a) for a in _split_args(m.group(2))]
        if any(a is None for a in args):
            return None
        if m.group(1) == 'not':
            if len(args) != 1:
                return None
            # not(x) can be true wherever x can be false: only an atom that is exactly an OS set
            # (true on those, false elsewhere) narrows it.
            exact = _exact_os(_split_args(m.group(2))[0])
            return set(PLATFORMS) - exact if exact is not None else set(PLATFORMS)
        if m.group(1) == 'all':
            out = set(PLATFORMS)
            for a in args:
                out &= a
            return out
        out = set()
        for a in args:
            out |= a
        return out
    exact = _exact_os(pred)
    if exact is not None:
        return exact
    if re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*(\s*=\s*"[^"]*")?', pred):
        return set(PLATFORMS)  # feature = "x", test, debug_assertions, target_arch = ...: any platform
    return None


def _exact_os(pred):
    """The platforms an OS atom is true on (and false elsewhere), or None for other atoms."""
    pred = pred.strip()
    if pred == 'windows':
        return {'windows'}
    if pred == 'unix':
        return {'macos', 'linux'}
    m = re.fullmatch(r'target_os\s*=\s*"([^"]+)"', pred)
    if m:
        return {m.group(1)} & set(PLATFORMS)
    m = re.fullmatch(r'target_family\s*=\s*"([^"]+)"', pred)
    if m:
        return {'windows'} if m.group(1) == 'windows' else {'macos', 'linux'} if m.group(1) == 'unix' else set()
    m = re.fullmatch(r'all\s*\((.*)\)|any\s*\((.*)\)', pred, re.S)
    if m:
        args = [_exact_os(a) for a in _split_args(m.group(1) or m.group(2))]
        if any(a is None for a in args):
            return None
        out = set(PLATFORMS) if m.group(1) is not None else set()
        for a in args:
            out = out & a if m.group(1) is not None else out | a
        return out
    m = re.fullmatch(r'not\s*\((.*)\)', pred, re.S)
    if m:
        inner = _exact_os(m.group(1))
        return None if inner is None else set(PLATFORMS) - inner
    return None


ROOTS = ('src/lib.rs', 'src/main.rs')


def module_platforms(root, crate_dir, rel, depth=0):
    """The platforms that compile crate file `rel` (a path within the crate), as far as provable."""
    every = set(PLATFORMS)
    if depth > 20 or not rel.endswith('.rs') or not rel.startswith('src/'):
        return every  # tests/, examples/, benches/, build.rs: compiled everywhere
    if rel in ROOTS or rel.startswith('src/bin/'):
        return every
    parts = rel[len('src/'):].split('/')
    if parts[-1] == 'mod.rs':
        parts = parts[:-1]
        name = parts[-1]
        parent_parts = parts[:-1]
    else:
        name = parts[-1][:-3]
        parent_parts = parts[:-1]
    if parent_parts:
        candidates = ['src/' + '/'.join(parent_parts) + '.rs', 'src/' + '/'.join(parent_parts) + '/mod.rs']
    else:
        candidates = list(ROOTS)
    found = []
    for cand in candidates:
        path = os.path.join(root, crate_dir, cand)
        if not os.path.exists(path):
            continue
        with open(path, encoding='utf-8', errors='replace') as f:
            text = f.read()
        gate = declared_gate(text, name)
        if gate == 'missing':
            continue
        if gate is None:
            return every
        found.append((cand, gate))
    if not found:
        return every
    out = set()
    for cand, gate in found:
        out |= gate & module_platforms(root, crate_dir, cand, depth + 1)
    return out


def declared_gate(text, name):
    """The platforms of the `mod name;` line's cfg attributes; 'missing' with no such line; None
    when it can't be read (a #[path], cfg_attr, an unreadable cfg)."""
    lines = text.splitlines()
    decl = re.compile(r'^\s*(pub(\s*\([^)]*\))?\s+)?mod\s+' + re.escape(name) + r'\s*;')
    hits = [i for i, l in enumerate(lines) if decl.match(l)]
    if not hits:
        if re.search(r'\bmod\s+' + re.escape(name) + r'\b', text):
            return None  # declared some other way (inline, a macro): can't tell
        return 'missing'
    out = set()
    for i in hits:
        attrs = []
        j = i - 1
        while j >= 0:
            s = lines[j].strip()
            if s.startswith('#['):
                attrs.append(s)
            elif s.startswith('//'):
                pass
            elif s == '' or s.endswith((';', '{', '}')):
                break  # the item above: nothing more attaches to this line
            else:
                return None  # the inside of an attribute spread over lines: can't read it
            j -= 1
        gate = set(PLATFORMS)
        for a in attrs:
            if a.startswith('#[path') or a.startswith('#[cfg_attr'):
                return None
            m = re.fullmatch(r'#\[cfg\((.*)\)\]', a, re.S)
            if m:
                g = cfg_platforms(m.group(1))
                if g is None:
                    return None
                gate &= g
            elif a.startswith('#[cfg'):
                return None
        out |= gate
    return out


# --- the plan --------------------------------------------------------------------------------

def plan(files, ws, event, labels=()):
    result = {'full': False, 'reasons': [], 'packages': set(), 'platforms': set(), 'scripts': False,
              'audit': False}

    def full(why):
        result['full'] = True
        result['reasons'].append(why)

    if event not in ('pull_request',):
        full(f'a {event or "push"} runs everything')
    if LABEL_FULL in labels:
        full(f'the pull request is labelled {LABEL_FULL}')
    # Crates a file belongs to (what depends on them is reached too), and crates whose tests only
    # read a changed file (they don't change, so what depends on them isn't).
    touched, read_by = set(), set()
    for path in files:
        if path in FULL_EXACT or path.startswith(FULL_PREFIX) or path.endswith('/build.rs') or path == 'build.rs':
            full(f'{path} can affect every crate')
            if path == 'Cargo.lock':
                result['audit'] = True
            continue
        if path.startswith(SCRIPT_PREFIX):
            result['scripts'] = True
        pkg, rel = ws.package_of(path)
        if pkg is not None:
            touched.add(pkg)
            crate_dir = os.path.dirname(os.path.relpath(ws.packages[pkg]['manifest_path'], ws.root))
            gates = module_platforms(ws.root, crate_dir, rel) if rel.endswith('.rs') else set(PLATFORMS)
            result['platforms'] |= gates
            # Another crate's test may read this file as a file (trek-words reads trek-app's
            # sources, trek-palette the theme in trek-assets, trek-parity every crate's gates): it
            # reaches that crate, on the platforms that compile the file (every one when none do).
            embedded, read = ws.readers(path)
            embedded, read = embedded - {pkg}, read - {pkg}
            if embedded or read:
                touched |= embedded
                read_by |= read
                if not gates:
                    result['platforms'] |= set(PLATFORMS)
            continue
        embedded, read = ws.readers(path)
        if embedded or read:
            touched |= embedded
            read_by |= read
            result['platforms'] |= set(PLATFORMS)
            continue
        if path in UNREAD_EXACT or path.startswith(UNREAD_PREFIX) or path.endswith(UNREAD_SUFFIX):
            continue
        if path.startswith(SCRIPT_PREFIX):
            continue  # a script no crate reads: the script tests cover it
        full(f'{path} is outside the crates and its readers are unknown')
    if result['full']:
        result['packages'] = set(ws.packages)
        result['platforms'] = set(PLATFORMS)
        result['scripts'] = True
        result['audit'] = True
    else:
        result['packages'] = ws.closure(touched) | read_by
        if not result['packages']:
            result['platforms'] = set()
    return result


def outputs(result):
    pk = sorted(result['packages'])
    others = [p for p in pk if p != APP]
    plat = result['platforms']
    out = {
        'full': str(result['full']).lower(),
        'scope': 'full' if result['full'] else 'partial',
        'packages': ' '.join(pk),
        'app': str(APP in pk).lower(),
        'selected': ' '.join(f'-p {p}' for p in pk),
        'others': ' '.join(f'-p {p}' for p in others),
        'macos': str('macos' in plat).lower(),
        'windows': str('windows' in plat).lower(),
        'linux': str('linux' in plat).lower(),
        'scripts': str(result['scripts']).lower(),
        'audit': str(result['audit']).lower(),
        'arm': str(bool({APP, 'trek-mcp'} & set(pk)) and 'linux' in plat).lower(),
        'rust': str(bool(pk) and bool(plat)).lower(),
    }
    return out


def changed_files(cwd=None):
    """What a pull request's merge commit brings to its base: HEAD^1..HEAD. --no-renames: a moved
    file is its old path and its new one, so moving a file out of a crate (or out of what a crate
    reads) still reaches that crate."""
    out = subprocess.run(['git', 'diff', '--name-only', '--no-renames', 'HEAD^1', 'HEAD'],
                         check=True, capture_output=True, text=True, cwd=cwd).stdout
    return [l for l in out.splitlines() if l]


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument('--files')
    ap.add_argument('--metadata')
    ap.add_argument('--event', default=os.environ.get('GITHUB_EVENT_NAME', 'push'))
    ap.add_argument('--labels', default=None)
    ap.add_argument('--root', default='.')
    a = ap.parse_args(argv)
    root = os.path.abspath(a.root)
    if a.files:
        with open(a.files, encoding='utf-8') as f:
            files = [l.strip() for l in f if l.strip()]
    elif a.event == 'pull_request':
        files = changed_files()
    else:
        files = []
    if a.metadata:
        with open(a.metadata, encoding='utf-8') as f:
            meta = json.load(f)
    else:
        meta = json.loads(subprocess.run(['cargo', 'metadata', '--no-deps', '--format-version', '1', '--locked'],
                                         check=True, capture_output=True, cwd=root).stdout)
    labels = []
    if a.labels is not None:
        labels = [l for l in a.labels.split(',') if l]
    elif os.environ.get('GITHUB_EVENT_PATH') and a.event == 'pull_request':
        with open(os.environ['GITHUB_EVENT_PATH'], encoding='utf-8') as f:
            labels = [l['name'] for l in json.load(f).get('pull_request', {}).get('labels', [])]
    ws = Workspace(meta, root)
    result = plan(files, ws, a.event, labels)
    out = outputs(result)
    report = [f"{len(files)} changed files; scope {out['scope']}"]
    report += [f'  full: {r}' for r in result['reasons'][:10]]
    report.append(f"  crates: {out['packages'] or 'none'}")
    report.append(f"  platforms: {', '.join(p for p in PLATFORMS if out[p] == 'true') or 'none'}")
    report.append(f"  scripts: {out['scripts']}, audit: {out['audit']}")
    print('\n'.join(report), file=sys.stderr)
    lines = [f'{k}={v}' for k, v in out.items()]
    print('\n'.join(lines))
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as f:
            f.write('\n'.join(lines) + '\n')
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a', encoding='utf-8') as f:
            f.write('### What this run tests\n\n```\n' + '\n'.join(report) + '\n```\n')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
