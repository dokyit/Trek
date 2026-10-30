//! The live cards (`pr_card`) on the phone: a pull request, a workflow run or a release as the
//! Mac draws it, sent as a `card` item that the phone draws the same way and that goes again
//! whenever the card moves on (each tick compares what a phone would read, so a card `gh` keeps
//! rewriting unchanged costs nothing). Its buttons come back as `card_action` and do what the
//! Mac's own do.

use crate::pr_card::{self, Card, CheckState, Kind, Phase};
use crate::workspace::Workspace;
use gpui_kit::Context;
use trek_core::store::Item;
use trek_remote as tr;

/// The most of a card's words (its title, its step) that goes to the phone.
const MAX_CARD_TEXT: usize = 4 << 10;
/// The most files of a conflict, and checks or jobs, a card carries.
const MAX_CARD_ROWS: usize = 64;

fn short(text: &str) -> String {
    super::clip_text_to(text, MAX_CARD_TEXT)
}

/// The card for `key` as the phone gets it.
pub(super) fn remote_card(key: &str, card: &Card) -> tr::RemoteCard {
    let state = |s: CheckState| match s {
        CheckState::Queued => tr::CardCheckState::Queued,
        CheckState::Running => tr::CardCheckState::Running,
        CheckState::Pass => tr::CardCheckState::Pass,
        CheckState::Fail => tr::CardCheckState::Fail,
        CheckState::Skipped => tr::CardCheckState::Skipped,
    };
    let paths =
        |v: &[String]| -> Vec<String> { v.iter().take(MAX_CARD_ROWS).map(|p| short(p)).collect() };
    tr::RemoteCard {
        key: key.to_string(),
        kind: match card.kind {
            Kind::Pr => tr::CardKind::Pr,
            Kind::Release => tr::CardKind::Release,
            Kind::Run => tr::CardKind::Run,
        },
        phase: match card.phase {
            Phase::Creating => tr::CardPhase::Creating,
            Phase::Open => tr::CardPhase::Open,
            Phase::Checking => tr::CardPhase::Checking,
            Phase::Conflicted => tr::CardPhase::Conflicted,
            Phase::Ready => tr::CardPhase::Ready,
            Phase::Merging => tr::CardPhase::Merging,
            Phase::Merged => tr::CardPhase::Merged,
            Phase::Closed => tr::CardPhase::Closed,
            Phase::Releasing => tr::CardPhase::Releasing,
            Phase::Published => tr::CardPhase::Published,
            Phase::Passed => tr::CardPhase::Passed,
            Phase::Failed => tr::CardPhase::Failed,
        },
        title: short(&card.title),
        number: card.number,
        url: card.url.clone(),
        head: short(&card.head),
        base: short(&card.base),
        repo: card.repo.clone(),
        commits: card.commits,
        history: card
            .history
            .iter()
            .map(|c| tr::CardCommit {
                title: short(&c.title),
                oid: c.oid.clone(),
                author: short(&c.author),
                committed_ms: c.committed_ms,
            })
            .collect(),
        added: card.added,
        removed: card.removed,
        checks: card
            .checks
            .iter()
            .take(MAX_CARD_ROWS)
            .map(|c| tr::CardCheck {
                name: short(&c.name),
                state: state(c.state),
            })
            .collect(),
        conflicts: paths(&card.conflicts),
        resolved: paths(&card.resolved),
        resolving: card.resolving,
        merging: card.merging,
        branch_deleted: card.branch_deleted,
        step: short(&card.step),
        version: card.version.clone(),
        channel: card.channel.clone(),
        stages: card
            .stages
            .iter()
            .take(MAX_CARD_ROWS)
            .map(|s| tr::CardStage {
                label: short(&s.label),
                state: state(s.state),
                started_ms: s.started_ms,
                ended_ms: s.ended_ms,
            })
            .collect(),
        subject: short(&card.subject),
        event: card.event.clone(),
        created_ms: card.created_ms,
        phase_at_ms: card.phase_at_ms,
        started_ms: card.started_ms,
        ended_ms: card.ended_ms,
        actions: actions(card),
        notes_url: pr_card::notes_url(card),
    }
}

