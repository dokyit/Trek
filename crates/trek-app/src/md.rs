//! How agent answers render: Trek's markdown style (quiet inline code, bordered code blocks) and
//! file-path chips — `src/main.rs` becomes a small bordered chip with a file icon that opens the
//! file, like T3 Code and Codex do.

use gpui_kit::component::text::{InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin, TextView, TextViewStyle, markdown_ast};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// Body text, code and table styling for transcript markdown.
pub fn style(cx: &App) -> TextViewStyle {
    let theme = cx.theme();
    let line = theme.foreground.opacity(0.09);
    let mut code_block = StyleRefinement::default();
    code_block = code_block.bg(theme.foreground.opacity(0.035)).border_1().border_color(line).rounded(px(10.)).px(px(14.)).py(px(12.)).text_size(px(12.5)).line_height(relative(1.55));
    let mut table = StyleRefinement::default();
    table = table.border_1().border_color(line).rounded(px(8.));
    let table_head = StyleRefinement::default().bg(theme.foreground.opacity(0.04));
    let table_cell = StyleRefinement::default().px(px(10.)).py(px(6.));
    let mut style = TextViewStyle::default();
    if theme.mode.is_dark() {
        // The component default is the light syntax palette, unreadable on Night.
        style.highlight_theme = gpui_kit::component::highlighter::HighlightTheme::default_dark();
    }
    style
        .paragraph_gap(rems(0.85))
        // A calm scale: answers use headings as section labels, not posters.
        .heading_font_size(|level, base| match level {
            1 => base * 1.45,
            2 => base * 1.25,
            3 => base * 1.1,
            _ => base,
        })
        .code_block(code_block)
        .table(table)
        .table_head(table_head)
        .table_cell(table_cell)
        .inline_code(HighlightStyle {
            background_color: Some(theme.foreground.opacity(0.075)),
            color: Some(theme.foreground.opacity(0.92)),
            ..Default::default()
        })
}

/// A markdown view for agent text in `cwd`, with Trek's style and path chips.
pub fn view(state: &Entity<gpui_kit::component::text::TextViewState>, cwd: Option<PathBuf>, cx: &App) -> TextView {
    TextView::new(state).selectable(true).style(style(cx)).plugin(PathChips { cwd }).code_block_actions(|block, _, cx| {
        // Language label and a copy button in the block's corner.
        let code = block.code().to_string();
        let lang = block.lang().map(|l| l.to_string()).unwrap_or_default();
        let muted = cx.theme().muted_foreground;
        h_flex()
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .gap(px(6.))
            .text_size(px(11.5))
            .text_color(muted)
            .when(!lang.is_empty(), |el| el.child(lang))
            .child(
                gpui_kit::component::button::Button::new("copy")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Copy).text_color(muted))
                    .tooltip("Copy")
                    .on_click(move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                        gpui_kit::component::WindowExt::push_notification(window, "Copied", cx);
                    }),
            )
    })
}

struct PathRef {
    /// What the agent wrote.
    raw: String,
    /// Resolved on disk, when it exists.
    resolved: Option<PathBuf>,
}

/// Inline code that names a file or folder.
fn looks_like_path(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 260 || s.contains(char::is_whitespace) && !s.starts_with('/') && !s.starts_with('~') {
        return false;
    }
    if s.contains("://") || s.starts_with('-') || s.contains(['(', ')', '{', '}', '=', ';', '<', '>', '|', '`', '"', '\'']) {
        return false;
    }
    let has_sep = s.contains('/');
    let file = s.trim_end_matches('/').rsplit('/').next().unwrap_or(s);
    let has_ext = file
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=6).contains(&ext.len()) && ext.starts_with(|c: char| c.is_ascii_alphabetic()) && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    let dotfile = file.starts_with('.') && file.len() > 1 && file[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
    (has_sep && (has_ext || s.ends_with('/') || s.starts_with('/') || s.starts_with("~/") || s.starts_with("./"))) || (has_ext && !file.contains("..")) || dotfile
}

fn resolve(raw: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let raw = raw.trim().split(':').next().unwrap_or(raw);
    let p = if let Some(rest) = raw.strip_prefix("~/") {
        trek_core::paths::home().join(rest)
    } else if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        cwd?.join(raw)
    };
    p.exists().then_some(p)
}

struct PathChips {
    cwd: Option<PathBuf>,
}

impl MarkdownPlugin for PathChips {
    fn name(&self) -> &str {
        "trek-path"
    }

    fn parse(&self, node: &markdown_ast::Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let markdown_ast::Node::InlineCode(code) = node else { return None };
        if !looks_like_path(&code.value) {
            return None;
        }
        let raw = code.value.trim().to_string();
        let resolved = resolve(&raw, self.cwd.as_deref());
        Some(MarkdownNode::new("trek-path", PathRef { raw: raw.clone(), resolved }).text(raw))
    }

    fn render_inline(&self, node: &MarkdownNode, context: &InlineRenderContext, _: &mut Window, cx: &mut App) -> Option<InlineElement> {
        let data = node.data::<PathRef>()?;
        let theme = cx.theme();
        let dir = data.raw.ends_with('/') || data.resolved.as_ref().is_some_and(|p| p.is_dir());
        let trimmed = data.raw.trim_end_matches('/');
        let label = trimmed.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(trimmed).to_string();
        let size = context.font_size() * 0.88;
        let height = context.line_height().min(context.font_size() * 1.5);
        let resolved = data.resolved.clone();
        let tooltip = data.resolved.as_ref().map(|p| trek_core::paths::tildify(p)).unwrap_or_else(|| data.raw.clone());
        let id = SharedString::from(format!("path-{}", data.raw));
        let chip = h_flex()
            .id(id)
            .h(height)
            .px(px(5.))
            .gap(px(4.))
            .rounded(px(5.))
            .border_1()
            .border_color(theme.foreground.opacity(0.13))
            .bg(theme.foreground.opacity(0.045))
            .text_size(size)
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.foreground.opacity(0.9))
            .child(if dir { Icon::new(IconName::Folder).size(size).text_color(theme.muted_foreground).into_any_element() } else { crate::file_icon::badge(trimmed, size, cx) })
            .child(label)
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx))
            .when_some(resolved, |el, path| {
                el.cursor_pointer().hover(|s| s.bg(cx.theme().foreground.opacity(0.09))).on_click(move |_, _, cx| {
                    if path.is_dir() {
                        cx.open_with_system(&path)
                    } else {
                        cx.reveal_path(&path)
                    }
                })
            });
        // Sit on the text baseline: the chip is a little taller than the glyphs around it.
        let baseline = (height + context.font_size() * 0.7) / 2.;
        Some(InlineElement::new(chip).with_baseline(baseline))
    }
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
    use super::looks_like_path;

    #[test]
    fn spots_paths() {
        for p in ["src/main.rs", "crates/trek-agents/src/codex.rs", "settings.local.json", "~/.codex/config.toml", "/Users/x/Trek/", "research_notes/", ".gitignore", "README.md"] {
            assert!(looks_like_path(p), "{p}");
        }
        for p in ["turn/start", "cargo build", "foo()", "x = 1", "https://a.com/b.js", "v1.2", "--flag", "a..b"] {
            assert!(!looks_like_path(p), "{p}");
        }
    }
}
