#!/usr/bin/env python3
"""Tests for script/ci-cache.py: a cache never vouches for code its artifacts weren't built from.

A throwaway workspace (crate `a`, `c` on it and `d` on `c`, whose test reads `c`) in a git repository, built with cargo
as a CI job builds it: restore, build, then (only when the build passed) prune and save. The case
that matters: a job changes `c` and fails before cargo rebuilt it. Its cache must not be saved with
a list saying `c`'s source is what its artifact was built from, or the next run, whose `c` is that
same source, dates it old and takes the stale artifact as fresh: a broken change could pass.

    python3 script/tests/ci-cache.py
"""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CACHE = os.path.join(ROOT, 'script', 'ci-cache.py')


class Job:
    """A CI job's steps against one workspace and one saved cache."""

    def __init__(self, tmp):
        self.repo = os.path.join(tmp, 'repo')
        self.saved = os.path.join(tmp, 'saved-target')  # what actions/cache keeps
        self.home = os.path.join(tmp, 'cargo-home')
        os.makedirs(self.repo)
        self.env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(self.repo, 'target'), CARGO_INCREMENTAL='0',
                        CARGO_TERM_COLOR='never',
                        GIT_CONFIG_GLOBAL=os.path.join(tmp, 'gitconfig'), GIT_CONFIG_NOSYSTEM='1')
        self.env.pop('GITHUB_OUTPUT', None)
        # The repository's own toolchain: ci-cache.py reads target/ as that Cargo lays it out (a
        # newer Cargo's build-dir layout puts units elsewhere).
        shutil.copy(os.path.join(ROOT, 'rust-toolchain.toml'), os.path.join(self.repo, 'rust-toolchain.toml'))
        self.git('init', '-q')
        self.git('config', 'user.name', 't')
        self.git('config', 'user.email', 't@t')
        self.write('Cargo.toml', '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n\n'
                   '[workspace.package]\nversion = "0.1.0"\n')
        self.write('crates/a/Cargo.toml', '[package]\nname = "a"\nversion.workspace = true\nedition = "2021"\n')
        self.write('crates/c/Cargo.toml', '[package]\nname = "c"\nversion.workspace = true\nedition = "2021"\n\n'
                   '[dependencies]\na = { path = "../a" }\n')
        self.write('crates/d/Cargo.toml', '[package]\nname = "d"\nversion.workspace = true\nedition = "2021"\n\n'
                   '[dependencies]\nc = { path = "../c" }\n')
        self.write('crates/a/src/lib.rs', 'pub fn base() -> u32 { 1 }\n')
        self.value(1)
        os.makedirs(os.path.join(self.repo, 'vendor'))
        self.write('vendor/.keep', '')
        subprocess.run(['cargo', 'generate-lockfile', '--offline'], cwd=self.repo, env=self.env, check=True,
                       capture_output=True)
        self.commit('one')

    def write(self, path, text):
        full = os.path.join(self.repo, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, 'w') as f:
            f.write(text)

    def value(self, n):
        # The test is d's: a test binary is always linked anew (prune drops executables), so a stale
        # unit shows in a library that another crate's test links: c's.
        self.write('crates/c/src/lib.rs', f'pub fn value() -> u32 {{ a::base() * {n} }}\n')
        self.write('crates/d/src/lib.rs', f'#[test]\nfn c_is_current() {{ assert_eq!(c::value(), {n}); }}\n')

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.repo, env=self.env, check=True, capture_output=True, text=True).stdout

    def commit(self, msg):
        self.git('add', '-A')
        self.git('commit', '-q', '-m', msg)

    def checkout_fresh(self):
        """What actions/checkout gives a job: every file stamped now; then the cache restored."""
        now = None
        for base, dirs, files in os.walk(self.repo):
            dirs[:] = [d for d in dirs if d not in ('.git', 'target')]
            for f in files:
                os.utime(os.path.join(base, f), now)
        target = os.path.join(self.repo, 'target')
        shutil.rmtree(target, ignore_errors=True)
        if os.path.isdir(self.saved):
            shutil.copytree(self.saved, target, symlinks=True)

    def cache(self, *args):
        return subprocess.run([sys.executable, CACHE, *args], cwd=self.repo, env=self.env, capture_output=True, text=True)

    def test(self):
        """`cargo test -p d`: (passed, what it compiled)."""
        p = subprocess.run(['cargo', 'test', '-p', 'd', '--offline'], cwd=self.repo, env=self.env,
                           capture_output=True, text=True)
        return p.returncode == 0, p.stderr

    def run_job(self, fails_before_build=False, save_on_failure=False, old_list=False):
        """A job: checkout, restore, build and test; prune and save only when it passed. To show what
        the bug did: a job that failed before it built anything (a step before cargo, like a tool
        install) and saved anyway, with the list restore had written for its checkout."""
        self.checkout_fresh()
        r = self.cache('restore')
        assert r.returncode == 0, r.stdout + r.stderr
        passed, log = (False, '') if fails_before_build else self.test()
        if passed or save_on_failure:
            target = os.path.join(self.repo, 'target')
            if old_list:
                pending = os.path.join(target, 'trek-ci', 'vendor-blobs.pending')
                os.replace(pending, os.path.join(target, 'trek-ci', 'vendor-blobs'))
            else:
                p = self.cache('prune')
                assert p.returncode == 0, p.stdout + p.stderr
            shutil.rmtree(self.saved, ignore_errors=True)
            shutil.copytree(target, self.saved, symlinks=True)
        return passed, log


