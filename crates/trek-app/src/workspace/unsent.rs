//! Messages not sent yet, kept across a restart. A composer's text lives in the composer, which
//! goes with the process, so what's typed is noted here as it changes (and what was noted is
//! saved now and then, for a crash, and as Trek quits). The next launch gives each composer back
//! what was its own, the first time it shows that thread or draft. Messages queued behind a turn
//! when Trek quit come back the same way, unsent: their turn is gone.

use super::switching::PendingSwitch;
use super::{Scope, Workspace};
use gpui_kit::App;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use trek_core::session_secrets;
use trek_core::store::Unsent;
use trek_core::{AgentId, Effort};

/// How long after the last keystroke someone is taken to be typing still: an update's restart
/// waits for it (`updater::wait::Moment::typing`).
pub const TYPING_QUIET: Duration = Duration::from_secs(30);

/// A move to another agent waiting for a thread's next message is kept beside the messages, by
/// this key and the thread's id: it's the user's pick, as much as what they typed.
const SWITCH_KEY: &str = "switch:";

/// What's known of the messages in progress.
#[derive(Default)]
pub struct UnsentBook {
    /// By `key`: the scope that typed it (`None`: it came from the last run).
    noted: HashMap<String, (Option<Scope>, Unsent)>,
    /// From the last run, until a composer asks for them.
    restored: HashMap<String, Unsent>,
    /// Noted since it was last saved.
    dirty: bool,
    /// When the user last changed a message they haven't sent.
    typed: Option<Instant>,
    /// Another Trek has the data folder: what's kept there is its own.
    borrowed: bool,
    /// The values agents were given in secret this run, taken out of whatever's kept. Its own
    /// copy: as Trek quits the secrets' files go (and `known` with them) before the last save.
    secrets: Arc<Vec<(String, String)>>,
}

/// A switch as it's kept: the agent's key, model, effort and account.
fn switch_text(p: &PendingSwitch) -> String {
    serde_json::json!({
        "agent": p.agent.key(),
        "model": p.model,
        "effort": p.effort.as_str(),
        "account": p.account,
    })
    .to_string()
}

fn switch_from(text: &str) -> Option<PendingSwitch> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let text = |key: &str| v.get(key).and_then(|s| s.as_str()).map(str::to_string);
    Some(PendingSwitch {
        agent: AgentId::from_key(&text("agent")?),
        model: text("model"),
        effort: Effort::parse(&text("effort")?)?,
        account: text("account"),
    })
}

/// The key a composer's message goes by: a thread's is the thread's wherever it's typed (a thread
/// window, the main window); a new thread's is the composer's own.
fn key_of(scope: &Scope, draft_key: &str) -> Option<String> {
    if draft_key.is_empty() {
        return None;
    }
    Some(match scope {
        _ if draft_key.starts_with("thread:") => draft_key.to_string(),
        Scope::Main | Scope::Thread(_) | Scope::Beside => draft_key.to_string(),
        Scope::Ide => format!("ide|{draft_key}"),
        Scope::Side => format!("side|{draft_key}"),
    })
}

impl Workspace {
    /// At launch: what the last run kept.
    pub(super) fn load_unsent(&mut self, alone: bool) {
        self.unsent.borrowed = !alone;
        if !alone {
            return;
        }
        let kept = self.store.unsent_messages().unwrap_or_else(|e| {
            tracing::warn!("read unsent messages: {e:#}");
            vec![]
        });
        for m in kept {
            // A switch the user picked waits again for the thread's next message, while it's
            // still a move (the thread may have been moved some other way since) to an agent
            // that's still on.
            if let Some(id) = m.key.strip_prefix(SWITCH_KEY) {
                let moves = |p: &PendingSwitch| {
                    !self.settings.disabled_agents.contains(&p.agent.key())
                        && self
                            .thread(id)
                            .is_some_and(|t| t.agent != p.agent || t.account != p.account)
                };
                if let Some(p) = switch_from(&m.text).filter(moves) {
                    self.live.entry(id.to_string()).or_default().pending_switch = Some(p);
                }
                continue;
            }
            self.unsent.restored.insert(m.key.clone(), m.clone());
            self.unsent.noted.insert(m.key.clone(), (None, m));
        }
    }

    /// A switch was picked, or dropped: kept at the next save.
    pub(super) fn switch_changed(&mut self) {
        self.unsent.dirty = true;
    }

    /// `secrets_to_scrub`, now: Trek is quitting, and the secrets' files are about to go.
    pub(super) fn keep_secrets_for_quit(&mut self) {
        self.secrets_to_scrub();
    }

    /// The values agents were given in secret this run, as last known.
    fn secrets_to_scrub(&mut self) -> Arc<Vec<(String, String)>> {
        let known = session_secrets::known();
        if known.len() > self.unsent.secrets.len() {
            self.unsent.secrets = known;
        }
        self.unsent.secrets.clone()
    }

