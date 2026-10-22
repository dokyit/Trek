//! What a thread's agent runs in the background (dev servers, watchers), for the phone as the
//! Mac's strip above the composer shows it: each task in the thread's row with its last line,
//! the end of its output when asked (read from the agent then), and Stop.

use crate::workspace::Workspace;
use gpui_kit::Context;
use std::time::Duration;
use trek_core::store::Thread;
use trek_remote as tr;

/// The most of a task's output the phone gets: its last lines, and no more than this.
const OUTPUT_LINES: usize = 400;
const MAX_OUTPUT: usize = 64 << 10;
/// How long a phone asking for output waits for the agent's fresh read.
const READ_WAIT: Duration = Duration::from_millis(1500);

/// The end of `output`, as the Mac's popover shows it.
fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let text = lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n");
    if text.len() <= MAX_OUTPUT {
        return text;
    }
    let mut start = text.len() - MAX_OUTPUT;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

impl Workspace {
    /// What `t`'s agent runs in the background, as the strip lists it.
    pub(super) fn remote_background(&self, t: &Thread) -> Vec<tr::BackgroundTask> {
        let Some(l) = self.live.get(&t.id) else {
            return vec![];
        };
        l.background_work()
            .map(|b| tr::BackgroundTask {
                id: b.task.id.clone(),
                kind: match b.task.kind {
                    trek_agents::BackgroundKind::Shell => tr::BackgroundKind::Shell,
                    trek_agents::BackgroundKind::Monitor => tr::BackgroundKind::Monitor,
                    _ => tr::BackgroundKind::Other,
                },
                title: super::clip_text_to(&b.task.title, 4 << 10),
                line: b.last_line().map(|l| l.chars().take(300).collect()),
                since: Some(super::instant_ms(b.started)),
                readable: b.task.readable,
                stoppable: b.task.stoppable,
                stopping: b.stopping,
            })
            .collect()
    }

    /// The end of a task's output: read from the agent now, and sent once it's in (or what was
    /// read before, if the agent takes a while).
    pub(super) fn remote_background_output(
        &mut self,
        thread_id: String,
        task_id: String,
        reply: tr::Reply<tr::BackgroundOutput>,
        cx: &mut Context<Self>,
    ) {
        let found = self
            .live
            .get(&thread_id)
            .and_then(|l| l.background_work().find(|b| b.task.id == task_id))
            .map(|b| (b.task.readable, b.output_rev));
        let Some((readable, rev)) = found else {
            let _ = reply.send(Err(tr::HostError::not_found(
                "That task isn't running any more",
            )));
            return;
        };
        if readable {
            self.read_background(&thread_id, &task_id);
        }
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let output = loop {
                let now = this.read_with(cx, |ws, _| {
                    ws.live
                        .get(&thread_id)
                        .and_then(|l| l.background.iter().find(|b| b.task.id == task_id))
                        .map(|b| (b.output_rev, b.output.as_deref().map(tail)))
                });
                match now {
                    Ok(Some((r, output)))
                        if r != rev || !readable || started.elapsed() >= READ_WAIT =>
                    {
                        break Ok(output);
                    }
                    Ok(Some(_)) => {
                        cx.background_executor()
                            .timer(Duration::from_millis(50))
                            .await
                    }
                    Ok(None) => {
                        break Err(tr::HostError::not_found("That task isn't running any more"));
                    }
                    Err(_) => break Err(tr::HostError::other("Trek is closing")),
                }
            };
            let _ = reply.send(output.map(|output| tr::BackgroundOutput {
                thread_id,
                task_id,
                output,
            }));
        })
        .detach();
    }

    /// Stop, as the strip's button does.
    pub(super) fn remote_stop_background(
        &mut self,
        thread_id: &str,
        task_id: &str,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<()> {
        let Some(live) = self.live.get(thread_id) else {
            return Err(tr::HostError::not_found("No such thread"));
        };
        let Some(b) = live.background_work().find(|b| b.task.id == task_id) else {
            return Err(tr::HostError::not_found("That task isn't running any more"));
        };
        if !b.task.stoppable {
            let agent = self
                .thread(thread_id)
                .map(|t| t.agent.display_name())
                .unwrap_or_default();
            return Err(tr::HostError::bad_request(format!(
                "{agent} can't stop this one from Trek"
            )));
        }
        self.stop_background(thread_id, task_id, cx);
        Ok(())
    }
}
