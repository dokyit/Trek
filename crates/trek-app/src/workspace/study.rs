//! Study sessions (`trek_core::study`): a thread that follows the study guide Trek ships, in the
//! learner's study folder, started from the Study screen's composer (or a map's buttons).

use super::{Route, Scope, Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::path::PathBuf;
use trek_core::store::now_ms;
use trek_core::study::{self, Kind};
use trek_core::{AgentId, skills};

/// What the Study screen's composer starts: the kind of session, and which files already in
/// `sources/` go with it. Kept here so the composer's send sees the screen's choices.
#[derive(Debug, Clone, Default)]
pub struct StudyDraft {
    pub kind: Kind,
    /// Files in `sources/` ticked for the next session.
    pub picked: Vec<PathBuf>,
}

impl Workspace {
    /// `scope` is the Study screen's composer: the main window's, on Study.
    pub fn is_study_in(&self, scope: &Scope) -> bool {
        *scope == Scope::Main && self.route == Route::Study
    }

    /// The thread in `scope` is a study session: it works in the study folder. Its composer is
    /// for thinking out loud, not for a project's branch, changes or plan mode.
    pub fn is_study_session_in(&self, scope: &Scope) -> bool {
        self.thread_in(scope)
            .and_then(|t| t.cwd.as_deref())
            .is_some_and(|cwd| cwd == study::dir())
    }

    /// Learn or Practice, for the next session the Study screen's composer starts.
    pub fn set_study_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.study.kind != kind {
            self.study.kind = kind;
            cx.notify();
        }
    }

    /// Tick or untick a file in `sources/` for the next session.
    pub fn toggle_study_source(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match self.study.picked.iter().position(|p| *p == path) {
            Some(i) => {
                self.study.picked.remove(i);
            }
            None => self.study.picked.push(path),
        }
        cx.notify();
    }

    /// Start a study session of `kind` on `topic` (in the learner's words) with the agent and model
    /// the composer has, and open it. `map`: the topic's concept map, when it carries on with one.
    pub fn start_study(
        &mut self,
        kind: Kind,
        topic: String,
        map: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.start_study_with(kind, topic, map, vec![], vec![], cx)
    }

    /// `start_study` from the composer: `text` is what the learner typed (or dictated), files in
    /// it named as `@path` and the `images` attached are copied into the study folder's
    /// `sources/<topic>/` and listed in the first message with the `picked` ones already there, so
    /// the tutor reads them. The images stay attached too. It works in the study folder, outside
    /// every project, so its maps are the learner's and not a project's.
    pub fn start_study_with(
        &mut self,
        kind: Kind,
        text: String,
        map: Option<PathBuf>,
        images: Vec<PathBuf>,
        picked: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() && picked.is_empty() {
            return;
        }
        let dir = study::dir();
        // What was typed goes back to the composer when the session can't start.
        let back = |cx: &mut Context<Self>, text: &str, images: &[PathBuf], message: String| {
            cx.emit(WorkspaceEvent::Toast {
                message,
                undo: None,
            });
            cx.emit(WorkspaceEvent::InsertIntoComposer(text.to_string()));
            for image in images {
                cx.emit(WorkspaceEvent::AttachImage(image.clone()));
            }
        };
        let guide = study::ensure(&dir)
            .map_err(anyhow::Error::from)
            .and_then(|_| skills::shipped(skills::STUDY));
        let guide = match guide {
            Ok(g) => g,
            Err(e) => {
                return back(
                    cx,
                    &text,
                    &images,
                    format!("Couldn't set up the study folder: {e}"),
                );
            }
        };
        // The topic, for the title and the sources' folder: the text without the files, folders
        // and pages it names.
        let mentioned = trek_core::media::mentioned_files(&text, Some(&dir));
        let folders = trek_core::media::mentioned_dirs(&text, Some(&dir));
        let links = study::links_in(&text);
        let mut attached: Vec<PathBuf> = mentioned;
        attached.extend(images.iter().cloned());
        attached.extend(picked);
        let topic = {
            let words = study::words_of(&text);
            if words.is_empty() {
                attached
                    .iter()
                    .chain(&folders)
                    .filter_map(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .chain(links.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                words
            }
        };
        let with = study::Material {
            files: study::add_sources(&dir, &topic, &attached),
            folders,
            links,
        };
        let asked = if text.is_empty() {
            topic.clone()
        } else {
            text.clone()
        };
        let first = study::prompt(kind, &guide, &dir, &asked, map.as_deref(), &with);
        let title = study::title(kind, &topic);
        let prefs = self.study_prefs();
        let folder = dir.clone();
        let made = self.make_thread(
            None,
            prefs,
            &first,
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
            Err(e) => return back(cx, &text, &images, e.to_string()),
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
        self.send_to(&id, first, images, cx);
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
