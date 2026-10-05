//! Slash commands from the phone: the list its `/` picker shows, and Trek's own commands sent
//! from it, run as the Mac's composer runs them. Full access stays behind the Mac's unlock
//! (`set_hand_holding` checks it, however it's asked for); `/new` opens the phone's own sheet
//! rather than moving the Mac's window.

use super::is_trek_command;
use crate::composer::{consult_split, consultant, default_model, find_model, model_name, restate_command, same_model};
use crate::workspace::{BUILTIN_COMMANDS, Scope, Workspace};
use trek_core::orchestrate::{Consult, Style};
use trek_core::store::Thread;
use trek_core::{AgentId, Effort};
use trek_remote as tr;

/// What a message from the phone comes to.
pub(super) enum Phone {
    /// Send this (as typed, or as the composer would have rewritten it).
    Send(String),
    /// Nothing to send: the phone opens this.
    Open(tr::Open),
}

impl Workspace {
    /// `text` sent to thread `t` from the phone: Trek's own commands as the composer runs them.
    pub(super) fn phone_command(&self, t: &Thread, text: &str) -> tr::HostResult<Phone> {
        let Some(cmd) = text.trim_start().strip_prefix('/').and_then(|c| c.split_whitespace().next()) else { return Ok(Phone::Send(text.to_string())) };
        match cmd.to_lowercase().as_str() {
            // The Mac would open a new thread in its window: the phone opens its sheet instead,
            // in the project the Mac would have picked.
            "new" | "clear" => {
                let folder = self.draft_folder(t);
                let project_id = folder.and_then(|f| self.projects.iter().find(|p| p.path == f)).map(|p| p.id.clone()).or_else(|| t.project_id.clone());
                Ok(Phone::Open(tr::Open::NewThread { project_id }))
            }
            "consult" | "restate" => self.rewrite(&t.agent, t.model.as_deref(), text, true).map(Phone::Send),
            // The rest (`/permissions`, `/usage`, `/context`, `/cost`, `/model`) are answered in
            // the thread by `send_to`, as typed on the Mac; anything else is the agent's.
            _ => Ok(Phone::Send(text.to_string())),
        }
    }

    /// The first message of a thread started from the phone. A thread can start with `/consult`
    /// or `/restate`; Trek's other commands are about a thread that's under way.
    pub(super) fn phone_first_message(&self, agent: &AgentId, model: Option<&str>, text: &str) -> tr::HostResult<String> {
        if !is_trek_command(text) {
            return Ok(text.to_string());
        }
        let cmd = text.trim_start().trim_start_matches('/').split_whitespace().next().unwrap_or_default().to_lowercase();
        match cmd.as_str() {
            "consult" | "restate" => self.rewrite(agent, model, text, false),
            _ => Err(tr::HostError::bad_request(format!("/{cmd} is for a thread that's under way: start the thread first, then send it there."))),
        }
    }

    /// `/consult …: message` and `/restate …` as the composer turns them into the message for the
    /// agent. `under_way`: in a thread (where `/restate` alone restates the thread so far).
    fn rewrite(&self, agent: &AgentId, model: Option<&str>, text: &str, under_way: bool) -> tr::HostResult<String> {
        if let Some(restate) = restate_command(text) {
            return match restate {
                Some(message) => Ok(trek_core::restate::with_restate(&message)),
                None if under_way => Ok(trek_core::restate::with_restate(trek_core::restate::THREAD)),
                None => Err(tr::HostError::bad_request("Say what to restate: /restate your message")),
            };
        }
        let rest = text.trim_start().strip_prefix("/consult").unwrap_or_default();
        if let Some(why) = self.consult_unavailable(agent) {
            return Err(tr::HostError::bad_request(format!("Can't consult: {why}.")));
        }
        let (targets, message) = match consult_split(rest) {
            Some((t, m)) => (t, m.trim()),
            None => (rest, ""),
        };
        if message.is_empty() {
            return Err(tr::HostError::bad_request("Say what to ask after a colon: /consult sol high: your question"));
        }
        let mut consult = Consult { implement: true, ..Default::default() };
        for part in targets.split(',') {
            let mut words: Vec<&str> = part.split_whitespace().collect();
            words.retain(|w| match w.to_lowercase().as_str() {
                "discuss" => {
                    consult.style = Style::Discuss;
                    false
                }
                "advise" => {
                    consult.style = Style::Advise;
                    false
                }
                "arena" => {
                    consult.style = Style::Arena;
                    false
                }
                "report" => {
                    consult.implement = false;
                    false
                }
                "implement" => {
                    consult.implement = true;
                    false
                }
                _ => true,
            });
            if words.is_empty() {
                continue;
            }
            let effort = words.last().and_then(|w| Effort::parse(w)).filter(|_| words.len() > 1);
            if effort.is_some() {
                words.pop();
            }
            let query = words.join(" ");
            let Some((agent, model)) = find_model(self, &query) else { return Err(tr::HostError::bad_request(format!("There's no model called “{query}” to consult."))) };
            consult.consultants.push(consultant(agent, &model, effort));
        }
        let options = crate::composer::arena_options(self);
        if consult.style == Style::Arena {
            let most = trek_core::orchestrate::MAX_RUNNING;
            if consult.consultants.len() > most {
                return Err(tr::HostError::bad_request(format!("An arena drafts {most} designs at most, all at once: name up to {most} models.")));
            }
            if consult.consultants.is_empty() {
                consult.consultants = trek_core::orchestrate::arena_defaults(&options);
            }
            // Judged by another model than the thread's own.
            let models = self.models_for(agent);
            let main = model.map(str::to_string).or_else(|| default_model(&models).map(|m| m.id.clone())).unwrap_or_default();
            let main = models.iter().find(|m| same_model(&main, &m.id)).map(|m| m.id.clone()).unwrap_or(main);
            consult.judge = trek_core::orchestrate::pick_judge((agent, &main), &consult.consultants, &options);
            if let Some(why) = trek_core::orchestrate::arena_problem(&consult) {
                return Err(tr::HostError::bad_request(format!("{why}.")));
            }
        }
        if consult.consultants.is_empty() {
            return Err(tr::HostError::bad_request("Name a model to consult: /consult sol high: your question"));
        }
        let name = |c: &trek_core::orchestrate::Consultant| format!("{} · {}", model_name(&self.models_for(&c.agent), &c.model), c.effort.label());
        Ok(trek_core::orchestrate::consult_prompt(message, &consult, name))
    }

    /// The slash commands thread `id` offers, as the composer's `/` picker lists them: Trek's
    /// own, then its agent's commands and skills in its folder.
    pub(super) fn remote_commands(&self, id: &str) -> tr::HostResult<Vec<tr::CommandInfo>> {
        let t = self.thread(id).ok_or_else(|| tr::HostError::not_found("No such thread"))?;
        let trek: Vec<&str> = BUILTIN_COMMANDS.iter().map(|(n, _)| *n).collect();
        Ok(self
            .slash_commands(&Scope::Thread(id.to_string()), &t.agent)
            .into_iter()
            .map(|c| tr::CommandInfo {
                trek: trek.contains(&c.name.as_str()),
                kind: match c.kind {
                    trek_agents::CommandKind::Command => tr::CommandKind::Command,
                    trek_agents::CommandKind::Skill => tr::CommandKind::Skill,
                    trek_agents::CommandKind::Agent => tr::CommandKind::Agent,
                },
                name: c.name,
                description: c.description,
            })
            .collect())
    }
}
