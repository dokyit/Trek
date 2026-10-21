//! What kind of file a changed path is (code, tests, docs, config, generated, assets, scripts),
//! and a diff's lines rolled up by kind: the card that opens over a "+504 −105" count breaks the
//! count down this way. Read from the path alone; nothing is opened.

/// A kind of file, in the order the breakdown lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    Code,
    Tests,
    Docs,
    Config,
    /// Lockfiles, snapshots, minified and protobuf output: written by tools, not by hand.
    Generated,
    Assets,
    Scripts,
    Other,
}

impl FileKind {
    pub const ALL: [FileKind; 8] = [Self::Code, Self::Tests, Self::Docs, Self::Config, Self::Generated, Self::Assets, Self::Scripts, Self::Other];

    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Tests => "Tests",
            Self::Docs => "Docs",
            Self::Config => "Config",
            Self::Generated => "Generated",
            Self::Assets => "Assets",
            Self::Scripts => "Scripts",
            Self::Other => "Other",
        }
    }

    /// A lower-case word for ids ("code", "tests", ...).
    pub fn key(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Tests => "tests",
            Self::Docs => "docs",
            Self::Config => "config",
            Self::Generated => "generated",
            Self::Assets => "assets",
            Self::Scripts => "scripts",
            Self::Other => "other",
        }
    }

    /// The kind of the file at `path` (relative or absolute, either slash). The first rule that
    /// fits wins: generated output, then tests (a fixture's JSON is a test's), docs, scripts,
    /// assets, config, code.
    pub fn of(path: &str) -> FileKind {
        let path = path.replace('\\', "/").to_ascii_lowercase();
        let mut dirs: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
        let name = dirs.pop().unwrap_or_default();
        // "a.test.ts" → stem "a", ext "ts"; ".gitignore" is a dotfile with no extension.
        let ext = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map(|(_, e)| e).unwrap_or_default();
        let first = name.split('.').next().unwrap_or_default();
        let stem = name.strip_suffix(&format!(".{ext}")).filter(|_| !ext.is_empty()).unwrap_or(name);
        let in_dir = |set: &[&str]| dirs.iter().any(|d| set.contains(d));

        if LOCKFILES.contains(&name)
            || ext == "lock"
            || ext == "snap"
            || GENERATED_SUFFIXES.iter().any(|s| name.ends_with(s))
            || name.contains(".generated.")
            || in_dir(&["__snapshots__", "__generated__"])
        {
            return Self::Generated;
        }
        if in_dir(TEST_DIRS)
            || name.contains(".test.")
            || name.contains(".spec.")
            || ["_test", "_tests", "_spec"].iter().any(|s| stem.ends_with(s))
            || (stem.starts_with("test_") && ext == "py")
            || (matches!(stem, "test" | "tests") && CODE.contains(&ext))
        {
            return Self::Tests;
        }
        if DOC_EXTS.contains(&ext) || (DOC_NAMES.contains(&first) && matches!(ext, "" | "txt")) {
            return Self::Docs;
        }
        if SCRIPT_EXTS.contains(&ext) {
            return Self::Scripts;
        }
        if ASSET_EXTS.contains(&ext) || dirs.iter().any(|d| d.ends_with(".xcassets")) {
            return Self::Assets;
        }
        if in_dir(&["doc", "docs"]) {
            return Self::Docs;
        }
        if in_dir(&["script", "scripts"]) {
            return Self::Scripts;
        }
        if in_dir(CONFIG_DIRS) || name.starts_with('.') || CONFIG_NAMES.contains(&name) || CONFIG_EXTS.contains(&ext) {
            return Self::Config;
        }
        if CODE.contains(&ext) {
            return Self::Code;
        }
        Self::Other
    }
}

