#!/usr/bin/env python3
"""Tests for script/ci-gate.py, the release gate: which CI runs vouch for the code a release is built
from. Each case builds a throwaway repository (a main, a pull request, GitHub's test merge of it, the
merge that landed) and a stand-in for GitHub's API holding the runs, their jobs and their records.

    python3 script/tests/ci-gate.py
"""
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
spec = importlib.util.spec_from_file_location('ci_gate', os.path.join(ROOT, 'script', 'ci-gate.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
os.environ['TREK_GATE_LOG'] = '0'

PASSED = [
    {'name': 'Plan', 'conclusion': 'success'},
    {'name': 'Test (macOS)', 'conclusion': 'success'},
    {'name': 'Test (Windows, trek-app 1/9)', 'conclusion': 'success'},
    {'name': 'Clippy', 'conclusion': 'success'},
    {'name': 'Test (Linux)', 'conclusion': 'success'},
    {'name': 'CI result', 'conclusion': 'success'},
]


def failed(jobs, name):
    return [dict(j, conclusion='failure') if j['name'] == name else j for j in jobs]


class FakeGh:
    """The API calls ci-gate.py makes, answered from dicts."""

    def __init__(self):
        self.all_runs = []      # dicts: id, head_sha, event, conclusion
        self.job_lists = {}     # run id -> jobs
        self.records = {}       # run id -> record dict
        self.pull_list = {}     # commit -> [pull request]

    def run(self, run_id, head, event, jobs=PASSED, rec=None, conclusion='success'):
        self.all_runs.append({'id': run_id, 'head_sha': head, 'event': event, 'conclusion': conclusion})
        self.job_lists[run_id] = jobs
        if rec is not None:
            self.records[run_id] = rec

    def runs(self, head_sha, event=None):
        return [r for r in self.all_runs if r['head_sha'] == head_sha and (event is None or r['event'] == event)]

    def jobs(self, run_id):
        return self.job_lists[run_id]

    def pulls(self, commit):
        return self.pull_list.get(commit, [])

    def record(self, run_id):
        return self.records.get(run_id)


class Repo:
    def __init__(self, path):
        self.path = path
        os.makedirs(path, exist_ok=True)
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.name', 'Gate Test')
        self.git('config', 'user.email', 'gate-test@localhost')
        self.git('config', 'commit.gpgsign', 'false')

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.path, check=True, capture_output=True, text=True).stdout.strip()

    def commit(self, name, text, msg):
        with open(os.path.join(self.path, name), 'w') as f:
            f.write(text)
        self.git('add', name)
        self.git('commit', '-q', '-m', msg)
        return self.git('rev-parse', 'HEAD')

    def tree(self, rev):
        return self.git('rev-parse', f'{rev}^{{tree}}')

    def merge(self, base, head, msg):
        """A merge commit of head into base (as GitHub's test merge and its merge button make), left on no branch."""
        self.git('checkout', '-q', '--detach', base)
        self.git('merge', '-q', '--no-ff', '--no-edit', '-m', msg, head)
        sha = self.git('rev-parse', 'HEAD')
        self.git('checkout', '-q', 'main')
        return sha


def record(run_id, head, tree, event='pull_request', scope='full', required=('Plan', 'CI result')):
    return {'version': 1, 'run_id': run_id, 'event': event, 'sha': 'merge', 'head': head, 'tree': tree,
            'scope': scope, 'required': list(required)}


def skipped_after_plan(plan='failure'):
    """What a run looks like when its Plan job failed: everything after it skipped."""
    return [dict(j, conclusion=plan if j['name'] == 'Plan' else 'skipped') for j in PASSED]


class Gate(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        env = {'GIT_CONFIG_GLOBAL': os.path.join(self.tmp.name, 'gitconfig'), 'GIT_CONFIG_NOSYSTEM': '1'}
        self._env = {k: os.environ.get(k) for k in env}
        os.environ.update(env)
        self.repo = Repo(os.path.join(self.tmp.name, 'repo'))
        self.m1 = self.repo.commit('a.txt', 'one\n', 'main one')
        self.repo.git('checkout', '-q', '-b', 'pr')
        self.head = self.repo.commit('b.txt', 'pull request\n', 'the pull request')
        self.repo.git('checkout', '-q', 'main')
        # GitHub's refs/pull/N/merge when the pull request's CI ran: its head merged into main one.
        self.tested = self.repo.merge(self.m1, self.head, 'Merge into main one (test merge)')
        self.gh = FakeGh()

    def tearDown(self):
        for k, v in self._env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        self.tmp.cleanup()

    def judge(self, commit):
        return gate.judge(commit, self.gh, cwd=self.repo.path)

    def landed_on_unmoved_main(self):
        return self.repo.merge(self.m1, self.head, 'Merge pull request #1')

    # Runs of the commit itself (pushes to main), as the gate always took them.

    def test_a_push_run_of_the_commit_counts(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(1, landed, 'push')
        self.assertEqual(self.judge(landed), (True, True))

    def test_a_push_run_with_a_failed_job_does_not(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(1, landed, 'push', jobs=failed(PASSED, 'Clippy'))
        self.assertEqual(self.judge(landed), (False, True))

    def test_linux_is_judged_apart(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(1, landed, 'push', jobs=failed(PASSED, 'Test (Linux)'))
        self.assertEqual(self.judge(landed), (True, False))

    def test_skipped_jobs_pass_beside_one_that_succeeded(self):
        landed = self.landed_on_unmoved_main()
        jobs = [dict(j, conclusion='skipped') if j['name'] == 'Test (macOS)' else j for j in PASSED]
        self.gh.run(1, landed, 'push', jobs=jobs)
        self.assertEqual(self.judge(landed), (True, True))

    def test_a_group_of_nothing_but_skipped_jobs_ran_nothing(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(1, landed, 'push', jobs=[dict(j, conclusion='skipped') for j in PASSED])
        self.assertEqual(self.judge(landed), (False, False))
        # Linux's jobs all skipped while the others ran: Linux hasn't passed.
        self.gh = FakeGh()
        jobs = [dict(j, conclusion='skipped') if 'Linux' in j['name'] else j for j in PASSED]
        self.gh.run(2, landed, 'push', jobs=jobs)
        self.assertEqual(self.judge(landed), (True, False))
        self.gh = FakeGh()
        self.gh.run(3, landed, 'push', jobs=[])
        self.assertEqual(self.judge(landed), (False, False))

    def test_a_failed_plan_that_left_a_full_record_does_not_count(self):
        # Plan wrote and uploaded its record, then failed: every other job was skipped.
        landed = self.landed_on_unmoved_main()
        self.gh.run(7, self.head, 'pull_request', jobs=skipped_after_plan(),
                    rec=record(7, self.head, self.repo.tree(self.tested)))
        self.assertEqual(self.judge(landed), (False, False))
        # Nor does it lend Linux to a push run whose Linux job failed.
        self.gh.run(8, landed, 'push', jobs=failed(PASSED, 'Test (Linux)'))
        self.assertEqual(self.judge(landed), (True, False))

    def test_a_job_the_record_names_must_have_succeeded(self):
        landed = self.landed_on_unmoved_main()
        tree = self.repo.tree(self.tested)
        for conclusion in ('failure', 'cancelled', 'skipped'):
            self.gh = FakeGh()
            self.gh.run(7, self.head, 'pull_request', jobs=failed(PASSED, 'CI result') if conclusion == 'failure'
                        else [dict(j, conclusion=conclusion) if j['name'] == 'CI result' else j for j in PASSED],
                        rec=record(7, self.head, tree))
            self.assertEqual(self.judge(landed), (False, False), conclusion)
        # A job the record names that isn't in the run at all.
        self.gh = FakeGh()
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, tree, required=['Tested tree']))
        self.assertEqual(self.judge(landed), (False, False))
        # A record that names none (from before the gate read them).
        self.gh = FakeGh()
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, tree, required=()))
        self.assertEqual(self.judge(landed), (False, False))

    def test_plan_and_ci_result_must_succeed_even_without_a_record(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(1, landed, 'push', jobs=[dict(j, conclusion='skipped') if j['name'] == 'CI result' else j
                                             for j in PASSED])
        self.assertEqual(self.judge(landed), (False, False))

    def test_a_cancelled_run_does_not_count(self):
        landed = self.landed_on_unmoved_main()
        jobs = [dict(j, conclusion='cancelled') if j['name'] not in ('Plan',) else j for j in PASSED]
        self.gh.run(1, landed, 'push', jobs=jobs, conclusion='cancelled')
        self.assertEqual(self.judge(landed), (False, False))
        self.gh = FakeGh()
        self.gh.run(7, self.head, 'pull_request', jobs=jobs, conclusion='cancelled',
                    rec=record(7, self.head, self.repo.tree(self.tested)))
        self.assertEqual(self.judge(landed), (False, False))

    def test_a_push_run_whose_record_says_partial_does_not_count(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(1, landed, 'push', rec=record(1, landed, self.repo.tree(landed), event='push', scope='trek-app'))
        self.assertEqual(self.judge(landed), (False, False))

    def test_no_run_at_all(self):
        self.assertEqual(self.judge(self.landed_on_unmoved_main()), (False, False))

    # A pull request's run, which tested GitHub's merge ref.

    def test_the_pull_requests_run_counts_when_main_did_not_move(self):
        landed = self.landed_on_unmoved_main()
        self.assertEqual(self.repo.tree(landed), self.repo.tree(self.tested))
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, self.repo.tree(self.tested)))
        self.assertEqual(self.judge(landed), (True, True))

    def test_main_moved_after_the_pull_requests_run(self):
        # The pull request's CI tested head + main one; then another change reached main, and the
        # pull request merged on top of it: different code from what its CI ran.
        self.repo.commit('c.txt', 'someone else\n', 'main two')
        m2 = self.repo.git('rev-parse', 'HEAD')
        landed = self.repo.merge(m2, self.head, 'Merge pull request #1')
        self.assertNotEqual(self.repo.tree(landed), self.repo.tree(self.tested))
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, self.repo.tree(self.tested)))
        self.assertEqual(self.judge(landed), (False, False))

    def test_main_moved_and_a_later_pull_request_run_tested_the_new_merge(self):
        self.repo.commit('c.txt', 'someone else\n', 'main two')
        m2 = self.repo.git('rev-parse', 'HEAD')
        retested = self.repo.merge(m2, self.head, 'Merge into main two (test merge)')
        landed = self.repo.merge(m2, self.head, 'Merge pull request #1')
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, self.repo.tree(self.tested)))
        self.gh.run(8, self.head, 'pull_request', rec=record(8, self.head, self.repo.tree(retested)))
        self.assertEqual(self.judge(landed), (True, True))

    def test_a_squash_merge_is_found_through_its_pull_request(self):
        # One parent: the head isn't in the history, so only GitHub's link names the pull request.
        self.repo.git('checkout', '-q', 'main')
        self.repo.git('merge', '-q', '--squash', self.head)
        self.repo.git('commit', '-q', '-m', 'The pull request (#1)')
        squashed = self.repo.git('rev-parse', 'HEAD')
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, self.repo.tree(self.tested)))
        self.assertEqual(self.judge(squashed), (False, False))
        self.gh.pull_list[squashed] = [{'number': 1, 'head': {'sha': self.head}}]
        self.assertEqual(self.judge(squashed), (True, True))

    def test_a_pull_request_run_without_a_record_does_not_count(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(7, self.head, 'pull_request')
        self.assertEqual(self.judge(landed), (False, False))

    def test_a_pull_request_run_of_only_what_changed_does_not_count(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, self.repo.tree(self.tested), scope='trek-app'))
        self.assertEqual(self.judge(landed), (False, False))

    def test_a_record_of_another_run_or_head_does_not_count(self):
        landed = self.landed_on_unmoved_main()
        tree = self.repo.tree(self.tested)
        self.gh.run(7, self.head, 'pull_request', rec=record(6, self.head, tree))
        self.assertEqual(self.judge(landed), (False, False))
        self.gh = FakeGh()
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.m1, tree))
        self.assertEqual(self.judge(landed), (False, False))
        self.gh = FakeGh()
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, tree, event='push'))
        self.assertEqual(self.judge(landed), (False, False))

    def test_a_matching_pull_request_run_that_failed_does_not_count(self):
        landed = self.landed_on_unmoved_main()
        self.gh.run(7, self.head, 'pull_request', jobs=failed(PASSED, 'Test (macOS)'),
                    rec=record(7, self.head, self.repo.tree(self.tested)))
        self.assertEqual(self.judge(landed), (False, True))

    def test_a_run_of_an_earlier_head_of_the_pull_request_does_not_count(self):
        # The pull request's CI passed on its first commit; a second commit was pushed and merged.
        self.repo.git('checkout', '-q', 'pr')
        head2 = self.repo.commit('b.txt', 'pull request, fixed\n', 'the fix')
        self.repo.git('checkout', '-q', 'main')
        landed = self.repo.merge(self.m1, head2, 'Merge pull request #1')
        self.gh.run(7, self.head, 'pull_request', rec=record(7, self.head, self.repo.tree(self.tested)))
        self.assertEqual(self.judge(landed), (False, False))


