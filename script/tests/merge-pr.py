#!/usr/bin/env python3
"""Tests for script/live-plan.py (what counts as app code for the live checks) and script/merge-pr
(it won't merge a pull request whose head lacks a passing CI run, or, for app code, a passing Live
run on all three platforms). GitHub's API is a stand-in.

    python3 script/tests/merge-pr.py
"""
import importlib.machinery
import importlib.util
import os
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def load(name, path):
    loader = importlib.machinery.SourceFileLoader(name, path)
    spec = importlib.util.spec_from_loader(name, loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


plan = load('live_plan', os.path.join(ROOT, 'script', 'live-plan.py'))
merge = load('merge_pr', os.path.join(ROOT, 'script', 'merge-pr'))

SHA = 'a' * 40
LIVE_OK = [{'name': n, 'conclusion': 'success'} for n in merge.LIVE_JOBS] + [{'name': 'Live plan', 'conclusion': 'success'}]


class AppCode(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        os.makedirs(os.path.join(self.root, 'crates', 'trek-app', 'src'))
        with open(os.path.join(self.root, 'crates', 'trek-app', 'src', 'phone.rs'), 'w') as f:
            f.write('const S: &str = include_str!("../../../ios/scripts/phone-update.sh");\n'
                    'fn t() { std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brand")); }\n')

    def tearDown(self):
        self.tmp.cleanup()

    def app(self, *files):
        return plan.app_code(list(files), self.root)

    def test_docs_the_site_and_the_iphone_app_are_not_app_code(self):
        self.assertFalse(self.app('docs/CI.md', 'README.md', 'site/index.html', 'ios/Trek/App.swift',
                                  '.github/workflows/ci.yml', 'media/x.mp4', 'LICENSE',
                                  'script/ci-gate.py', 'script/tests/ci-gate.py', 'script/site/stamp.py',
                                  '.config/nextest.toml', '.github/actions/rust-setup/action.yml'))

    def test_anything_in_a_crate_is_app_code_markdown_too(self):
        self.assertTrue(self.app('crates/trek-core/skills/create-verification-skill/SKILL.md'))
        self.assertTrue(self.app('crates/trek-app/README.md'))
        self.assertTrue(self.app('docs/a.md', 'crates/trek-app/src/lib.rs'))

    def test_a_file_outside_the_crates_that_a_crate_reads_is_app_code(self):
        self.assertTrue(self.app('ios/scripts/phone-update.sh'))
        self.assertTrue(self.app('assets/brand/mark.svg'))
        self.assertFalse(self.app('ios/scripts/other.sh'))

    def test_a_workflow_is_not_app_code_even_where_a_crate_names_it(self):
        with open(os.path.join(self.root, 'crates', 'trek-app', 'src', 'kinds.rs'), 'w') as f:
            f.write('assert_eq!(kind(".github/workflows/ci.yml"), Kind::Yaml);\n')
        self.assertFalse(self.app('.github/workflows/ci.yml'))

    def test_the_ledger_live_yml_and_unknown_places_are_app_code(self):
        self.assertTrue(self.app('docs/parity/ledger.toml'))
        self.assertTrue(self.app('.github/workflows/live.yml'))
        self.assertTrue(self.app('plugins/trail-notes/plugin.toml'))
        self.assertTrue(self.app('script/linux/packages.sh'))
        self.assertTrue(self.app('Cargo.lock'))


class FakeGh:
    def __init__(self, files, ci=(('completed', 'success'),), live=()):
        self.pr_data = {'state': 'open', 'draft': False, 'merged': False, 'head': {'sha': SHA}}
        self.file_list = files
        self.ci = [{'id': 100 + i, 'status': s, 'conclusion': c, 'created_at': f'2026-11-28T0{i}:00:00Z'}
                   for i, (s, c) in enumerate(ci)]
        self.live = [{'id': 200 + i, 'status': 'completed', 'conclusion': 'success', 'created_at': f'2026-11-28T0{i}:00:00Z'}
                     for i in range(len(live))]
        self.live_jobs = {200 + i: jobs for i, jobs in enumerate(live)}
        self.merged = None

    def pr(self, n):
        return self.pr_data

    def files(self, n):
        return self.file_list

    def runs(self, workflow, sha):
        assert sha == SHA
        return self.ci if workflow == 'ci.yml' else self.live

    def jobs(self, run_id):
        return self.live_jobs[run_id]


class MergePr(unittest.TestCase):
    def verdict(self, gh):
        return merge.verdict(7, gh, ROOT)[0]

    def test_an_app_change_with_ci_and_live_passed_merges(self):
        self.assertTrue(self.verdict(FakeGh(['crates/trek-app/src/lib.rs'], live=[LIVE_OK])))

    def test_an_app_change_without_a_live_run_does_not(self):
        self.assertFalse(self.verdict(FakeGh(['crates/trek-app/src/lib.rs'])))

    def test_a_live_run_with_a_platform_that_failed_or_skipped_does_not_count(self):
        for bad in ('failure', 'skipped', 'cancelled'):
            jobs = [dict(j, conclusion=bad) if j['name'] == 'Live (Windows)' else j for j in LIVE_OK]
            self.assertFalse(self.verdict(FakeGh(['crates/trek-app/src/lib.rs'], live=[jobs])), bad)
        # An older failed run beside a passing one: the passing one counts.
        jobs = [dict(j, conclusion='failure') for j in LIVE_OK]
        self.assertTrue(self.verdict(FakeGh(['crates/trek-app/src/lib.rs'], live=[jobs, LIVE_OK])))

    def test_a_docs_change_needs_no_live_run(self):
        self.assertTrue(self.verdict(FakeGh(['docs/CI.md'])))
        self.assertTrue(self.verdict(FakeGh(['docs/CI.md'], ci=())))

    def test_ci_must_have_passed_on_the_head(self):
        for ci in ((('completed', 'failure'),), (('in_progress', None),), (('completed', 'success'), ('completed', 'failure'))):
            self.assertFalse(self.verdict(FakeGh(['crates/trek-app/src/lib.rs'], ci=ci, live=[LIVE_OK])), ci)
        # The newest run decides: a rerun that passed after a failure.
        self.assertTrue(self.verdict(FakeGh(['crates/trek-app/src/lib.rs'], ci=(('completed', 'failure'), ('completed', 'success')), live=[LIVE_OK])))
        # No CI run at all on an app change.
        self.assertFalse(self.verdict(FakeGh(['crates/trek-app/src/lib.rs'], ci=(), live=[LIVE_OK])))

    def test_a_draft_or_closed_pull_request_does_not_merge(self):
        gh = FakeGh(['docs/CI.md'])
        gh.pr_data['draft'] = True
        self.assertFalse(self.verdict(gh))
        gh = FakeGh(['docs/CI.md'])
        gh.pr_data['state'] = 'closed'
        self.assertFalse(self.verdict(gh))


if __name__ == '__main__':
    unittest.main(verbosity=1)