const LOCKFILES: &[&str] = &[
    "cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "deno.lock",
    "poetry.lock",
    "pipfile.lock",
    "uv.lock",
    "gemfile.lock",
    "composer.lock",
    "go.sum",
    "flake.lock",
    "podfile.lock",
    "package.resolved",
    "mix.lock",
    "pubspec.lock",
    "gradle.lockfile",
    "packages.lock.json",
];
const GENERATED_SUFFIXES: &[&str] = &[".min.js", ".min.css", ".js.map", ".css.map", ".pb.go", ".pb.cc", ".pb.h", ".pb.swift", "_pb2.py", "_pb2_grpc.py", ".g.dart", ".freezed.dart"];
const TEST_DIRS: &[&str] = &["test", "tests", "spec", "specs", "__tests__", "__mocks__", "fixtures", "__fixtures__", "testdata", "test-fixtures", "test_fixtures", "e2e"];
const DOC_EXTS: &[&str] = &["md", "mdx", "markdown", "rst", "adoc", "asciidoc"];
const DOC_NAMES: &[&str] = &["readme", "changelog", "changes", "license", "licence", "contributing", "authors", "notice", "code_of_conduct"];
const SCRIPT_EXTS: &[&str] = &["sh", "bash", "zsh", "fish", "ps1", "psm1", "bat", "cmd"];
const ASSET_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "icns", "svg", "tif", "tiff", "avif", "heic", "ttf", "otf", "woff", "woff2", "eot", "mp3", "wav", "ogg", "flac", "m4a", "mp4", "mov", "webm", "psd", "sketch", "fig", "riv", "lottie",
];
const CONFIG_DIRS: &[&str] = &[".github", ".vscode", ".cargo", ".config", ".devcontainer", ".circleci", ".husky", ".idea"];
const CONFIG_NAMES: &[&str] = &["makefile", "dockerfile", "justfile", "cmakelists.txt", "procfile", "rust-toolchain", "gemfile", "podfile", "brewfile", "vagrantfile", "codeowners"];
const CONFIG_EXTS: &[&str] = &["toml", "yaml", "yml", "json", "jsonc", "json5", "ini", "cfg", "conf", "plist", "xcconfig", "entitlements", "properties", "gradle", "pbxproj", "xcscheme", "xcworkspacedata", "manifest", "iss", "wxs"];
const CODE: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "py", "pyi", "go", "swift", "kt", "kts", "java", "c", "h", "cc", "cpp", "cxx", "hh", "hpp", "cs", "fs", "rb", "php", "m", "mm", "lua", "zig", "scala", "ex", "exs", "erl", "hs", "ml", "mli",
    "dart", "vue", "svelte", "astro", "css", "scss", "sass", "less", "html", "htm", "sql", "wgsl", "metal", "glsl", "hlsl", "proto", "graphql", "gql", "r", "jl", "clj", "elm", "nim", "sol", "vb", "pl", "nix", "tf", "scm", "el", "vim",
];

/// One kind's lines in a breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindLines {
    pub kind: FileKind,
    pub added: u32,
    pub removed: u32,
}

/// A diff's added and removed lines by kind of file: a row per kind that has a changed file in
/// it (a binary image with no lines to count lists Assets at +0 −0), in `FileKind` order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineBreakdown {
    pub rows: Vec<KindLines>,
}

impl LineBreakdown {
    /// Roll up `(path, added, removed)` of each changed file.
    pub fn of<P: AsRef<str>>(files: impl IntoIterator<Item = (P, u32, u32)>) -> Self {
        let mut rows: Vec<KindLines> = vec![];
        for (path, added, removed) in files {
            let kind = FileKind::of(path.as_ref());
            match rows.iter_mut().find(|r| r.kind == kind) {
                Some(r) => (r.added, r.removed) = (r.added.saturating_add(added), r.removed.saturating_add(removed)),
                None => rows.push(KindLines { kind, added, removed }),
            }
        }
        rows.sort_by_key(|r| r.kind);
        Self { rows }
    }

    /// Roll up a turn's (or a review's) changed files.
    pub fn of_changes(files: &[crate::changes::FileChange]) -> Self {
        Self::of(files.iter().map(|f| (f.path.as_str(), f.added, f.removed)))
    }

    /// Lines added and removed over every kind.
    pub fn total(&self) -> (u32, u32) {
        self.rows.iter().fold((0u32, 0u32), |(a, r), k| (a.saturating_add(k.added), r.saturating_add(k.removed)))
    }

