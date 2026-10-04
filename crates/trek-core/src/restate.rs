//! "In your own words" (pstack): before it does anything, the agent restates what it was asked,
//! in plain English, and stops there. The user catches a misunderstanding before any work is done,
//! without having led the agent with their own guesses. The composer adds the request to a
//! message; the transcript shows the message as written.

const OPEN: &str = "<trek-restate>";
const CLOSE: &str = "</trek-restate>";

/// What the agent is asked to do.
const ASK: &str = "Before you do anything else, restate in your own words and in plain English what you understand I'm asking for, and the problem underneath it. Read what you need to understand it, but don't change any files or start on the work, and don't hand it to anyone else yet. Then stop, and wait for me to confirm or correct you.";

/// The message that confirms a restatement.
pub const GO_AHEAD: &str = "That's right — go ahead.";

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
    fn it_goes_with_a_consult() {
        let consult = Consult { consultants: vec![Consultant { agent: AgentId::Codex, model: "gpt-5.6-sol".into(), effort: Effort::High }], ..Default::default() };
        let text = consult_prompt(&with_restate("Fix it"), &consult, |_| "Sol".into());
        let (rest, back) = split_consult(&text);
        assert!(back.is_some());
        assert_eq!(split_restate(rest), ("Fix it", true));
        assert_eq!(as_written(&text), "Fix it");
    }
}
