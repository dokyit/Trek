//! The follow-ups queued behind a running turn, each in its own row above the prompt: moved up or
//! down, taken back into the prompt to change, or removed. The row's controls show as the pointer
//! comes to it, so a queue reads as a list of what's coming and nothing else.

use super::Composer;
use crate::scale::text;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use trek_core::orchestrate::Consult;

/// A queued follow-up being changed in the prompt. It's off the queue meanwhile, so it can't go
/// out half-edited as the turn ends; sending puts it back where it was.
pub(super) struct QueueEdit {
    pub(super) thread: String,
    ix: usize,
    /// As it was queued: back in its place on cancel.
    original: (String, Vec<PathBuf>),
    /// What the prompt held before: back once it's put away.
    draft: (String, Vec<PathBuf>),
    /// The consultants picked and Restate first as they were: a message that asked for them brings
    /// them along while it's changed, and they stay with it.
    asks: (Consult, bool),
}

/// What a row says of a message: its first line as the user wrote it (without what Trek adds for
/// consultants or a restatement, or the files mentioned after it), or what goes with it when it
/// has no text: its images, a file's name, or how many files.
pub(super) fn first_line(text: &str, images: usize) -> String {
    let (text, _) = trek_core::orchestrate::split_consult(text);
    let (text, _) = trek_core::restate::split_restate(text);
    let (text, files) = trek_core::media::split_attached(text, std::path::Path::exists);
    if let Some(line) = text.lines().find(|l| !l.trim().is_empty()) {
        return line.trim().to_string();
    }
    let images = format!("{images} image{}", if images == 1 { "" } else { "s" });
    let files = match files.as_slice() {
        [] => return images,
        [one] => one.file_name().map_or_else(
            || one.display().to_string(),
            |n| n.to_string_lossy().to_string(),
        ),
        many => format!("{} files", many.len()),
    };
    if images.starts_with("0 ") {
        files
    } else {
        format!("{images}, {files}")
    }
}

impl Composer {
    /// `edit_queued`, for tests: the row's pencil.
    #[cfg(any(test, feature = "test-support"))]
    pub fn edit_queued_for_test(
        &mut self,
        thread: &str,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_queued(thread, ix, window, cx);
    }

    /// Take queued message `ix` of `thread` into the prompt to change it.
    fn edit_queued(
        &mut self,
        thread: &str,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_queue_edit(window, cx);
        self.cancel_edit(window, cx);
        let Some((text, images)) = self
            .workspace
            .update(cx, |ws, cx| ws.queue_take(thread, ix, cx))
        else {
            return;
        };
        let draft = self.take_written(cx);
        self.queue_edit = Some(QueueEdit {
            thread: thread.to_string(),
            ix,
            original: (text.clone(), images.clone()),
            draft,
            asks: (self.consult.clone(), self.restate),
        });
        // As it was written, what it asked for picked again: sending adds them back.
        let (written, consult) = trek_core::orchestrate::split_consult(&text);
        if let Some(c) = consult {
            self.consult = c;
        }
        let (written, restate) = trek_core::restate::split_restate(written);
        self.restate |= restate;
        let written = written.to_string();
        self.put_back(&written, images, Some(written.len()), window, cx);
        self.pickers.trigger = None;
        self.sync_overlay(cx);
        self.focus(window, cx);
        cx.notify();
    }