    /// "Code +3 −1" a row, then "Total +a −r": what the card shows, as text.
    pub fn describe(&self) -> Vec<String> {
        let (a, r) = self.total();
        self.rows.iter().map(|k| format!("{} +{} −{}", k.kind.label(), k.added, k.removed)).chain([format!("Total +{a} −{r}")]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::FileKind::{self, *};
    use super::LineBreakdown;

    fn kinds(paths: &[&str]) -> Vec<FileKind> {
        paths.iter().map(|p| FileKind::of(p)).collect()
    }

    #[test]
    fn source_files_are_code() {
        assert_eq!(kinds(&["src/main.rs", "crates/trek-app/src/panels/git.rs", "web/App.tsx", "lib/util.py", "ios/Trek/App.swift", "styles/site.css", "a.go"]), [Code; 7]);
    }

    #[test]
    fn tests_go_by_folder_and_by_name() {
        assert_eq!(
            kinds(&[
                "tests/parse.rs",
                "crates/trek-app/src/tests/changes.rs",
                "pkg/test/helpers.go",
                "web/__tests__/App.tsx",
                "spec/models/user_spec.rb",
                "fixtures/answer.json",
                "crates/test-fixtures/src/lib.rs",
                "web/Button.test.tsx",
                "web/api.spec.ts",
                "server/handler_test.go",
                "py/test_parse.py",
                "src/hunks/tests.rs",
                "C:\\work\\proj\\tests\\a.rs",
            ]),
            [Tests; 13]
        );
        // "test" inside a name isn't a test folder.
        assert_eq!(kinds(&["src/contest.rs", "src/testing_utils.rs", "latest/x.rs"]), [Code; 3]);
        // Nor is a README's name a doc when it's a source file's.
        assert_eq!(kinds(&["crates/trek-core/src/changes.rs", "src/license.rs"]), [Code; 2]);
    }

    #[test]
    fn docs_are_markdown_readmes_changelogs_and_the_docs_folder() {
        assert_eq!(kinds(&["README.md", "readme", "CHANGELOG", "CHANGELOG.txt", "docs/guide.html", ".agents/skills/x/SKILL.md", ".github/pull_request_template.md", "LICENSE"]), [Docs; 8]);
    }

    #[test]
    fn config_is_manifests_data_files_dotfiles_and_ci() {
        assert_eq!(
            kinds(&["Cargo.toml", "crates/trek-app/Cargo.toml", "package.json", "tsconfig.json", "config.yaml", "ci.yml", ".github/workflows/release.yml", ".github/CODEOWNERS", ".gitignore", ".env", ".eslintrc.js", "Makefile", "Info.plist"]),
            [Config; 13]
        );
    }

    #[test]
    fn lockfiles_and_tool_output_are_generated() {
        assert_eq!(
            kinds(&["Cargo.lock", "package-lock.json", "pnpm-lock.yaml", "yarn.lock", "go.sum", "src/__snapshots__/view.ts.snap", "tests/snapshots/a.snap", "dist/app.min.js", "api/service.pb.go", "gen/thing_pb2.py"]),
            [Generated; 10]
        );
    }

    #[test]
    fn images_fonts_and_icons_are_assets() {
        assert_eq!(kinds(&["assets/logo.png", "icon.svg", "macos/Trek.icns", "fonts/Inter.woff2", "ios/Assets.xcassets/AppIcon.appiconset/Contents.json"]), [Assets; 5]);
    }

    #[test]
    fn scripts_are_shell_files_and_the_scripts_folders() {
        assert_eq!(kinds(&["scripts/install-device.sh", "build.ps1", "script/bootstrap", "scripts/release.py", "run.bat"]), [Scripts; 5]);
    }

    #[test]
    fn anything_else_is_other() {
        assert_eq!(kinds(&["notes.txt", "data.csv", "binary.bin", ""]), [Other; 4]);
    }

    #[test]
    fn the_breakdown_sums_each_kind_and_lists_only_kinds_that_changed() {
        let b = LineBreakdown::of([("src/main.rs", 300, 80), ("README.md", 4, 1), ("src/lib.rs", 200, 20), ("tests/a.rs", 0, 4), ("Cargo.lock", 0, 0)]);
        assert_eq!(b.describe(), ["Code +500 −100", "Tests +0 −4", "Docs +4 −1", "Generated +0 −0", "Total +504 −105"]);
        assert_eq!(b.total(), (504, 105));
    }

    #[test]
    fn one_kind_still_has_its_row_and_a_total() {
        let b = LineBreakdown::of([("a.rs", 2, 0), ("b.rs", 1, 1)]);
        assert_eq!(b.describe(), ["Code +3 −1", "Total +3 −1"]);
        assert_eq!(LineBreakdown::of(Vec::<(String, u32, u32)>::new()).describe(), ["Total +0 −0"]);
    }
}
