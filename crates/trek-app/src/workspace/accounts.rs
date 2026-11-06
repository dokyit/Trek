//! Accounts (`trek_core::accounts`) in the app: adding and signing them in, which one a thread
//! runs on, what each says of its plan and limits (the Usage card and the limits Trek keeps to),
//! and carrying a thread on to another when a usage limit stops it. The user's normal login isn't
//! an account here: it's `Thread::account == None`, and `agent_status` keeps what it says, so
//! nothing changes for anyone who adds none.

use super::usage::{READ_EVERY_MS, Snapshot, UsageRow};
use super::{Scope, Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use trek_agents::{AgentStatus, LimitScope, UsageLimit};
use trek_core::AgentId;
use trek_core::accounts::Account;

/// What the picker and the limit bar call the user's normal login once there are others.
pub const DEFAULT_LABEL: &str = "Default";

/// How long an account that met a limit with no reset known isn't offered to go on with.
const LIMITED_FOR_MS: i64 = 60 * 60_000;

/// Reads an account's status, answering on the channel: the agent's own report by default; tests
/// answer for any account.
pub type AccountFetch =
    Rc<dyn Fn(&Account, &Path) -> async_channel::Receiver<Result<AgentStatus, String>>>;

/// What accounts have said, by account id. The user's normal login has `agent_status` for this.
#[derive(Default)]
pub struct AccountState {
    pub status: HashMap<String, AgentStatus>,
    /// What each said in an earlier run: shown until it's read again.
    pub cached: HashMap<String, Snapshot>,
    fetched: HashMap<String, i64>,
    inflight: HashSet<String>,
    /// The accounts a limit stopped a thread on (the normal login by its agent's key), and until
    /// when: not offered to go on with until then, whatever their last reading says.
    limited: HashMap<String, i64>,
    pub fetch: Option<AccountFetch>,
}

impl AccountState {
    /// What was kept last time (none when there's no file, or it can't be read).
    pub fn load() -> AccountState {
        AccountState {
            cached: std::fs::read(snapshots_file())
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}

fn snapshots_file() -> PathBuf {
    trek_core::paths::data_dir().join("accounts-usage.json")
}

/// One way to run an agent, as the picker lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountChoice {
    /// `None`: the user's normal login.
    pub id: Option<String>,
    pub label: String,
    /// Who it's signed in as, when that's known.
    pub detail: Option<String>,
}

/// An account's own report, read on Trek's tokio runtime with its folder set.
fn read_account(
    account: &Account,
    cwd: &Path,
) -> async_channel::Receiver<Result<AgentStatus, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let (account, cwd) = (account.clone(), cwd.to_path_buf());
    trek_core::runtime().spawn(async move {
        let status = trek_agents::on_account(Some(&account), async {
            match account.agent() {
                AgentId::ClaudeCode => trek_agents::claude_status(&cwd).await,
                _ => trek_agents::codex_status(&cwd).await,
            }
        })
        .await;
        let _ = tx.send(status.map_err(|e| e.to_string())).await;
    });
    rx
}

impl Workspace {
    pub fn account(&self, id: &str) -> Option<&Account> {
        self.settings.accounts.iter().find(|a| a.id == id)
    }

    /// `agent`'s accounts beyond the user's normal login.
    pub fn accounts_of(&self, agent: &AgentId) -> Vec<Account> {
        let key = agent.key();
        self.settings
            .accounts
            .iter()
            .filter(|a| a.agent == key)
            .cloned()
            .collect()
    }

    pub fn has_accounts(&self, agent: &AgentId) -> bool {
        let key = agent.key();
        self.settings.accounts.iter().any(|a| a.agent == key)
    }

    /// `account` if it's one of `agent`'s: another agent's, or one that's gone, means the normal
    /// login.
    pub(super) fn account_on(&self, agent: &AgentId, account: Option<String>) -> Option<String> {
        account.filter(|id| self.account(id).is_some_and(|a| a.agent == agent.key()))
    }

    /// The account a thread started by `parent` runs on: its own, if it runs the same agent.
    pub(super) fn account_after(
        &self,
        parent: &trek_core::store::Thread,
        agent: &AgentId,
    ) -> Option<String> {
        (parent.agent == *agent)
            .then(|| parent.account.clone())
            .flatten()
    }

    /// What a thread on `account` of `agent` says it runs on. Nothing while the agent has only
    /// the user's normal login, which is what nearly everyone has.
    pub fn account_label(&self, agent: &AgentId, account: Option<&str>) -> Option<String> {
        if !self.has_accounts(agent) {
            return None;
        }
        Some(
            account
                .and_then(|id| self.account(id))
                .map_or_else(|| DEFAULT_LABEL.to_string(), |a| a.label.clone()),
        )
    }

    /// `account_label` for the agent and account `scope` runs: nothing for a thread on another
    /// machine, whose accounts are its own.
    pub fn account_label_in(&self, scope: &Scope) -> Option<String> {
        self.account_label_for(scope, &self.prefs_in(scope))
    }

    /// `account_label_in` for `prefs` as `scope`'s menus show them: a switch waiting for the next
    /// message names the account it goes to (`chosen_prefs_in`).
    pub fn account_label_for(&self, scope: &Scope, prefs: &super::Prefs) -> Option<String> {
        if self.on_other_machine(scope) {
            return None;
        }
        self.account_label(&prefs.agent, prefs.account.as_deref())
    }

    fn on_other_machine(&self, scope: &Scope) -> bool {
        self.thread_id_in(scope)
            .is_some_and(crate::machines::is_remote)
    }

    /// The accounts `agent` can run on, the user's normal login first. Empty while that's the
    /// only one: nothing to choose.
    pub fn account_choices(&self, agent: &AgentId) -> Vec<AccountChoice> {
        let others = self.accounts_of(agent);
        if others.is_empty() {
            return vec![];
        }
        let who = |status: Option<&AgentStatus>| status.and_then(|s| s.account.clone());
        let mut out = vec![AccountChoice {
            id: None,
            label: DEFAULT_LABEL.to_string(),
            detail: who(self.agent_status.get(&agent.key())),
        }];
        out.extend(others.into_iter().map(|a| AccountChoice {
            // Pickable all the same: its session says what's wrong, as the normal login's does.
            detail: if self.account_signed_in(&a.id) {
                who(self.accounts.status.get(&a.id))
            } else {
                Some("Not signed in".to_string())
            },
            label: a.label,
            id: Some(a.id),
        }));
        out
    }

    /// Whether the account has said it's signed in: this run, or in an earlier one until it's
    /// read again.
    fn account_signed_in(&self, id: &str) -> bool {
        match self.accounts.status.get(id) {
            Some(s) => s.logged_in || s.account.is_some(),
            None => self.accounts.cached.contains_key(id),
        }
    }

    /// `account_choices` for the agent `scope` runs.
    pub fn account_choices_in(&self, scope: &Scope) -> Vec<AccountChoice> {
        self.account_choices_for(scope, &self.prefs_in(scope).agent)
    }

    /// `account_choices_in` for `agent`, the one `scope`'s menus show (a switch waiting included).
    pub fn account_choices_for(&self, scope: &Scope, agent: &AgentId) -> Vec<AccountChoice> {
        if self.on_other_machine(scope) {
            return vec![];
        }
        self.account_choices(agent)
    }

    /// What `account` of `agent` last said (`None`: the normal login's own, in `agent_status`).
    pub fn status_on(&self, agent: &AgentId, account: Option<&str>) -> Option<&AgentStatus> {
        match account {
            Some(id) => self.accounts.status.get(id),
            None => self.agent_status.get(&agent.key()),
        }
    }

    pub(super) fn status_on_mut(
        &mut self,
        agent: &AgentId,
        account: Option<&str>,
    ) -> Option<&mut AgentStatus> {
        match account {
            Some(id) => self.accounts.status.get_mut(id),
            None => self.agent_status.get_mut(&agent.key()),
        }
    }

    /// Put what a probe said of `account` in its place.
    pub(super) fn set_status_on(
        &mut self,
        agent: &AgentId,
        account: Option<&str>,
        status: AgentStatus,
    ) {
        match account {
            Some(id) => self.accounts.status.insert(id.to_string(), status),
            None => self.agent_status.insert(agent.key(), status),
        };
    }

    /// Whether the account's sign-in is being checked.
    pub fn account_checking(&self, id: &str) -> bool {
        self.accounts.inflight.contains(id)
    }

    /// Add an account for `agent`, signed in to nothing yet. Returns its id.
    pub fn add_account(&mut self, agent: &AgentId, cx: &mut Context<Self>) -> Option<String> {
        if !trek_core::accounts::supported(agent) {
            return None;
        }
        let account = Account::new(agent, &self.settings.accounts);
        let id = account.id.clone();
        self.settings.accounts.push(account);
        self.save_settings(cx);
        cx.notify();
        Some(id)
    }

    /// Call the account what the user typed. Nothing typed changes nothing.
    pub fn rename_account(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(label) = trek_core::accounts::clean_label(text) else {
            return;
        };
        let Some(account) = self.settings.accounts.iter_mut().find(|a| a.id == id) else {
            return;
        };
        if account.label != label {
            account.label = label;
            self.save_settings(cx);
            cx.notify();
        }
    }

    /// Sign the account in with the agent's own login, in Trek's terminal.
    pub fn sign_in_account(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(account) = self.account(id).cloned() else {
            return;
        };
        let Some(command) = account.login_command() else {
            return;
        };
        // The agent wants its folder to be there (Codex refuses a `CODEX_HOME` that isn't).
        if account.open().is_none() {
            cx.emit(WorkspaceEvent::Toast {
                message: "Couldn't make this account's folder".into(),
                undo: None,
            });
            return;
        }
        cx.emit(WorkspaceEvent::RunInTerminal { command, cwd: None });
    }

    /// A Trek command finished in a terminal: an account's sign-in shows at once.
    pub(super) fn account_setup_finished(&mut self, command: &str, cx: &mut Context<Self>) {
        let signed_in = self
            .settings
            .accounts
            .iter()
            .find(|a| a.login_command().as_deref() == Some(command))
            .map(|a| a.id.clone());
        if let Some(id) = signed_in {
            self.fetch_account(&id, true, cx);
        }
    }

    /// Remove an account: its threads go back to the user's normal login, each picking up from a
    /// recap (their sessions go with its folder), and it's signed out. The normal login isn't
    /// touched: the agent's sign-out runs with the account's own folder set, and only that folder
    /// is deleted.
    pub fn remove_account(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(account) = self.account(id).cloned() else {
            return;
        };
        // While it still has a name: the transcript says where each moved from.
        let on_it: Vec<String> = self
            .threads
            .iter()
            .filter(|t| t.account.as_deref() == Some(id))
            .map(|t| t.id.clone())
            .collect();
        for thread in on_it {
            let scope = Scope::Thread(thread);
            let prefs = super::Prefs {
                account: None,
                ..self.prefs_in(&scope)
            };
            self.set_prefs_in(&scope, prefs, cx);
        }
        // Archived ones too, which aren't loaded.
        if let Err(e) = self.store.forget_account(id) {
            tracing::warn!("forget account: {e:#}");
        }
        self.settings.accounts.retain(|a| a.id != id);
        self.save_settings(cx);
        for prefs in [
            &mut self.draft_prefs,
            &mut self.ide_chat.draft_prefs,
            &mut self.side_chat.draft_prefs,
        ] {
            if prefs.account.as_deref() == Some(id) {
                prefs.account = None;
            }
        }
        self.accounts.status.remove(id);
        self.accounts.fetched.remove(id);
        self.accounts.limited.remove(id);
        if self.accounts.cached.remove(id).is_some() {
            self.save_account_snapshots(cx);
        }
        trek_core::runtime().spawn(trek_agents::remove_account_login(account));
        cx.notify();
    }

    /// Read the accounts' plans and limits: those of the agents the Usage card shows, and any not
    /// signed in as far as is known (Settings and the picker say who each is signed in as, and a
    /// sign-in may have finished out of Trek's sight). Not those of an agent turned off.
    pub(super) fn read_accounts(&mut self, force: bool, cx: &mut Context<Self>) {
        let shown = self.usage_shown();
        let due: Vec<String> = self
            .settings
            .accounts
            .iter()
            .filter(|a| !self.settings.disabled_agents.contains(&a.agent))
            .filter(|a| {
                let signed_in = self.accounts.status.get(&a.id).is_some_and(|s| s.logged_in);
                shown.contains(&a.agent()) || !signed_in
            })
            .map(|a| a.id.clone())
            .collect();
        for id in due {
            self.fetch_account(&id, force, cx);
        }
    }

    /// Read the accounts of `agent` (a turn on it ended), at most every 30 seconds.
    pub(super) fn read_accounts_of(&mut self, agent: &AgentId, cx: &mut Context<Self>) {
        for account in self.accounts_of(agent) {
            self.fetch_account(&account.id, false, cx);
        }
    }

    /// Read one account's plan and limits, unless it was read in the last 30 seconds (or is being
    /// read) and `force` isn't set. Never in an isolated process, unless a test answers for it.
    pub(super) fn fetch_account(&mut self, id: &str, force: bool, cx: &mut Context<Self>) {
        let Some(account) = self.account(id).cloned() else {
            return;
        };
        let fetch: AccountFetch = match &self.accounts.fetch {
            Some(f) => f.clone(),
            // A live test (`TREK_LIVE_AGENT`) asks the real agent, as the app does.
            None if trek_core::paths::isolated()
                && std::env::var_os("TREK_LIVE_AGENT").is_none() =>
            {
                return;
            }
            None if trek_core::accounts::config_var(&account.agent()).is_none() => return,
            None => Rc::new(read_account),
        };
        let now = self.now();
        let fresh = self.accounts.inflight.contains(id)
            || self
                .accounts
                .fetched
                .get(id)
                .is_some_and(|at| now - at < READ_EVERY_MS);
        if (!force && fresh) || self.agent_updating(&account.agent) {
            return;
        }
        self.accounts.inflight.insert(id.to_string());
        let folder = self.current_cwd().unwrap_or_else(trek_core::paths::home);
        let rx = fetch(&account, &folder);
        let id = id.to_string();
        let task = cx.spawn(async move |this, cx| {
            let res = rx
                .recv()
                .await
                .unwrap_or_else(|_| Err("the read was dropped".into()));
            let _ = this.update(cx, |this, cx| this.account_read(&id, res, cx));
        });
        self.keep(task);
        cx.notify();
    }

    /// An account said what its usage is (or failed to): the card and the limits Trek keeps to
    /// follow, and what it said is kept for the next launch.
    fn account_read(&mut self, id: &str, res: Result<AgentStatus, String>, cx: &mut Context<Self>) {
        self.accounts.inflight.remove(id);
        let Some(agent) = self.account(id).map(Account::agent) else {
            return;
        };
        let now = self.now();
        self.accounts.fetched.insert(id.to_string(), now);
        match res {
            Ok(st) => {
                let snapshot = Snapshot::of(&st, now);
                if snapshot.plan.is_some() || !snapshot.limits.is_empty() {
                    self.accounts.cached.insert(id.to_string(), snapshot);
                    self.save_account_snapshots(cx);
                }
                self.accounts.status.insert(id.to_string(), st);
            }
            Err(e) => {
                self.accounts
                    .status
                    .entry(id.to_string())
                    .or_default()
                    .error = Some(e);
            }
        }
        self.fill_unknown_resets(cx);
        self.wrap_up_due_on(&agent, Some(id), cx);
        cx.notify();
    }

    /// Written off the main thread. Plans and limits only: no login is kept here.
    fn save_account_snapshots(&mut self, cx: &mut Context<Self>) {
        let (path, data) = (
            snapshots_file(),
            serde_json::to_vec(&self.accounts.cached).unwrap_or_default(),
        );
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = std::fs::write(&path, data) {
                    tracing::warn!("keep account usage: {e}");
                }
            })
            .detach();
    }

    /// The Usage card's rows for `agent`'s accounts, beneath its own. One not signed in says so,
    /// rather than that it has no limits.
    pub fn account_rows(&self, agent: &AgentId) -> Vec<UsageRow> {
        let now = self.now();
        let not_signed_in = |mut row: UsageRow| {
            row.no_limits = false;
            row.error = Some("Not signed in".to_string());
            row
        };
        self.accounts_of(agent)
            .into_iter()
            .map(|a| {
                let status = self.accounts.status.get(&a.id);
                let cached = self.accounts.cached.get(&a.id);
                let mut row = match (status, cached) {
                    (Some(st), _) if st.logged_in || st.account.is_some() => {
                        UsageRow::of_status(agent.clone(), st, None)
                    }
                    (Some(st), _) => not_signed_in(UsageRow::of_status(agent.clone(), st, None)),
                    (None, Some(s)) => UsageRow::of_snapshot(agent.clone(), s, now, None),
                    (None, None) if self.account_checking(&a.id) => {
                        UsageRow::empty(agent.clone(), None)
                    }
                    (None, None) => not_signed_in(UsageRow::empty(agent.clone(), None)),
                };
                row.account = Some(a.id);
                row
            })
            .collect()
    }

    /// What the Usage card calls a row of `agent` once it has accounts ("Default", "Work").
    pub fn usage_row_label(&self, row: &UsageRow) -> Option<String> {
        self.account_label(&row.agent, row.account.as_deref())
    }

    /// The account to carry `id` on with, when a limit stops it: the one with most room of those
    /// signed in, other than the one it's on. Not one that has met a limit and hasn't reset, or
    /// has used up a window this thread needs. `None`: nowhere to go.
    pub fn next_account(&self, id: &str) -> Option<AccountChoice> {
        if crate::machines::is_remote(id) {
            return None;
        }
        let t = self.thread(id)?;
        let now = self.now();
        let room = |choice: &AccountChoice| -> Option<f32> {
            let account = choice.id.as_deref();
            if self
                .accounts
                .limited
                .get(account.unwrap_or(&t.agent.key()))
                .is_some_and(|until| *until > now)
            {
                return None;
            }
            let status = self.status_on(&t.agent, account);
            let cached = account.and_then(|a| self.accounts.cached.get(a));
            // An added account has to have said it's signed in; the normal login is taken to be
            // until it says otherwise, as everywhere else.
            let signed_in = match account {
                Some(id) => self.account_signed_in(id),
                None => status.is_none_or(|s| s.logged_in || s.account.is_some()),
            };
            if !signed_in {
                return None;
            }
            let limits: &[UsageLimit] = match (status, cached) {
                (Some(s), _) => &s.limits,
                (None, Some(s)) => &s.limits,
                (None, None) => &[],
            };
            let fullest = limits
                .iter()
                .filter(|l| trek_agents::limits::applies(l, &LimitScope::Other, t.model.as_deref()))
                .filter(|l| l.resets_at.is_none_or(|at| at > now))
                .map(|l| l.percent)
                .fold(0., f32::max);
            (fullest < 99.5).then_some(fullest)
        };
        self.account_choices(&t.agent)
            .into_iter()
            .filter(|c| c.id != t.account)
            .filter_map(|c| room(&c).map(|fullest| (c, fullest)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| c)
    }

    /// A limit stopped a turn on `account` of `agent`: it isn't offered as a way on until `resets_at`.
    pub(super) fn note_account_limited(
        &mut self,
        agent: &AgentId,
        account: Option<&str>,
        resets_at: Option<i64>,
    ) {
        if !self.has_accounts(agent) {
            return;
        }
        let until = resets_at.unwrap_or_else(|| self.now() + LIMITED_FOR_MS);
        let key = account.map_or_else(|| agent.key(), str::to_string);
        self.accounts.limited.insert(key, until);
    }

    /// "Continue on <account>": `id` moves there and carries on now, in a new session started from
    /// a recap (its session stays behind in the other account's folder). What waited for the reset
    /// goes first; with nothing waiting, it's told to continue.
    pub fn continue_on_account(
        &mut self,
        id: &str,
        account: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let pause = self.pause(id).cloned();
        // The pause ends here, not with the move (`set_prefs_in`), which hands what's queued back
        // to the composer.
        self.mutate_thread(id, cx, |t| {
            t.paused = None;
            t.snoozed_until = None;
        });
        let scope = Scope::Thread(id.to_string());
        let prefs = super::Prefs {
            account,
            ..self.prefs_in(&scope)
        };
        self.set_prefs_in(&scope, prefs, cx);
        if let Some(mut pause) = pause {
            pause.tries = 0;
            self.send_resume(id, &pause, cx);
            self.deliver_wakes(id, cx);
        }
        self.schedule_limits(cx);
    }
}