def pinned_cargo():
    """Whether the repository's pinned toolchain's cargo runs here."""
    if not shutil.which('cargo'):
        return False
    return subprocess.run(['cargo', '--version'], cwd=ROOT, capture_output=True).returncode == 0


@unittest.skipUnless(pinned_cargo(), "no cargo of the repository's toolchain here")
class Poisoning(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.job = Job(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def scenario(self, **failing_job):
        job = self.job
        passed, log = job.run_job()                       # main, all good: the cache is saved
        self.assertTrue(passed, log)
        job.value(2)                                     # c changes, and the job fails before
        job.commit('two')                                # cargo builds it
        passed, _ = job.run_job(fails_before_build=True, **failing_job)
        self.assertFalse(passed)
        job.write('notes.txt', 'unrelated\n')            # the next push: c as in "two"
        job.commit('three')
        return job.run_job()

    def test_a_failed_job_saves_nothing_and_the_next_rebuilds_what_it_didnt(self):
        passed, log = self.scenario()
        self.assertTrue(passed, f'c came back stale:\n{log}')
        self.assertIn('Compiling c', log)

    def test_the_old_way_takes_the_stale_unit(self):
        # Saved after the failure with the list restore wrote: the next run dates c's file old and
        # cargo takes c's artifact from "one" as fresh. This is what the test above guards against.
        passed, log = self.scenario(save_on_failure=True, old_list=True)
        self.assertFalse(passed, 'the stale artifact went unnoticed: the scenario no longer shows the bug')
        self.assertNotIn('Compiling c', log)

    def test_restore_leaves_the_saved_list_alone_and_prune_needs_restore(self):
        job = self.job
        job.run_job()
        manifest = os.path.join(job.saved, 'trek-ci', 'vendor-blobs')
        with open(manifest) as f:
            saved = f.read()
        job.value(3)
        job.commit('three')
        job.checkout_fresh()
        job.cache('restore')
        with open(os.path.join(job.repo, 'target', 'trek-ci', 'vendor-blobs')) as f:
            self.assertEqual(f.read(), saved, 'restore rewrote the list before anything was built')
        self.assertTrue(os.path.exists(os.path.join(job.repo, 'target', 'trek-ci', 'vendor-blobs.pending')))
        os.remove(os.path.join(job.repo, 'target', 'trek-ci', 'vendor-blobs.pending'))
        self.assertNotEqual(job.cache('prune').returncode, 0, 'prune saved a cache with no list of its build')


if __name__ == '__main__':
    unittest.main(verbosity=2)
