//! What language a file is in: the name the status bar shows, and the grammar the editor
//! highlights it with. gpui-kit picks a grammar by a short name (`rust`, `csharp`, `bash`…) and
//! knows only a few spellings of each; a file's extension (`.h`, `.mjs`, `.zsh`, `.cc`) or its
//! whole name (`Makefile`, `CMakeLists.txt`) is mapped here, so what people open is highlighted.

use std::path::Path;

/// A language: the name the status bar shows and the grammar that colours it (`text`: none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Language {
    pub name: &'static str,
    pub grammar: &'static str,
    /// Indented with tab characters, which the Tab key types (Go, Makefiles: a recipe indented
    /// with spaces is an error to `make`, and `gofmt` writes tabs); else with spaces.
    pub hard_tabs: bool,
}

const fn lang(name: &'static str, grammar: &'static str) -> Language {
    Language { name, grammar, hard_tabs: false }
}

const fn tabbed(name: &'static str, grammar: &'static str) -> Language {
    Language { name, grammar, hard_tabs: true }
}

const PLAIN: Language = lang("Plain Text", "text");

/// gpui-kit 0.7 links the C#, Swift and CMake grammars without a highlight query for them, so
/// they parse and colour nothing; give each the query its grammar crate carries. Its TSX has only
/// TypeScript's additions to JavaScript's query (no keywords, no tags), its C++ only C++'s
/// additions to C's, and its JavaScript has no JSX: those get the queries they build on too. Once, before any editor is made (the
/// registry is the process's).
pub fn register_highlights() {
    use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let registry = LanguageRegistry::singleton();
        if let Some(mut config) = registry.language("javascript") {
            config.highlights = format!("{}\n{}", config.highlights, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY).into();
            registry.register("javascript", &config);
        }
        // C++'s query is C's additions: `int`, `return` and `if` are C's.
        if let Some(mut config) = registry.language("cpp") {
            config.highlights = format!("{}\n{}", tree_sitter_c::HIGHLIGHT_QUERY, config.highlights).into();
            registry.register("cpp", &config);
        }
        if let Some(mut config) = registry.language("tsx") {
            config.highlights = format!("{}\n{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, config.highlights).into();
            registry.register("tsx", &config);
        }
        registry.register("csharp", &LanguageConfig::new("csharp", tree_sitter_c_sharp::LANGUAGE.into(), vec![], tree_sitter_c_sharp::HIGHLIGHTS_QUERY, "", ""));
        // Kotlin comes out plain with gpui-kit's own query (no colour in the test below); the grammar crate's colours it.
        registry.register("kotlin", &LanguageConfig::new("kotlin", tree_sitter_kotlin_sg::LANGUAGE.into(), vec![], tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY, "", ""));
        registry.register("swift",&LanguageConfig::new("swift", tree_sitter_swift::LANGUAGE.into(), vec![], tree_sitter_swift::HIGHLIGHTS_QUERY, "", ""));
        registry.register("cmake", &LanguageConfig::new("cmake", tree_sitter_cmake::LANGUAGE.into(), vec![], tree_sitter_cmake::HIGHLIGHTS_QUERY, "", ""));
    });
}

