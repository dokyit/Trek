//! "In your own words" (pstack): before it does anything, the agent restates what it was asked,
//! in plain English, and stops there. The user catches a misunderstanding before any work is done,
//! without having led the agent with their own guesses. The composer adds the request to a
//! message; the transcript shows the message as written.

const OPEN: &str = "<trek-restate>";
const CLOSE: &str = "</trek-restate>";

/// What the agent is asked to do.
const ASK: &str = "Before you do anything else, restate in your own words and in plain English what you understand I'm asking for, and the problem underneath it. Read what you need to understand it, but don't change any files or start on the work, and don't hand it to anyone else yet. Then stop, and wait for me to confirm or correct you.";

/// What `/restate` alone sends in a thread under way: the agent restates the thread so far,
/// the problem underneath it included, without being led by a new request.
pub const THREAD: &str = "Where are we? Say back what this thread is about so far.";

/// The message that confirms a restatement.
pub const GO_AHEAD: &str = "That's right — go ahead.";

/// The go-ahead for a restatement of `asked` (the message that asked for it). What else that
/// message asked of the agent (a consult, an arena) was held back for the restatement, so it goes
/// with the go-ahead.
pub fn go_ahead(asked: &str) -> String {
    let (rest, consult) = crate::orchestrate::split_consult(asked);
    match consult {
        Some(_) => format!("{GO_AHEAD}{}", &asked[rest.len()..]),
        None => GO_AHEAD.into(),
    }
}

/// The go-ahead after a restatement, given the user's messages newest first. The exchange runs
/// back through the messages that asked for a restatement (a request, then corrections of it);
/// the newest of them that asked for more (a consult, an arena) is what goes ahead.
pub fn go_ahead_after<'a>(asked: impl IntoIterator<Item = &'a str>) -> String {
    asked
        .into_iter()
        .take_while(|m| split_restate(crate::orchestrate::split_consult(m).0).1)
        .find(|m| crate::orchestrate::split_consult(m).1.is_some())
        .map(go_ahead)
        .unwrap_or_else(|| GO_AHEAD.into())
}

/// `text`, asking the agent to restate it first.
pub fn with_restate(text: &str) -> String {
    format!("{}\n\n{OPEN}\n{ASK}\n{CLOSE}", text.trim_end())
}

/// A message split into what the user wrote and whether it asks for a restatement.
pub fn split_restate(text: &str) -> (&str, bool) {
    let Some(at) = text.rfind(&format!("\n\n{OPEN}")) else { return (text, false) };
    if !text[at..].trim_end().ends_with(CLOSE) {
        return (text, false);
    }
    (text[..at].trim_end(), true)
}

/// Whether the turn ending at `end` (its footer's index in `items`) was asked for a
/// restatement: its last message from the user, which may have been steered in mid-turn, did.
pub fn asked_in_turn(items: &[crate::store::Item], end: usize) -> bool {
    let Some(start) = crate::rewind::turn_start(items, end) else { return false };
    items[start..end.min(items.len())]
        .iter()
        .rev()
        .find_map(|i| match i {
            crate::store::Item::User { text, aside: false, .. } => Some(split_restate(crate::orchestrate::split_consult(text).0).1),
            _ => None,
        })
        .unwrap_or(false)
}

/// A message as the user wrote it, without what Trek added for the agent (a consult, a request
/// to restate it).
pub fn as_written(text: &str) -> &str {
    let (text, _) = crate::orchestrate::split_consult(text);
    split_restate(text).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrate::{Consult, Consultant, consult_prompt, split_consult};
    use crate::{AgentId, Effort};

    #[test]
    fn the_request_comes_off_again() {
        let text = with_restate("Why does the sidebar flicker?\n");
        assert!(text.starts_with("Why does the sidebar flicker?\n\n<trek-restate>") && text.contains("in your own words") && text.contains("don't change any files"));
        assert_eq!(split_restate(&text), ("Why does the sidebar flicker?", true));
        assert_eq!(split_restate("plain"), ("plain", false));
        assert_eq!(split_restate("a\n\n<trek-restate>\nunfinished"), ("a\n\n<trek-restate>\nunfinished", false));
        assert_eq!(as_written(&text), "Why does the sidebar flicker?");
    }

    #[test]
    fn a_turn_asked_for_one_by_its_last_message() {
        use crate::store::Item;
        let user = |t: &str| Item::User { text: t.into(), images: vec![], at: None, resume: None, aside: false };
        let said = |t: &str| Item::Assistant { text: t.into() };
        let end = || Item::TurnEnd { at: 1, took_secs: 1 };
        let asked = with_restate("Fix it");
        let items = vec![user(&asked), said("You want it fixed."), end()];
        assert!(asked_in_turn(&items, 2));
        // Steered in while a turn ran: the restatement was asked for mid-turn.
        let steered = vec![user("Fix it"), said("Looking."), user(&with_restate("Actually, the other bug")), said("You want the other bug fixed."), end()];
        assert!(asked_in_turn(&steered, 4));
        // A plain message steered in after it: the agent went on.
        let then = vec![user(&asked), said("You want…"), user("Just do it"), said("Done."), end()];
        assert!(!asked_in_turn(&then, 4));
        // The next turn, a go-ahead, asks nothing.
        let next = [items.clone(), vec![user(GO_AHEAD), said("Done."), end()]].concat();
        assert!(!asked_in_turn(&next, 5) && asked_in_turn(&next, 2));
    }

    #[test]
    fn it_goes_with_a_consult() {
        let consult = Consult { consultants: vec![Consultant { agent: AgentId::Codex, model: "gpt-5.6-sol".into(), effort: Effort::High }], ..Default::default() };
        let text = consult_prompt(&with_restate("Fix it"), &consult, |_| "Sol".into());
        let (rest, back) = split_consult(&text);
        assert!(back.is_some());
        assert_eq!(split_restate(rest), ("Fix it", true));
        assert_eq!(as_written(&text), "Fix it");
        // The go-ahead brings the consult that waited for it; a plain one is just the go-ahead.
        let ahead = go_ahead(&text);
        assert!(ahead.starts_with(GO_AHEAD) && !ahead.contains("<trek-restate>"), "{ahead}");
        assert_eq!(split_consult(&ahead), (GO_AHEAD, back));
        assert_eq!(go_ahead(&with_restate("Fix it")), GO_AHEAD);
        // A correction ("Not quite…") restated again: the consult the request held still goes.
        let correction = with_restate("No, the other sidebar");
        assert_eq!(go_ahead_after([correction.as_str(), text.as_str()]), ahead);
        assert_eq!(go_ahead_after([correction.as_str(), "Fix it", text.as_str()]), GO_AHEAD, "an earlier exchange's consult is spent");
        assert_eq!(go_ahead_after(["Fix it"]), GO_AHEAD);
        assert_eq!(go_ahead_after([]), GO_AHEAD);
    }
}