    /// A composer's message changed (`draft_key`, the composer's key for what it shows).
    pub fn note_unsent(
        &mut self,
        scope: &Scope,
        draft_key: &str,
        text: &str,
        images: Vec<PathBuf>,
        now: Instant,
    ) {
        self.unsent.typed = (!text.trim().is_empty()).then_some(now);
        let Some(key) = key_of(scope, draft_key) else {
            return;
        };
        // While a thread waits on a secret's card, what's typed for it may be the secret, pasted
        // in the wrong place (or put back by the composer's guard): none of it is kept. What was
        // kept before the card came up stays.
        if key
            .strip_prefix("thread:")
            .is_some_and(|id| self.asks_for_secret(id))
        {
            return;
        }
        let secrets = self.secrets_to_scrub();
        let text = session_secrets::scrub(text, &secrets);
        let text = text.as_ref();
        let book = &mut self.unsent;
        if text.is_empty() && images.is_empty() {
            // Emptied: the message is gone, unless this composer never had it and another does (a
            // window opening on a thread whose message is in the main window's keeping).
            let theirs = book
                .noted
                .get(&key)
                .is_some_and(|(by, _)| by.as_ref().is_some_and(|by| by != scope));
            if !theirs && book.noted.remove(&key).is_some() {
                book.dirty = true;
            }
            return;
        }
        let same = book
            .noted
            .get(&key)
            .is_some_and(|(_, m)| m.text == text && m.images == images);
        if !same {
            book.dirty = true;
        }
        book.noted.insert(
            key.clone(),
            (
                Some(scope.clone()),
                Unsent {
                    key,
                    text: text.to_string(),
                    images,
                },
            ),
        );
    }

    /// What the last run kept for `draft_key`, once. Attachments that have gone are left out.
    pub fn take_restored_message(
        &mut self,
        scope: &Scope,
        draft_key: &str,
    ) -> Option<(String, Vec<PathBuf>)> {
        let key = key_of(scope, draft_key)?;
        let m = self.unsent.restored.remove(&key)?;
        let images = m.images.into_iter().filter(|p| p.exists()).collect();
        Some((m.text, images))
    }

    /// Someone is in the middle of typing a message.
    pub fn typing_now(&self, cx: &App) -> bool {
        self.unsent
            .typed
            .is_some_and(|at| cx.background_executor().now().duration_since(at) < TYPING_QUIET)
    }

    /// Keep what's noted, with what's queued behind turns: those go with the process too. A
    /// message and what was queued behind it become one, as a turn that fails hands them back.
    pub fn save_unsent(&mut self) {
        if self.unsent.borrowed {
            return;
        }
        let secrets = self.secrets_to_scrub();
        let mut all: HashMap<String, Unsent> = self
            .unsent
            .noted
            .values()
            .map(|(_, m)| (m.key.clone(), m.clone()))
            .collect();
        for (id, live) in self
            .live
            .iter()
            .filter(|(id, _)| !crate::machines::is_remote(id))
        {
            for (text, images) in live.queued.iter().chain(&live.left_over) {
                let key = format!("thread:{id}");
                let m = all.entry(key.clone()).or_insert_with(|| Unsent {
                    key,
                    text: String::new(),
                    images: vec![],
                });
                if !text.is_empty() {
                    if !m.text.is_empty() {
                        m.text.push_str("\n\n");
                    }
                    m.text.push_str(text);
                }
                m.images.extend(images.iter().cloned());
            }
            if let Some(p) = &live.pending_switch {
                let key = format!("{SWITCH_KEY}{id}");
                all.insert(
                    key.clone(),
                    Unsent {
                        key,
                        text: switch_text(p),
                        images: vec![],
                    },
                );
            }
        }
        // Nothing an agent was given in secret is kept, whatever it came in with.
        for m in all.values_mut() {
            session_secrets::scrub_string(&mut m.text, &secrets);
        }
        if let Err(e) = self.store.save_unsent_messages(all.values()) {
            tracing::warn!("save unsent messages: {e:#}");
        }
        self.unsent.dirty = false;
    }

    /// Housekeeping: what changed since the last save, in case Trek doesn't get to quit.
    pub(super) fn save_unsent_if_changed(&mut self) {
        if self.unsent.dirty {
            self.save_unsent();
        }
    }

    /// Note an editor, to know later whether it has unsaved edits (`unsaved_files`).
    pub fn note_editor(&mut self, editor: gpui_kit::WeakEntity<crate::editor::EditorView>) {
        self.editors.retain(|e| e.upgrade().is_some());
        self.editors.push(editor);
    }

    /// Files open in the editor with edits not saved. A scratch buffer has nowhere to go.
    pub fn unsaved_files(&self, cx: &App) -> usize {
        self.editors
            .iter()
            .filter_map(|e| e.upgrade())
            .filter(|e| {
                let e = e.read(cx);
                e.dirty && !e.scratch
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_threads_message_goes_by_the_thread_a_new_threads_by_its_composer() {
        assert_eq!(
            key_of(&Scope::Main, "thread:a").as_deref(),
            Some("thread:a")
        );
        assert_eq!(
            key_of(&Scope::Thread("a".into()), "thread:a").as_deref(),
            Some("thread:a"),
            "the same message wherever the thread is shown"
        );
        assert_eq!(key_of(&Scope::Ide, "thread:a").as_deref(), Some("thread:a"));
        assert_eq!(
            key_of(&Scope::Main, "draft:/p").as_deref(),
            Some("draft:/p")
        );
        assert_eq!(
            key_of(&Scope::Ide, "draft:/p").as_deref(),
            Some("ide|draft:/p"),
            "the editor's new chat isn't the main window's"
        );
        assert_eq!(
            key_of(&Scope::Side, "draft:/p").as_deref(),
            Some("side|draft:/p")
        );
        assert_eq!(key_of(&Scope::Main, ""), None);
    }
}
