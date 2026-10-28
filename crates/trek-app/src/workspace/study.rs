//! Study sessions (`trek_core::study`): a thread that follows the study guide Trek ships, in the
//! learner's study folder, started from the Study screen.

use super::{Route, Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::path::PathBuf;
use trek_core::store::now_ms;
use trek_core::study::{self, Kind};
use trek_core::{AgentId, skills};

impl Workspace {
    /// Start a study session of `kind` on `topic` (in the learner's words) with the agent and model
    /// the composer has, and open it. `map`: the topic's concept map, when it carries on with one.
    /// It works in the study folder, outside every project, so its maps are the learner's and not a
    /// project's.
    pub fn start_study(
        &mut self,
        kind: Kind,
        topic: String,
        map: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let topic = topic.trim().to_string();
        if topic.is_empty() {
            return;
        }
        let dir = study::dir();
        let guide = study::ensure(&dir)
            .map_err(anyhow::Error::from)
            .and_then(|_| skills::shipped(skills::STUDY));
        let guide = match guide {
            Ok(g) => g,
            Err(e) => {
                cx.emit(WorkspaceEvent::Toast {
                    message: format!("Couldn't set up the study folder: {e}"),
                    undo: None,
                });
                return;
            }
        };
        let text = study::prompt(kind, &guide, &dir, &topic, map.as_deref());
        let title = study::title(kind, &topic);
        let prefs = self.study_prefs();
        let folder = dir.clone();
        let made = self.make_thread(
            None,
            prefs,
            &text,
            false,
            move |t| {
                // A thread in no project is given a chat folder of its own: this one works in the
                // study folder instead, so the empty one goes.
                if let Some(chat) = t.cwd.take().filter(|c| {
                    trek_core::paths::is_chat_dir(c) && *c != trek_core::paths::chats_dir()
                }) {
                    let _ = std::fs::remove_dir_all(chat);
                }
                t.cwd = Some(folder);
                t.title = title;
            },
            cx,
        );
        let id = match made {
            Ok(id) => id,
            Err(e) => {
                cx.emit(WorkspaceEvent::Toast {
                    message: e.to_string(),
                    undo: None,
                });
                return;
            }
        };
        let _ = study::record(
            &dir,
            study::Session {
                thread: id.clone(),
                kind,
                topic,
                started_at: now_ms(),
            },
        );
        self.navigate(Route::Thread(id.clone()), cx);
        self.send_to(&id, text, vec![], cx);
    }

    /// The draft's agent and model, if it can work with files (the session reads the learner's
    /// sources and keeps their maps); else the user's default agent, or the first that can. Never
    /// in a worktree or plan mode.
    fn study_prefs(&self) -> super::Prefs {
        let works =
            |a: &AgentId| !matches!(a, AgentId::Direct(p) if !trek_core::catalog::is_mock(p));
        let ready = self.ready_agents();
        let mut p = self.draft_prefs.clone();
        if !(ready.contains(&p.agent) && works(&p.agent)) {
            let default = AgentId::from_key(&self.settings.general.default_agent);
            if ready.contains(&default) && works(&default) {
                p.agent = default;
                p.model = self.settings.general.default_model.clone();
            } else if let Some(a) = ready.into_iter().find(works) {
                p.agent = a;
                p.model = None;
            }
        }
        p.worktree = false;
        p.plan = false;
        p
    }
}
