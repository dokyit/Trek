//! A message to a thread on another machine sends its attachments first (`machines::files`):
//! pressing send starts them on their way and leaves the draft as it is; each one's tile fills as
//! it goes, a line under the strip says how far they all are with a Cancel, and once the last is
//! in the message goes as any message does, naming the copies there. A failure (or Cancel, or
//! moving to another thread, or taking a file off) stops them: what's typed and attached stays,
//! and a toast says why.

use super::Composer;
use crate::attachments::Attaching as _;
use crate::machines::files::{Outgoing, Plan, Sent};
use crate::scale::text;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use trek_core::settings::FollowUp;
use trek_remote as tr;
use trek_remote::client::ClientError;

/// Attachments on their way to another machine, ahead of their message.
pub(super) struct MachineUpload {
    host: String,
    machine: String,
    /// Where the message was sent from (`draft_key`).
    pub(super) target: String,
    follow: Option<FollowUp>,
    meters: Vec<Meter>,
    _send: Task<()>,
    _tick: Task<()>,
}

struct Meter {
    path: PathBuf,
    /// Its size: a folder's is its zip's, known once it's packed (0 until then).
    total: Arc<AtomicU64>,
    sent: Arc<AtomicU64>,
}

impl MachineUpload {
    /// How far `path` is, 0 to 1 (`None`: it isn't one of these).
    pub(super) fn progress_of(&self, path: &Path) -> Option<f32> {
        self.meters.iter().find(|m| m.path == path).map(|m| {
            let total = m.total.load(Ordering::Relaxed);
            if total == 0 {
                // A folder still being packed has sent nothing; an empty file is all there.
                if m.path.is_dir() { 0. } else { 1. }
            } else {
                (m.sent.load(Ordering::Relaxed) as f64 / total as f64).min(1.) as f32
            }
        })
    }

    /// Every file's progress, for the strip.
    pub(super) fn progress(&self) -> Vec<(PathBuf, f32)> {
        self.meters
            .iter()
            .filter_map(|m| Some((m.path.clone(), self.progress_of(&m.path)?)))
            .collect()
    }

    /// "Sending 2 files to T-Book · 1.2 MB of 2.6 MB".
    pub(super) fn words(&self) -> String {
        let total: u64 = self
            .meters
            .iter()
            .map(|m| m.total.load(Ordering::Relaxed))
            .sum();
        let sent: u64 = self
            .meters
            .iter()
            .map(|m| {
                m.sent
                    .load(Ordering::Relaxed)
                    .min(m.total.load(Ordering::Relaxed))
            })
            .sum();
        // Nothing sized yet: a folder is being packed.
        if total == 0 && self.meters.iter().any(|m| m.path.is_dir()) {
            return format!("Packing a folder to send to {}…", self.machine);
        }
        let what = match self.meters.len() {
            1 => "1 file".to_string(),
            n => format!("{n} files"),
        };
        format!(
            "Sending {what} to {} · {} of {}",
            self.machine,
            crate::attachments::size_label(sent),
            crate::attachments::size_label(total)
        )
    }
}

/// What a toast says when `name` didn't get to `machine`.
pub fn failed_words(name: &str, machine: &str, e: &ClientError) -> String {
    match e {
        ClientError::TooLarge(_) => format!(
            "{name} is too big to send to {machine}: files of up to {} can go.",
            tr::upload_size_words(tr::MAX_UPLOAD)
        ),
        ClientError::Server { message, .. } => {
            format!("Couldn't send {name} to {machine}: {message}")
        }
        ClientError::Disconnected => format!(
            "Couldn't send {name} to {machine}: the connection dropped. Send again when it's back."
        ),
        ClientError::Invalid(why) => format!("Couldn't send {name} to {machine}: {why}"),
        other => format!("Couldn't send {name} to {machine}: {other}"),
    }
}