    /// Stop changing a queued message: it goes back as it was, and what the prompt held before
    /// comes back. Its turn over, there's no queue to go back to: it stays in the prompt, ahead of
    /// what was there, for the user to send or not.
    pub(super) fn cancel_queue_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.queue_edit.take() else {
            return;
        };
        if self.workspace.read(cx).queue_waits(&e.thread) {
            let (text, images) = e.original;
            self.workspace
                .update(cx, |ws, cx| ws.queue_put(&e.thread, e.ix, text, images, cx));
            (self.consult, self.restate) = e.asks;
            self.bring_back_draft(e.draft, window, cx);
        } else {
            let (draft, images) = e.draft;
            self.outbox.paths.extend(images);
            // Its files go back on the strip, beside the edited message's.
            let (draft, files) = trek_core::media::split_attached(&draft, std::path::Path::exists);
            self.outbox.add_files(files);
            if !draft.is_empty() {
                self.input.update(cx, |s, cx| {
                    let text = format!("{}\n\n{draft}", s.value());
                    s.set_value(text, window, cx);
                });
            }
        }
        cx.notify();
    }

    /// A message is going while a queued one is being changed. Returns whether that was all: it's
    /// put back in its place in the queue, and what the prompt held before comes back. Its turn
    /// over, it goes as any message does (and `after_send` brings that back).
    pub(super) fn send_queue_edit(
        &mut self,
        text: &str,
        images: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let waits = self
            .queue_edit
            .as_ref()
            .is_some_and(|e| self.workspace.read(cx).queue_waits(&e.thread));
        if !waits {
            return false;
        }
        let Some(e) = self.queue_edit.take() else {
            return false;
        };
        let (text, images) = (text.to_string(), images.to_vec());
        self.workspace
            .update(cx, |ws, cx| ws.queue_put(&e.thread, e.ix, text, images, cx));
        (self.consult, self.restate) = e.asks;
        self.bring_back_draft(e.draft, window, cx);
        cx.notify();
        true
    }

    /// What the prompt held before a queued message was taken into it to change, while that's
    /// under way: it's kept with the prompt's own as Trek quits, as putting the edit away would
    /// bring it back.
    pub(super) fn held_draft(&self) -> Option<&(String, Vec<PathBuf>)> {
        self.queue_edit.as_ref().map(|e| &e.draft)
    }

    /// The message that went was the one being changed, its turn over: what the prompt held
    /// before comes back.
    pub(super) fn after_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = self.queue_edit.take() {
            self.bring_back_draft(e.draft, window, cx);
        }
    }

    fn bring_back_draft(
        &mut self,
        (text, images): (String, Vec<PathBuf>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.put_back(&text, images, Some(text.len()), window, cx);
    }

    /// The rows, while anything is queued behind `id`'s turn.
    pub(super) fn queue_bar(
        &mut self,
        id: &str,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ws = self.workspace.read(cx);
        let queued: Vec<(String, usize)> = ws
            .live
            .get(id)?
            .queued
            .iter()
            .map(|(text, images)| (first_line(text, images.len()), images.len()))
            .collect();
        let editing = self
            .queue_edit
            .as_ref()
            .filter(|e| e.thread == id && ws.queue_waits(id))
            .map(|e| {
                (
                    e.ix.min(queued.len()),
                    first_line(&e.original.0, e.original.1.len()),
                )
            });
        if queued.is_empty() && editing.is_none() {
            return None;
        }
        let theme = cx.theme().clone();
        let total = queued.len();
        let mut rows: Vec<AnyElement> = queued
            .into_iter()
            .enumerate()
            .map(|(ix, (text, images))| self.queue_row(id, ix, total, text, images, compact, cx))
            .collect();
        if let Some((at, text)) = editing {
            rows.insert(at, self.editing_row(text, cx));
        }
        Some(
            v_flex()
                .id("queue-bar")
                .test_support()
                .mx(px(12.))
                .rounded_t(px(self.bar_corners()))
                .border_1()
                .border_b_0()
                .border_color(theme.foreground.opacity(0.1))
                .bg(theme.foreground.opacity(0.03))
                .text_size(px(text::BODY))
                .children(rows.into_iter().enumerate().map(|(i, row)| {
                    div()
                        .when(i > 0, |el| {
                            el.border_t_1().border_color(theme.foreground.opacity(0.07))
                        })
                        .child(row)
                }))
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_row(
        &self,
        id: &str,
        ix: usize,
        total: usize,
        text: String,
        images: usize,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let moved = |to: usize| {
            let (ws, id) = (self.workspace.clone(), id.to_string());
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                ws.update(cx, |ws, cx| ws.queue_move(&id, ix, to, cx))
            }
        };
        let (edit_id, drop_ws, drop_id) = (id.to_string(), self.workspace.clone(), id.to_string());
        h_flex()
            .id(("queue-row", ix))
            .test_support()
            .group("queue-row")
            .h(px(28.))
            .pl(px(12.))
            .pr(px(4.))
            .gap(px(8.))
            .child(
                Icon::new(crate::assets::Lucide::Clock)
                    .xsmall()
                    .text_color(muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.foreground.opacity(0.85))
                    .child(text),
            )
            .when(images > 0 && !compact, |el| {
                el.child(div().flex_none().text_color(muted).child(format!(
                    "{images} image{}",
                    if images == 1 { "" } else { "s" }
                )))
            })
            // Shown with the pointer on the row; laid out always, so nothing moves as it comes.
            .child(
                h_flex()
                    .flex_none()
                    .invisible()
                    .group_hover("queue-row", |s| s.visible())
                    // A lone message has nowhere to go.
                    .when(total > 1 && ix > 0, |el| {
                        el.child(
                            Button::new(("queue-up", ix))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::ArrowUp).text_color(muted))
                                .tooltip("Move up")
                                .on_click(moved(ix - 1)),
                        )
                    })
                    .when(total > 1 && ix + 1 < total, |el| {
                        el.child(
                            Button::new(("queue-down", ix))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::ArrowDown).text_color(muted))
                                .tooltip("Move down")
                                .on_click(moved(ix + 1)),
                        )
                    })
                    .child(
                        Button::new(("queue-edit", ix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(crate::assets::Lucide::Pencil).text_color(muted))
                            .tooltip("Edit")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_queued(&edit_id, ix, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("queue-remove", ix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Close).text_color(muted))
                            .tooltip("Remove")
                            .on_click(move |_, _, cx| {
                                drop_ws.update(cx, |ws, cx| ws.queue_remove(&drop_id, ix, cx))
                            }),
                    ),
            )
            .into_any_element()
    }

    /// The row of the message that's in the prompt being changed.
    fn editing_row(&self, text: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        h_flex()
            .id("queue-editing")
            .test_support()
            .h(px(28.))
            .pl(px(12.))
            .pr(px(12.))
            .gap(px(8.))
            .text_color(theme.muted_foreground)
            .child(Icon::new(crate::assets::Lucide::Pencil).xsmall())
            .child(div().flex_1().min_w_0().truncate().child(text))
            .child(div().flex_none().child("Editing · Esc to cancel"))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::first_line;

    #[test]
    fn a_row_shows_the_first_line_or_the_images() {
        assert_eq!(first_line("then\nthe docs", 0), "then");
        assert_eq!(first_line("\n  then the docs", 1), "then the docs");
        assert_eq!(first_line("", 1), "1 image");
        assert_eq!(first_line("  ", 2), "2 images");
        assert_eq!(
            first_line(&trek_core::restate::with_restate("then the docs"), 0),
            "then the docs"
        );
    }

    #[test]
    fn a_row_of_files_alone_names_them() {
        let dir = std::env::temp_dir().join(format!("trek-queue-row-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (clip, pdf) = (dir.join("clip.mov"), dir.join("Q3 report.pdf"));
        for f in [&clip, &pdf] {
            std::fs::write(f, b"x").unwrap();
        }
        let at = |p: &std::path::Path| trek_core::media::mention_of(&p.display().to_string());
        // The files mentioned after the words aren't the first line.
        assert_eq!(
            first_line(&format!("look at this {} ", at(&clip)), 0),
            "look at this"
        );
        assert_eq!(first_line(&format!("{} ", at(&clip)), 0), "clip.mov");
        assert_eq!(
            first_line(&format!("{} {}", at(&clip), at(&pdf)), 1),
            "1 image, 2 files"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