class Record(unittest.TestCase):
    def test_it_records_the_checked_out_tree_and_the_pull_requests_head(self):
        with tempfile.TemporaryDirectory() as tmp:
            os.environ['GIT_CONFIG_GLOBAL'] = os.path.join(tmp, 'gitconfig')
            os.environ['GIT_CONFIG_NOSYSTEM'] = '1'
            repo = Repo(os.path.join(tmp, 'repo'))
            sha = repo.commit('a.txt', 'one\n', 'one')
            event = os.path.join(tmp, 'event.json')
            with open(event, 'w') as f:
                json.dump({'pull_request': {'head': {'sha': 'abc123'}}}, f)
            out = os.path.join(tmp, 'tested-tree.json')
            env = dict(os.environ, GITHUB_SHA=sha, GITHUB_EVENT_NAME='pull_request', GITHUB_EVENT_PATH=event,
                       GITHUB_RUN_ID='42')
            env.pop('GITHUB_STEP_SUMMARY', None)
            subprocess.run([sys.executable, os.path.join(ROOT, 'script', 'ci-gate.py'), 'record', out, '--scope', 'full', '--require', 'Plan'],
                           cwd=repo.path, env=env, check=True, capture_output=True)
            with open(out) as f:
                rec = json.load(f)
            self.assertEqual(rec['tree'], repo.tree(sha))
            self.assertEqual((rec['run_id'], rec['event'], rec['head'], rec['sha'], rec['scope'], rec['required']),
                             (42, 'pull_request', 'abc123', sha, 'full', ['Plan']))
            # It won't write a record that names no job that has to pass.
            p = subprocess.run([sys.executable, os.path.join(ROOT, 'script', 'ci-gate.py'), 'record', out],
                               cwd=repo.path, env=env, capture_output=True)
            self.assertNotEqual(p.returncode, 0)
            # A checkout that isn't the run's commit refuses to record.
            env['GITHUB_SHA'] = '0' * 40
            p = subprocess.run([sys.executable, os.path.join(ROOT, 'script', 'ci-gate.py'), 'record', out],
                               cwd=repo.path, env=env, capture_output=True)
            self.assertNotEqual(p.returncode, 0)


if __name__ == '__main__':
    unittest.main(verbosity=2)