impl Composer {
    /// Before a message to another machine goes, its attachments are sent there: true while the
    /// message waits for them (they're on their way, or they can't go and a toast says why).
    pub(super) fn machine_files_first(
        &mut self,
        typed: &str,
        follow: Option<FollowUp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let ws = self.workspace.read(cx);
        let Some(host) = ws.machine_host_for_send(&self.scope) else {
            return false;
        };
        // Already on their way: send again waits with them.
        if self.machine_upload.is_some() {
            return true;
        }
        let items = crate::machines::files::outgoing(typed, &self.outbox.paths, &self.outbox.files);
        if items.is_empty() {
            return false;
        }
        let items = match ws.machine_files_plan(&self.scope, &host, &items) {
            Plan::Ready | Plan::Inline => return false,
            Plan::Refuse(why) => {
                self.toast(why, cx);
                return true;
            }
            Plan::Upload(items) => items,
        };
        let Some((client, machine)) = ws.machine_client(&host) else {
            return false;
        };
        self.send_to_machine(host, machine, client, items, follow, window, cx);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn send_to_machine(
        &mut self,
        host: String,
        machine: String,
        client: Arc<trek_remote::client::Client>,
        items: Vec<Outgoing>,
        follow: Option<FollowUp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let meters: Vec<Meter> = items
            .iter()
            .map(|o| Meter {
                path: o.path.clone(),
                total: Arc::new(AtomicU64::new(
                    std::fs::metadata(&o.path)
                        .ok()
                        .filter(|m| !m.is_dir())
                        .map_or(0, |m| m.len()),
                )),
                sent: Arc::default(),
            })
            .collect();
        let jobs: Vec<(Outgoing, Arc<AtomicU64>, Arc<AtomicU64>)> = items
            .into_iter()
            .zip(meters.iter().map(|m| (m.sent.clone(), m.total.clone())))
            .map(|(o, (sent, total))| (o, sent, total))
            .collect();
        let name = machine.clone();
        let send = cx.spawn_in(window, async move |this, cx| {
            // Named as this computer is among the machine's devices.
            let from = cx
                .background_executor()
                .spawn(async { crate::machines::this_name() })
                .await;
            let mut sent: Vec<(PathBuf, Sent)> = Vec::new();
            for (item, meter, total) in jobs {
                let label = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| item.path.display().to_string());
                // A folder goes as one zip of its files, packed off the main thread and let go
                // of once it's sent.
                let mut packed: Option<PathBuf> = None;
                if item.path.is_dir() {
                    let dir = item.path.clone();
                    let into = std::env::temp_dir().join(format!(
                        "trek-folder-{}-{}",
                        std::process::id(),
                        trek_core::store::now_ms()
                    ));
                    let made = cx
                        .background_executor()
                        .spawn(async move { crate::machines::files::pack_folder(&dir, &into) })
                        .await;
                    match made {
                        Ok(made) => {
                            total.store(
                                std::fs::metadata(&made.zip).map_or(0, |m| m.len()),
                                Ordering::Relaxed,
                            );
                            // What couldn't be read isn't in the zip: said, and the rest goes.
                            if made.skipped > 0 {
                                let words =
                                    crate::machines::files::skipped_words(made.skipped, &label);
                                let _ = this.update(cx, |this, cx| this.toast(words, cx));
                            }
                            packed = Some(made.zip);
                        }
                        Err(e) => {
                            let _ = this.update(cx, |this, cx| {
                                this.machine_upload = None;
                                this.toast(e.words(&label, &name), cx);
                                cx.notify();
                            });
                            return;
                        }
                    }
                }
                let (client, from, image) = (client.clone(), from.clone(), item.image);
                let path = packed.clone().unwrap_or_else(|| item.path.clone());
                // Read and sent off the main thread, a piece at a time.
                let done = cx
                    .background_executor()
                    .spawn(async move {
                        client
                            .upload_file(&path, &from, image, |n, _| {
                                meter.store(n, Ordering::Relaxed)
                            })
                            .await
                    })
                    .await;
                if let Some(zip) = packed.as_ref().and_then(|z| z.parent()) {
                    let _ = std::fs::remove_dir_all(zip);
                }
                match done {
                    Ok(up) => sent.push((
                        item.path.clone(),
                        Sent {
                            upload_id: up.upload_id,
                            there: up.path,
                            image: item.image,
                        },
                    )),
                    Err(e) => {
                        tracing::info!(machine = name, file = label, "machine upload failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.machine_upload = None;
                            this.toast(failed_words(&label, &name, &e), cx);
                            cx.notify();
                        });
                        return;
                    }
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(up) = this.machine_upload.take() else {
                    return;
                };
                let host = up.host.clone();
                this.workspace
                    .update(cx, |ws, cx| ws.machine_took_files(&host, sent, cx));
                // Still where it was sent from: it goes now, as it was when it's sent.
                if up.target == this.target(cx) {
                    let input = this.input.clone();
                    this.submit_as(input, up.follow, window, cx);
                }
                cx.notify();
            });
        });
        // The tiles fill as the pieces go.
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(120))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        self.machine_upload = Some(MachineUpload {
            host,
            machine,
            target: self.target(cx),
            follow,
            meters,
            _send: send,
            _tick: tick,
        });
        cx.notify();
    }

    /// Stop sending the attachments (Cancel, a file taken off, another thread opened): the draft
    /// stays as it is.
    pub(super) fn cancel_machine_upload(&mut self, cx: &mut Context<Self>) {
        if self.machine_upload.take().is_some() {
            cx.notify();
        }
    }

    /// The line under the strip while attachments are on their way: how far, and Cancel.
    pub(super) fn machine_upload_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let up = self.machine_upload.as_ref()?;
        let theme = cx.theme().clone();
        let me = cx.entity().downgrade();
        Some(
            h_flex()
                .id("machine-upload")
                .test_support()
                .pt(px(6.))
                .gap_2()
                .text_size(px(text::LABEL))
                .text_color(theme.muted_foreground)
                .child(div().child(up.words()))
                .child(
                    div()
                        .id("machine-upload-cancel")
                        .test_support()
                        .cursor_pointer()
                        .text_color(theme.foreground)
                        .hover(|s| s.underline())
                        .child("Cancel")
                        .on_click(move |_, _, cx| {
                            let _ = me.update(cx, |this, cx| this.cancel_machine_upload(cx));
                        }),
                )
                .into_any_element(),
        )
    }
}
