//! Trek's own `/` commands as the AI side bar takes them, the way the harness composer does:
//! `/consult <models>: <message>` asks other models first (`trek_core::orchestrate`), `/restate`
//! has the agent say back what it's asked before it starts (`trek_core::restate`), `/new` and
//! `/clear` start a new chat here. The rest go to the agent as typed.

use crate::composer::{arena_options, consult_split, consultant, default_model, find_model, model_name, same_model};
use crate::workspace::{Scope, Workspace};
use trek_core::orchestrate::{Consult, Consultant, Style};
use trek_core::Effort;

/// `/new` or `/clear`, alone.
pub(crate) fn is_new_chat(text: &str) -> bool {
    matches!(text.trim(), "/new" | "/clear")
}

/// `/consult sol high, opus max: <message>` (also `discuss`, `advise`, `arena`, `report`,
/// `implement`): `consult` with the consultants and style it names, and the message to send
/// (`None`: just picked, for the next message). `None` overall when `text` isn't the command.
pub(crate) fn consult_command(ws: &Workspace, text: &str, mut consult: Consult) -> Option<Result<(Consult, Option<String>), String>> {
    let rest = text.trim_start().strip_prefix("/consult")?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let agent = ws.prefs_in(&Scope::Ide).agent;
    if let Some(why) = ws.consult_unavailable(&agent) {
        return Some(Err(format!("Can't consult: {why}.")));
    }
    let (targets, message) = match consult_split(rest) {
        Some((t, m)) => (t, Some(m.trim().to_string()).filter(|m| !m.is_empty())),
        None => (rest, None),
    };
    let mut picked = vec![];
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
        let Some((agent, model)) = find_model(ws, &query) else { return Some(Err(format!("There's no model called “{query}” to consult."))) };
        picked.push(consultant(agent, &model, effort));
    }
    let most = trek_core::orchestrate::MAX_RUNNING;
    if consult.style == Style::Arena && picked.len() > most {
        return Some(Err(format!("An arena drafts {most} designs at most, all at once: name up to {most} models.")));
    }
    if !picked.is_empty() {
        consult.consultants = picked;
    }
    if consult.style == Style::Arena {
        consult.consultants.truncate(most);
        if consult.consultants.is_empty() {
            consult.consultants = trek_core::orchestrate::arena_defaults(&arena_options(ws));
        }
    }
    if consult.consultants.is_empty() {
        return Some(Err("Name a model to consult: /consult sol high: your question".into()));
    }
    Some(Ok((consult, message)))
}

/// "Sol · High": how a consultant is named to the agent and on the input's chip.
pub(crate) fn consultant_name(ws: &Workspace, c: &Consultant) -> String {
    format!("{} · {}", model_name(&ws.models_for(&c.agent), &c.model), c.effort.label())
}

/// `text` with `consult`'s instructions for the agent, an arena's judge picked when none is.
pub(crate) fn consult_prompt(ws: &Workspace, text: &str, consult: &Consult) -> Result<String, String> {
    let mut consult = consult.clone();
    if consult.style == Style::Arena && consult.judge.is_none() {
        let prefs = ws.prefs_in(&Scope::Ide);
        let models = ws.models_for(&prefs.agent);
        let model = prefs.model.clone().or_else(|| default_model(&models).map(|m| m.id.clone())).unwrap_or_default();
        let model = models.iter().find(|m| same_model(&model, &m.id)).map(|m| m.id.clone()).unwrap_or(model);
        consult.judge = trek_core::orchestrate::pick_judge((&prefs.agent, &model), &consult.consultants, &arena_options(ws));
    }
    if let Some(why) = trek_core::orchestrate::arena_problem(&consult) {
        return Err(format!("{why}."));
    }
    Ok(trek_core::orchestrate::consult_prompt(text, &consult, |c| consultant_name(ws, c)))
}

#[cfg(test)]
mod tests {
    use super::is_new_chat;

    #[test]
    fn new_and_clear_alone_start_a_new_chat() {
        assert!(is_new_chat("/new") && is_new_chat(" /clear "));
        assert!(!is_new_chat("/newer") && !is_new_chat("/new thing") && !is_new_chat("new"));
    }
}