/// The buttons `card` shows on the Mac right now (its footer's), which the phone may press.
fn actions(card: &Card) -> Vec<tr::CardActionKind> {
    let mut out = vec![];
    let pr = card.kind == Kind::Pr;
    if pr && card.phase == Phase::Ready && (card.url.is_some() || card.number.is_some()) {
        out.push(tr::CardActionKind::Merge);
    }
    if pr && card.phase == Phase::Conflicted && !card.resolving {
        out.push(tr::CardActionKind::Resolve);
    }
    if pr_card::rerun_id(card).is_some() {
        out.push(tr::CardActionKind::Rerun);
    }
    out
}

impl Workspace {
    /// A card's button pressed on the phone: what the Mac's own button does, once the card still
    /// offers it.
    pub(super) fn remote_card_action(
        &mut self,
        req: tr::CardActionRequest,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<()> {
        let Some(live) = self.live.get(&req.thread_id) else {
            return Err(tr::HostError::not_found("No such thread"));
        };
        let row = pr_card::row_id(&req.key);
        let Some(card) = live.items.iter().find_map(|i| match i {
            Item::Tool { id, detail, .. } if *id == row => pr_card::read(id, detail),
            _ => None,
        }) else {
            return Err(tr::HostError::not_found(
                "That card isn't in the thread any more",
            ));
        };
        if !actions(&card).contains(&req.action) {
            return Err(tr::HostError::conflict(match req.action {
                tr::CardActionKind::Merge => "It can't be merged right now",
                tr::CardActionKind::Resolve => "There's nothing to resolve right now",
                tr::CardActionKind::Rerun => "There's nothing to re-run right now",
            }));
        }
        match req.action {
            tr::CardActionKind::Merge => self.pr_merge(&req.thread_id, &row, cx),
            tr::CardActionKind::Resolve => self.pr_resolve(&req.thread_id, &row, cx),
            tr::CardActionKind::Rerun => self.pr_rerun(&req.thread_id, &row, cx),
        }
        cx.notify();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_goes_to_the_phone_with_the_buttons_it_shows() {
        let ready = Card {
            phase: Phase::Ready,
            ..pr_card::demo("ready")
        };
        let sent = remote_card("https://github.com/o/r/pull/128", &ready);
        assert_eq!(sent.key, "https://github.com/o/r/pull/128");
        assert_eq!(
            (sent.kind, sent.phase),
            (tr::CardKind::Pr, tr::CardPhase::Ready)
        );
        assert_eq!(sent.number, Some(128));
        assert_eq!(sent.history.len(), ready.history.len());
        assert_eq!(sent.actions, [tr::CardActionKind::Merge]);

        let conflicted = pr_card::demo("conflicted");
        assert_eq!(
            remote_card("k", &conflicted).actions,
            [tr::CardActionKind::Resolve]
        );
        let resolving = Card {
            resolving: true,
            ..conflicted
        };
        assert!(remote_card("k", &resolving).actions.is_empty());
        assert!(
            remote_card("k", &pr_card::demo("merging"))
                .actions
                .is_empty()
        );

        let failed = pr_card::demo("run-failed");
        let sent = remote_card("run-1", &failed);
        assert_eq!(sent.kind, tr::CardKind::Run);
        assert_eq!(sent.actions, [tr::CardActionKind::Rerun]);
        assert!(
            sent.stages
                .iter()
                .any(|s| s.state == tr::CardCheckState::Fail)
        );
    }

    #[test]
    fn a_published_release_carries_its_notes() {
        let card = pr_card::demo("release-published");
        let sent = remote_card("release-0.4.15", &card);
        assert_eq!(sent.kind, tr::CardKind::Release);
        assert_eq!(sent.phase, tr::CardPhase::Published);
        assert_eq!(sent.notes_url, pr_card::notes_url(&card));
        assert!(sent.actions.is_empty());
    }
}