/// The language of the file at `path`: by its name first (`Makefile`), then its extension, in
/// whatever case the file system keeps it (`MAIN.CS` on Windows).
pub fn of(path: &Path) -> Language {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    match name.as_str() {
        "makefile" | "gnumakefile" => return tabbed("Makefile", "make"),
        "cmakelists.txt" => return lang("CMake", "cmake"),
        "dockerfile" => return lang("Dockerfile", "text"),
        ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" => return lang("Shell Script", "bash"),
        "gemfile" | "rakefile" => return lang("Ruby", "ruby"),
        _ => {}
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => lang("Rust", "rust"),
        "swift" => lang("Swift", "swift"),
        "ts" | "mts" | "cts" => lang("TypeScript", "typescript"),
        "tsx" => lang("TypeScript JSX", "tsx"),
        "js" | "mjs" | "cjs" => lang("JavaScript", "javascript"),
        "jsx" => lang("JavaScript JSX", "javascript"),
        "py" | "pyi" | "pyw" => lang("Python", "python"),
        "go" => tabbed("Go", "go"),
        "c" | "h" => lang("C", "c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => lang("C++", "cpp"),
        "cs" => lang("C#", "csharp"),
        "java" => lang("Java", "java"),
        "kt" | "kts" => lang("Kotlin", "kotlin"),
        "scala" | "sc" => lang("Scala", "scala"),
        "rb" => lang("Ruby", "ruby"),
        "php" | "phtml" => lang("PHP", "php"),
        "lua" => lang("Lua", "lua"),
        "zig" => lang("Zig", "zig"),
        "ex" | "exs" => lang("Elixir", "elixir"),
        "sql" => lang("SQL", "sql"),
        // Named, but with no grammar: gpui-kit 0.7 has no highlight query for these two, so a
        // grammar would parse them and colour nothing.
        "graphql" | "gql" => lang("GraphQL", "text"),
        "proto" => lang("Protocol Buffers", "text"),
        "svelte" => lang("Svelte", "svelte"),
        "astro" => lang("Astro", "astro"),
        "json" => lang("JSON", "json"),
        "jsonc" => lang("JSON with Comments", "json"),
        "toml" => lang("TOML", "toml"),
        "yaml" | "yml" => lang("YAML", "yaml"),
        "md" | "markdown" | "mdx" => lang("Markdown", "markdown"),
        "html" | "htm" => lang("HTML", "html"),
        "css" => lang("CSS", "css"),
        "scss" => lang("SCSS", "css"),
        "sh" | "bash" | "zsh" => lang("Shell Script", "bash"),
        "mk" => tabbed("Makefile", "make"),
        "cmake" => lang("CMake", "cmake"),
        "diff" | "patch" => lang("Diff", "diff"),
        // Named as the user knows them, though nothing here colours them yet.
        "ps1" | "psm1" | "psd1" => lang("PowerShell", "text"),
        "ini" | "cfg" | "conf" => lang("INI", "text"),
        "bat" | "cmd" => lang("Batch", "text"),
        "xml" | "csproj" | "props" | "xaml" => lang("XML", "text"),
        _ => PLAIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(file: &str) -> (&'static str, &'static str) {
        let l = of(Path::new(file));
        (l.name, l.grammar)
    }

    #[test]
    fn a_file_is_named_and_coloured_by_its_extension() {
        assert_eq!(named("main.rs"), ("Rust", "rust"));
        assert_eq!(named("a.mts"), ("TypeScript", "typescript"));
        assert_eq!(named("a.jsx"), ("JavaScript JSX", "javascript"));
        assert_eq!(named("a.mjs"), ("JavaScript", "javascript"));
        assert_eq!(named("util.h"), ("C", "c"));
        assert_eq!(named("util.cc"), ("C++", "cpp"));
        assert_eq!(named("Program.cs"), ("C#", "csharp"));
        assert_eq!(named("q.sql"), ("SQL", "sql"));
        assert_eq!(named("index.php"), ("PHP", "php"));
        assert_eq!(named("schema.graphql"), ("GraphQL", "text"));
        assert_eq!(named("m.proto"), ("Protocol Buffers", "text"));
        assert_eq!(named("build.zig"), ("Zig", "zig"));
        assert_eq!(named("A.scala"), ("Scala", "scala"));
        assert_eq!(named("a.exs"), ("Elixir", "elixir"));
        assert_eq!(named("App.svelte"), ("Svelte", "svelte"));
        assert_eq!(named("Page.astro"), ("Astro", "astro"));
        assert_eq!(named("run.zsh"), ("Shell Script", "bash"));
        assert_eq!(named("a.yml"), ("YAML", "yaml"));
    }

    #[test]
    fn json_is_highlighted_and_jsonc_says_what_it_is() {
        assert_eq!(named("package.json"), ("JSON", "json"));
        assert_eq!(named("tsconfig.jsonc"), ("JSON with Comments", "json"));
    }

    #[test]
    fn a_file_without_an_extension_is_known_by_its_name() {
        assert_eq!(named("Makefile"), ("Makefile", "make"));
        assert_eq!(named("proj/CMakeLists.txt"), ("CMake", "cmake"));
        assert_eq!(named("rules.mk"), ("Makefile", "make"));
        assert_eq!(named("x.cmake"), ("CMake", "cmake"));
        // Plain `.txt` is plain text, CMakeLists.txt's name aside.
        assert_eq!(named("notes.txt"), ("Plain Text", "text"));
    }

    #[test]
    fn what_has_no_grammar_here_is_still_named() {
        assert_eq!(named("build.ps1"), ("PowerShell", "text"));
        assert_eq!(named("Mod.psm1"), ("PowerShell", "text"));
        assert_eq!(named("launcher_imgui.ini"), ("INI", "text"));
        assert_eq!(named("setup.cfg"), ("INI", "text"));
        assert_eq!(named("run.bat"), ("Batch", "text"));
    }

    #[test]
    fn the_case_a_windows_file_keeps_doesnt_matter() {
        assert_eq!(named("MAIN.CS"), ("C#", "csharp"));
        assert_eq!(named("MAKEFILE"), ("Makefile", "make"));
        assert_eq!(named("src/App.TSX"), ("TypeScript JSX", "tsx"));
    }

    /// What the editor does with a file: its grammar's name goes to gpui-kit, whose highlighter
    /// colours the code if that grammar is compiled in (else it falls back to plain text, silently).
    fn colours(file: &str, code: &str) -> usize {
        use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};
        register_highlights();
        let mut highlighter = SyntaxHighlighter::new(of(Path::new(file)).grammar);
        let rope = ropey::Rope::from_str(code);
        highlighter.update(None, &rope, None);
        // Spans with a colour of their own: plain text still comes back as one uncoloured span.
        highlighter.styles(&(0..code.len()), &*HighlightTheme::default_dark()).iter().filter(|(_, style)| style.color.is_some()).count()
    }

    #[test]
    fn every_language_with_a_grammar_colours_its_code() {
        let mut plain = vec![];
        for (file, code) in [
            ("main.rs", "fn main() { let x = 1; }"),
            ("a.ts", "const x: number = 1;"),
            ("a.tsx", "const a = <div>{1}</div>;"),
            ("a.js", "const x = 1;"),
            ("a.mjs", "export const x = 1;"),
            ("a.jsx", "const a = <b>hi</b>;"),
            ("a.py", "def f(x):\n    return 1\n"),
            ("a.go", "package main\nfunc main() {}\n"),
            ("a.c", "int main() { return 0; }"),
            ("a.h", "int main(void);"),
            ("a.cpp", "int main() { return 0; }"),
            ("Program.cs", "class A { static void Main() { var x = 1; } }"),
            ("A.java", "class A { void f() { int x = 1; } }"),
            ("a.kt", "fun f() { val x = 1 }"),
            ("a.scala", "object A { def f = 1 }"),
            ("a.rb", "def f\n  1\nend\n"),
            ("a.php", "<?php function f() { return 1; }"),
            ("a.lua", "local x = 1"),
            ("a.zig", "pub fn main() void {}"),
            ("a.ex", "defmodule A do\n  def f, do: 1\nend\n"),
            ("q.sql", "SELECT a, b FROM t WHERE a = 1;"),
            ("a.svelte", "<script>let x = 1;</script>"),
            ("a.astro", "---\nconst x = 1;\n---\n<div></div>"),
            ("a.json", "{\"a\": [1, true]}"),
            ("a.jsonc", "{\"a\": 1} // note"),
            ("a.toml", "a = 1\n"),
            ("a.yaml", "a: 1\n"),
            ("a.md", "# Title\n\ntext\n"),
            ("a.html", "<div class=\"a\"></div>"),
            ("a.css", "a { color: red; }"),
            ("run.sh", "echo \"hi\"\n"),
            ("Makefile", "all:\n\techo hi\n"),
            ("CMakeLists.txt", "project(a)\n"),
            ("a.diff", "--- a\n+++ b\n@@ -1 +1 @@\n-a\n+b\n"),
            ("a.swift", "let x = 1"),
        ] {
            if colours(file, code) == 0 {
                plain.push(format!("{file} ({})", of(Path::new(file)).grammar));
            }
        }
        assert!(plain.is_empty(), "not coloured (no grammar compiled in, or none that colours): {plain:?}");
    }

    #[test]
    fn what_has_no_grammar_stays_plain_rather_than_pretending() {
        assert_eq!(colours("build.ps1", "Write-Host \"hi\""), 0);
        assert_eq!(colours("a.ini", "[a]\nb=1\n"), 0);
        assert_eq!(colours("notes.txt", "hello"), 0);
        assert_eq!(colours("a.graphql", "type A { b: Int }"), 0);
        assert_eq!(colours("a.proto", "message A { int32 b = 1; }"), 0);
    }

    #[test]
    fn go_and_makefiles_are_indented_with_tabs_and_the_rest_with_spaces() {
        for tabs in ["main.go", "Makefile", "rules.mk", "GNUmakefile"] {
            assert!(of(Path::new(tabs)).hard_tabs, "{tabs}");
        }
        for spaces in ["main.rs", "a.py", "a.cs", "a.ts", "CMakeLists.txt", "notes.txt", "a.yaml"] {
            assert!(!of(Path::new(spaces)).hard_tabs, "{spaces}");
        }
    }

    #[test]
    fn nothing_known_is_plain_text() {
        assert_eq!(named("README"), ("Plain Text", "text"));
        assert_eq!(named("data.xyz"), ("Plain Text", "text"));
        assert_eq!(named(""), ("Plain Text", "text"));
    }
}
