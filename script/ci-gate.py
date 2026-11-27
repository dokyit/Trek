#!/usr/bin/env python3
"""Did CI pass on the code a release is built from? The release workflow's gate (`ci-passed`).

    python3 script/ci-gate.py record OUT [--scope full|partial]
        In a CI run (.github/workflows/ci.yml): write the tested-tree record, the run's own account of
        the code it tested, to OUT (the run keeps it as its `tested-tree` artifact).
    python3 script/ci-gate.py judge COMMIT
        For the release (.github/workflows/release.yml): print "<others> <linux>", each yes or no:
        whether some completed CI run tested exactly COMMIT's code and passed every job without
        "Linux" in its name (others) and every job with it (linux). Passed means success or skipped,
        and a group with no jobs hasn't passed. What it found goes to stderr.

Which runs count:
- A run of COMMIT itself (a push to main, a dispatch): as before. Its record, when it has one, must
  say it tested COMMIT's tree with the full suite.
- A pull request's run, which tests GitHub's merge of the pull request into its base
  (refs/pull/N/merge), not the head commit the API names: only when its record says the tree it
  tested is COMMIT's tree (`git rev-parse COMMIT^{tree}`), with the full suite, in that run, of
  that head. The pull requests looked at are those GitHub links to COMMIT (merged, squashed or
  rebased into it) and a merge commit's second parent. So a release made right after a merge
  reuses the pull request's CI when nothing else reached main in between; when main moved after
  the pull request's run, the merge's tree differs from the tree the run tested and the run
  doesn't count. A run without a record (older than this check, or one that changed it) never
  counts for a pull request.

The record is written by the run's own workflow, read from the tree it tested: a run whose tree is
the release's ran the release's own ci.yml and this script, so its record can't claim another
tree without the release's own workflow being the one that lies.

Environment: GITHUB_REPOSITORY (default dokyit/Trek); `gh` with a token that can read actions,
contents and pull requests. TREK_GATE_LOG=0 silences stderr.
"""
import io
import json
import os
import subprocess
import sys
import zipfile

RECORD = 'tested-tree'
RECORD_FILE = 'tested-tree.json'
WORKFLOW = 'ci.yml'


def log(*parts):
    if os.environ.get('TREK_GATE_LOG', '1') != '0':
        print(*parts, file=sys.stderr)


