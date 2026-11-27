#!/usr/bin/env python3
"""Tests for script/ci-plan.py, which decides what a pull request's CI builds and tests: the crates
a change reaches (and everything depending on them), the platforms that compile it, and when the
full suite runs. Each case lays out a small workspace in a temporary folder.

    python3 script/tests/ci-plan.py
"""
import importlib.util
import os
import tempfile
import textwrap
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
spec = importlib.util.spec_from_file_location('ci_plan', os.path.join(ROOT, 'script', 'ci-plan.py'))
plan_mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plan_mod)

# name -> workspace crates it depends on
CRATES = {
    'trek-core': [],
    'trek-agents': ['trek-core'],
    'trek-app': ['trek-core', 'trek-agents', 'trek-phone'],
    'trek-mcp': ['trek-core', 'trek-phone'],
    'trek-phone': [],
    'trek-parity': [],
}


class Plan(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        packages = []
        for name, deps in CRATES.items():
            d = os.path.join(self.root, 'crates', name)
            os.makedirs(os.path.join(d, 'src'))
            self.write(f'crates/{name}/Cargo.toml', f'[package]\nname = "{name}"\n')
            self.write(f'crates/{name}/src/lib.rs', '// root\n')
            packages.append({'id': f'{name} 0.1', 'name': name, 'manifest_path': os.path.join(d, 'Cargo.toml'),
                             'dependencies': [{'name': x} for x in deps] + [{'name': 'serde'}]})
        self.meta = {'workspace_members': [p['id'] for p in packages], 'packages': packages}

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, path, text):
        full = os.path.join(self.root, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, 'w') as f:
            f.write(textwrap.dedent(text))

    def plan(self, files, event='pull_request', labels=()):
        ws = plan_mod.Workspace(self.meta, self.root)
        return plan_mod.outputs(plan_mod.plan(files, ws, event, labels))

    def platforms(self, out):
        return {p for p in plan_mod.PLATFORMS if out[p] == 'true'}

    # Which crates

    def test_an_app_change_tests_the_app_alone(self):
        out = self.plan(['crates/trek-app/src/lib.rs'])
        self.assertEqual((out['packages'], out['app'], out['others'], out['scope']), ('trek-app', 'true', '', 'partial'))
        self.assertEqual(self.platforms(out), {'macos', 'windows', 'linux'})

    def test_a_core_change_tests_everything_that_depends_on_it(self):
        out = self.plan(['crates/trek-core/src/lib.rs'])
        self.assertEqual(out['packages'], 'trek-agents trek-app trek-core trek-mcp')
        self.assertEqual(out['others'], '-p trek-agents -p trek-core -p trek-mcp')
        self.assertEqual(out['arm'], 'true')

    def test_a_leaf_change_skips_the_app(self):
        out = self.plan(['crates/trek-parity/src/lib.rs', 'docs/notes.md'])
        self.assertEqual((out['packages'], out['app'], out['arm']), ('trek-parity', 'false', 'false'))

    def test_a_crate_manifest_or_asset_counts_for_its_crate(self):
        self.assertEqual(self.plan(['crates/trek-phone/Cargo.toml'])['packages'], 'trek-app trek-mcp trek-phone')
        self.assertEqual(self.plan(['crates/trek-phone/assets/x.png'])['packages'], 'trek-app trek-mcp trek-phone')

    # The full suite

    def test_what_can_affect_every_crate_runs_the_full_suite(self):
        for path in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'vendor/gpui-base/src/lib.rs',
                     '.github/workflows/ci.yml', 'crates/trek-app/build.rs', 'crates/test-fixtures/src/lib.rs',
                     'script/ci-shard.py', 'script/linux/packages.sh', '.cargo/config.toml']:
            out = self.plan(['crates/trek-app/src/lib.rs', path])
            self.assertEqual(out['scope'], 'full', path)
            self.assertEqual(len(out['packages'].split()), len(CRATES), path)
            self.assertEqual(self.platforms(out), {'macos', 'windows', 'linux'}, path)
        self.assertEqual(self.plan(['Cargo.lock'])['audit'], 'true')

    def test_a_push_or_the_label_runs_the_full_suite(self):
        self.assertEqual(self.plan([], event='push')['scope'], 'full')
        self.assertEqual(self.plan([], event='workflow_dispatch')['scope'], 'full')
        self.assertEqual(self.plan(['docs/a.md'], labels=['ci:full'])['scope'], 'full')
        self.assertEqual(self.plan(['docs/a.md'], labels=['live'])['scope'], 'partial')

    def test_a_file_whose_readers_are_unknown_runs_the_full_suite(self):
        out = self.plan(['plugins/trail-notes/plugin.toml'])
        self.assertEqual(out['scope'], 'full')

    # Files outside the crates

    def test_docs_alone_build_nothing(self):
        out = self.plan(['docs/RELEASING.md', 'README.md', 'ios/Trek/App.swift', 'site/index.html', '.agents/x.md'])
        self.assertEqual((out['rust'], out['packages'], out['scope']), ('false', '', 'partial'))
        self.assertEqual(self.platforms(out), set())

    def test_a_file_a_crate_reads_counts_for_that_crate(self):
        self.write('crates/trek-parity/tests/parity.rs', 'let t = root.join("docs/parity/ledger.toml");\n')
        self.write('crates/trek-core/src/artifact.rs', 'include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ios/Trek/Views/LivePage.swift"))\n')
        self.write('crates/trek-phone/src/fonts.rs', 'const DIR: &str = "../../assets/fonts";\n')
        self.assertEqual(self.plan(['docs/parity/ledger.toml'])['packages'], 'trek-parity')
        self.assertEqual(self.plan(['ios/Trek/Views/LivePage.swift'])['packages'], 'trek-agents trek-app trek-core trek-mcp')
        self.assertEqual(self.plan(['assets/fonts/x/Regular.ttf'])['packages'], 'trek-app trek-mcp trek-phone')
        # Naming a different file in the same folder isn't reading this one.
        self.assertEqual(self.plan(['ios/Trek/App.swift'])['packages'], '')
        self.assertEqual(self.plan(['ios/Trek/Views/Other.swift'])['packages'], '')

    def test_script_changes_run_the_script_tests(self):
        out = self.plan(['script/release.sh'])
        self.assertEqual((out['scripts'], out['rust']), ('true', 'false'))
        self.assertEqual(self.plan(['site/index.html'])['scripts'], 'true')
        self.assertEqual(self.plan(['crates/trek-app/src/lib.rs'])['scripts'], 'false')

    # Platforms

    def gated(self, attrs, name='platform', parent='crates/trek-app/src/lib.rs'):
        self.write(parent, f'pub mod other;\n\n{attrs}\nmod {name};\n')
        return self.platforms(self.plan([f'crates/trek-app/src/{name}.rs']))

    def test_a_module_gated_to_one_platform_runs_only_there(self):
        self.assertEqual(self.gated('#[cfg(target_os = "windows")]'), {'windows'})
        self.assertEqual(self.gated('#[cfg(windows)]'), {'windows'})
        self.assertEqual(self.gated('#[cfg(target_os = "macos")]'), {'macos'})
        self.assertEqual(self.gated('#[cfg(unix)]'), {'macos', 'linux'})
        self.assertEqual(self.gated('#[cfg(not(windows))]'), {'macos', 'linux'})
        self.assertEqual(self.gated('#[cfg(any(target_os = "linux", target_os = "macos"))]'), {'macos', 'linux'})
        self.assertEqual(self.gated('#[cfg(all(windows, feature = "shots"))]'), {'windows'})
        self.assertEqual(self.gated('/// Docs.\n#[cfg(target_os = "linux")]\n#[allow(dead_code)]'), {'linux'})

    def test_what_the_check_cannot_follow_runs_everywhere(self):
        every = {'macos', 'windows', 'linux'}
        self.assertEqual(self.gated(''), every)
        self.assertEqual(self.gated('#[cfg(any(feature = "shots", windows))]'), every)
        self.assertEqual(self.gated('#[cfg(not(all(windows, feature = "shots")))]'), every)
        self.assertEqual(self.gated('#[cfg_attr(windows, path = "win.rs")]'), every)
        self.assertEqual(self.gated('#[path = "elsewhere.rs"]\n#[cfg(windows)]'), every)
        self.assertEqual(self.gated('#[cfg(any(\n    windows,\n    target_os = "linux"\n))]'), every)
        self.assertEqual(self.gated('#[cfg(weird stuff)]'), every)
        # No `mod` line for it anywhere: compiled somehow we can't see (include!, a macro).
        self.write('crates/trek-app/src/lib.rs', '// nothing\n')
        self.assertEqual(self.platforms(self.plan(['crates/trek-app/src/stray.rs'])), every)
        # Tests, examples and build files are compiled on every platform.
        self.assertEqual(self.platforms(self.plan(['crates/trek-app/tests/windows.rs'])), every)

    def test_gates_of_enclosing_modules_narrow_it(self):
        self.write('crates/trek-app/src/lib.rs', '#[cfg(unix)]\nmod platform;\n')
        self.write('crates/trek-app/src/platform/mod.rs', '#[cfg(target_os = "linux")]\nmod gtk;\nmod shared;\n')
        self.assertEqual(self.platforms(self.plan(['crates/trek-app/src/platform/gtk.rs'])), {'linux'})
        self.assertEqual(self.platforms(self.plan(['crates/trek-app/src/platform/shared.rs'])), {'macos', 'linux'})
        self.assertEqual(self.platforms(self.plan(['crates/trek-app/src/platform/mod.rs'])), {'macos', 'linux'})
        # A file in a gated folder that an ungated file also changes alongside: both count.
        out = self.plan(['crates/trek-app/src/platform/gtk.rs', 'crates/trek-app/src/lib.rs'])
        self.assertEqual(self.platforms(out), {'macos', 'windows', 'linux'})

    def test_a_platform_only_change_still_tests_the_crates_that_depend_on_it(self):
        self.write('crates/trek-core/src/lib.rs', '#[cfg(windows)]\nmod win;\n')
        out = self.plan(['crates/trek-core/src/win.rs'])
        self.assertEqual(self.platforms(out), {'windows'})
        self.assertEqual(out['packages'], 'trek-agents trek-app trek-core trek-mcp')
        self.assertEqual(out['arm'], 'false')

    def test_this_repositorys_gates_read(self):
        # The real workspace: a macOS-only module of the app is found as such.
        ws = plan_mod.Workspace({'workspace_members': ['a'], 'packages': [
            {'id': 'a', 'name': 'trek-app', 'manifest_path': os.path.join(ROOT, 'crates', 'trek-app', 'Cargo.toml'),
             'dependencies': []}]}, ROOT)
        path = 'crates/trek-app/src/panels/browser_adblock/page.rs'
        if os.path.exists(os.path.join(ROOT, path)):
            out = plan_mod.outputs(plan_mod.plan([path], ws, 'pull_request'))
            self.assertEqual(self.platforms(out), {'macos'})


if __name__ == '__main__':
    unittest.main(verbosity=1)
