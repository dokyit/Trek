#!/usr/bin/env python3
"""Split trek-app's test targets across CI jobs.

    python3 script/ci-shard.py SHARD COUNT    (SHARD counts from 1)

Prints the `cargo test` target options for that shard: `--test <name>` for each integration test
it runs, and `--lib --bins` in shard 1. Together the shards run every target `cargo test -p
trek-app` runs: the test targets come from `cargo metadata`, so a new tests/*.rs file lands in a
shard without anyone listing it (and `--check` fails CI if one ever didn't).

Targets go to the shard with the least work so far, heaviest first. A target's weight is its run
time on the Windows runner (the slower one; measured 2026-10-27), in seconds, plus COMPILE for
building and linking it; one missing from the table counts as DEFAULT.
"""
import json
import subprocess
import sys

PACKAGE = 'trek-app'
# The unit tests in src/ (--lib --bins): the lib compiled once more as a test crate (~70 s).
UNIT = '<lib+bins>'
WEIGHTS = {
    UNIT: 60, 'long_answers': 81, 'lifecycle': 76, 'limits': 56, 'background': 54,
    'worktrees': 54, 'rewind': 51, 'flows': 50, 'ide': 47, 'orchestrate': 45, 'screens': 36,
    'remote': 35, 'changes': 32, 'pstack': 27, 'palette': 25, 'composer': 25, 'lineage': 24,
    'agent_updates': 23, 'tabs': 21, 'render': 18, 'editor_polish': 17, 'alerts': 16,
    'motion': 15, 'mcp': 14, 'activity': 14, 'machines': 13, 'terminal': 13, 'visualization': 12,
    'add_agent': 12, 'artifacts': 11, 'terminal_dock': 10, 'preview': 10, 'inbox': 10,
    'readability': 9, 'usage': 8,
}
DEFAULT = 4
COMPILE = 4


def targets():
    meta = json.loads(subprocess.run(['cargo', 'metadata', '--no-deps', '--format-version', '1'],
                                     check=True, capture_output=True).stdout)
    pkg = next(p for p in meta['packages'] if p['name'] == PACKAGE)
    kinds = {k for t in pkg['targets'] for k in t['kind']}
    # Anything `cargo test` would build beyond lib, bins and tests needs a place here first.
    unknown = kinds - {'lib', 'bin', 'test', 'custom-build'}
    if unknown:
        sys.exit(f'ci-shard: {PACKAGE} has {sorted(unknown)} targets; teach script/ci-shard.py about them')
    return sorted(t['name'] for t in pkg['targets'] if 'test' in t['kind'])


def plan(count):
    shards = [[] for _ in range(count)]
    load = [0] * count
    shards[0].append(UNIT)
    load[0] += WEIGHTS[UNIT]
    for name in sorted(targets(), key=lambda n: (-WEIGHTS.get(n, DEFAULT), n)):
        i = min(range(count), key=lambda k: (load[k], k))
        shards[i].append(name)
        load[i] += WEIGHTS.get(name, DEFAULT) + COMPILE
    return shards, load


def main():
    if len(sys.argv) == 3 and sys.argv[1] == '--check':
        count = int(sys.argv[2])
        shards, load = plan(count)
        planned = sorted(n for s in shards for n in s if n != UNIT)
        assert planned == targets() and sum(UNIT in s for s in shards) == 1, 'shards miss a target'
        for i, (s, w) in enumerate(zip(shards, load), 1):
            print(f'shard {i}/{count}: ~{w}s {" ".join(s)}')
        return
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    shard, count = int(sys.argv[1]), int(sys.argv[2])
    if not 1 <= shard <= count:
        sys.exit(f'ci-shard: shard {shard} of {count}?')
    shards, _ = plan(count)
    args = []
    for name in shards[shard - 1]:
        args += ['--lib', '--bins'] if name == UNIT else ['--test', name]
    print(' '.join(args))


if __name__ == '__main__':
    main()