def git(*args, cwd=None):
    return subprocess.run(['git', *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


class Gh:
    """GitHub's REST API through `gh api`."""

    def __init__(self, repo):
        self.repo = repo

    def _get(self, path):
        out = subprocess.run(['gh', 'api', path], check=True, capture_output=True).stdout
        return out

    def json(self, path):
        return json.loads(self._get(path))

    def runs(self, head_sha, event=None):
        q = f'repos/{self.repo}/actions/workflows/{WORKFLOW}/runs?head_sha={head_sha}&status=completed&per_page=20'
        if event:
            q += f'&event={event}'
        return self.json(q)['workflow_runs']

    def jobs(self, run_id):
        return self.json(f'repos/{self.repo}/actions/runs/{run_id}/jobs?per_page=100')['jobs']

    def pulls(self, commit):
        try:
            return self.json(f'repos/{self.repo}/commits/{commit}/pulls')
        except subprocess.CalledProcessError as e:
            log(f'(no pull requests for {commit[:8]}: {e.stderr.decode(errors="replace").strip()})')
            return []

    def record(self, run_id):
        found = self.json(f'repos/{self.repo}/actions/runs/{run_id}/artifacts?name={RECORD}')
        live = [a for a in found.get('artifacts', []) if a.get('name') == RECORD and not a.get('expired')]
        if not live:
            return None
        blob = self._get(f'repos/{self.repo}/actions/artifacts/{live[0]["id"]}/zip')
        with zipfile.ZipFile(io.BytesIO(blob)) as z:
            return json.loads(z.read(RECORD_FILE))


def groups(jobs):
    """(others, linux): each True when that group has jobs and all of them succeeded or were skipped."""
    def passed(linux):
        group = [j for j in jobs if ('Linux' in j['name']) == linux]
        return bool(group) and all(j.get('conclusion') in ('success', 'skipped') for j in group)
    return passed(False), passed(True)


def record_says(rec, run, tree):
    """Why the record doesn't vouch for this run having tested `tree` in full, or None when it does."""
    if not isinstance(rec, dict):
        return 'its record is unreadable'
    if rec.get('run_id') != run['id']:
        return f"its record is run {rec.get('run_id')}'s, not this run's"
    if rec.get('event') != run.get('event'):
        return f"its record is of a {rec.get('event')} event, the run of a {run.get('event')}"
    if rec.get('head') != run.get('head_sha'):
        return f"its record is of head {str(rec.get('head'))[:8]}, the run of {run.get('head_sha', '')[:8]}"
    if rec.get('scope') != 'full':
        return f"it tested only what the change touched ({rec.get('scope')}), not the full suite"
    if rec.get('tree') != tree:
        return f"it tested tree {str(rec.get('tree'))[:12]}, not this commit's {tree[:12]}"
    return None


def candidates(commit, api, cwd=None):
    """(run, needs a record) for each CI run that may have tested `commit`'s code, each once."""
    seen = set()
    out = []

    def add(runs, why):
        for run in runs:
            if run['id'] in seen:
                continue
            seen.add(run['id'])
            out.append((run, run.get('event') == 'pull_request', why))

    add(api.runs(commit), 'this commit')
    heads = []
    for pr in api.pulls(commit) or []:
        head = (pr.get('head') or {}).get('sha')
        if head and head not in heads:
            heads.append(head)
    parents = git('rev-list', '--parents', '-n', '1', commit, cwd=cwd).split()[1:]
    if len(parents) == 2 and parents[1] not in heads:
        heads.append(parents[1])
    for head in heads:
        if head != commit:
            add(api.runs(head, event='pull_request'), f'pull request head {head[:8]}')
    return out


def judge(commit, api, cwd=None):
    commit = git('rev-parse', '--verify', f'{commit}^{{commit}}', cwd=cwd)
    tree = git('rev-parse', f'{commit}^{{tree}}', cwd=cwd)
    others = linux = False
    for run, needs_record, why in candidates(commit, api, cwd):
        name = f"run {run['id']} ({run.get('event')}, {why}, {run.get('conclusion')})"
        rec = api.record(run['id'])
        if rec is None and needs_record:
            log(f'{name}: no tested-tree record, so what it tested is unknown; not counted')
            continue
        if rec is not None:
            wrong = record_says(rec, run, tree)
            if wrong:
                log(f'{name}: {wrong}; not counted')
                continue
        o, l = groups(api.jobs(run['id']))
        log(f"{name}: tested this tree; macOS and Windows {'yes' if o else 'no'}, Linux {'yes' if l else 'no'}")
        others, linux = others or o, linux or l
    return others, linux


def record(out, scope):
    sha = os.environ['GITHUB_SHA']
    head_now = git('rev-parse', 'HEAD')
    if head_now != sha:
        sys.exit(f'ci-gate: the checkout is {head_now}, not the run\'s {sha}')
    event = os.environ.get('GITHUB_EVENT_NAME', '')
    head = sha
    path = os.environ.get('GITHUB_EVENT_PATH')
    if event == 'pull_request' and path:
        head = json.load(open(path, encoding='utf-8'))['pull_request']['head']['sha']
    rec = {
        'version': 1,
        'run_id': int(os.environ['GITHUB_RUN_ID']),
        'event': event,
        'sha': sha,
        'head': head,
        'tree': git('rev-parse', 'HEAD^{tree}'),
        'scope': scope,
    }
    with open(out, 'w', encoding='utf-8') as f:
        json.dump(rec, f, indent=1)
        f.write('\n')
    print(json.dumps(rec))
    summary = os.environ.get('GITHUB_STEP_SUMMARY')
    if summary:
        with open(summary, 'a', encoding='utf-8') as f:
            f.write(f"Tested tree `{rec['tree']}` (commit `{sha}`, head `{head}`, scope {scope}).\n")


def main(argv):
    if len(argv) >= 2 and argv[0] == 'record':
        scope = 'full'
        if '--scope' in argv:
            scope = argv[argv.index('--scope') + 1]
        record(argv[1], scope)
        return 0
    if len(argv) == 2 and argv[0] == 'judge':
        others, linux = judge(argv[1], Gh(os.environ.get('GITHUB_REPOSITORY', 'dokyit/Trek')))
        print(f"{'yes' if others else 'no'} {'yes' if linux else 'no'}")
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
