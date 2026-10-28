//! Node.js for agent installs that run npm (`trek_core::node`): set up before such an install on
//! a computer without npm, and offered when an install stops because npm or node wasn't found.
//! Onboarding's Agents step and Settings › Agents show where it's got (`NodeSetup`).

use super::{Workspace, WorkspaceEvent};
use trek_core::node::{self, Progress};

/// Setting up Node.js for an agent's install, while there's something to show about it.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeSetup {
    /// Trek is installing its own Node.js; `then` (an install line) runs once it's in place.
    Installing {
        progress: Option<Progress>,
        then: Option<String>,
    },
    /// The install line `then` stopped: npm or node wasn't found.
    Missing { then: String },
    /// Trek couldn't install Node.js.
    Failed { error: String, then: Option<String> },
}

impl NodeSetup {
    /// The install line that runs once Node.js is in place.
    pub fn then(&self) -> Option<&str> {
        match self {
            NodeSetup::Installing { then, .. } | NodeSetup::Failed { then, .. } => then.as_deref(),
            NodeSetup::Missing { then } => Some(then),
        }
    }

    /// The agent it's for ("Pi"), when the line is one Trek lists.
    pub fn agent(&self) -> Option<String> {
        self.then().and_then(trek_core::catalog::installed_by)
    }
}

enum Step {
    Progress(Progress),
    Done(Result<std::path::PathBuf, String>),
}

/// What runs before an install line when there's no npm and Homebrew is here: its Node, in the
/// same terminal.
const BREW_NODE: &str = "brew install node && ";

impl Workspace {
    /// Run an agent's install line in a terminal (agents are looked for again when it ends). A
    /// line that runs npm on a computer without it gets Node.js first: Homebrew's, in the same
    /// terminal, when Homebrew is here; otherwise Trek's own (`install_node`).
    pub fn install_agent(&mut self, line: &str, cx: &mut gpui_kit::Context<Self>) {
        let line = line.trim().to_string();
        if node::needs_node(&line) && !trek_core::paths::isolated() && node::find_npm().is_none() {
            if !cfg!(windows) && trek_core::detect::which("brew").is_some() {
                self.node_setup = None;
                self.run_setup(format!("{BREW_NODE}{line}"), cx);
            } else {
                self.install_node(Some(line), cx);
            }
            return;
        }
        self.run_setup(line, cx);
    }

    fn run_setup(&mut self, command: String, cx: &mut gpui_kit::Context<Self>) {
        cx.emit(WorkspaceEvent::RunInTerminal { command, cwd: None });
        cx.notify();
    }

    /// Download Node.js from nodejs.org into Trek's data folder (`node::install`), showing how far
    /// it's got, then run `then` in a terminal. Once at a time.
    pub fn install_node(&mut self, then: Option<String>, cx: &mut gpui_kit::Context<Self>) {
        if matches!(self.node_setup, Some(NodeSetup::Installing { .. })) {
            return;
        }
        self.node_setup = Some(NodeSetup::Installing {
            progress: None,
            then: then.clone(),
        });
        cx.notify();
        let (tx, rx) = async_channel::unbounded();
        trek_core::runtime().spawn(async move {
            let progress = tx.clone();
            let done = node::install(&move |p| {
                let _ = progress.try_send(Step::Progress(p));
            })
            .await
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Step::Done(done)).await;
        });
        let task = cx.spawn(async move |this, cx| {
            while let Ok(step) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    match step {
                        Step::Progress(p) => {
                            if let Some(NodeSetup::Installing { progress, .. }) =
                                &mut this.node_setup
                            {
                                *progress = Some(p);
                            }
                        }
                        Step::Done(Ok(_)) => {
                            this.node_setup = None;
                            cx.emit(WorkspaceEvent::Toast {
                                message: "Node.js is installed for Trek's agents".into(),
                                undo: None,
                            });
                            if let Some(line) = then.clone() {
                                this.run_setup(line, cx);
                            }
                        }
                        Step::Done(Err(error)) => {
                            tracing::warn!("installing Node.js: {error}");
                            this.node_setup = Some(NodeSetup::Failed {
                                error,
                                then: then.clone(),
                            });
                        }
                    }
                    cx.notify();
                });
            }
        });
        self.keep(task);
    }

    /// A Trek command finished in a terminal with `output` last on its screen. An install that
    /// runs npm and found no npm or node (said so, or there's still none to be found) is said
    /// plainly, with Node.js to install, instead of left as the shell's error.
    pub fn setup_finished(
        &mut self,
        command: &str,
        output: &str,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        if !node::needs_node(command) {
            return;
        }
        if node::missing_in_output(output) || node::find_npm().is_none() {
            let then = command
                .strip_prefix(BREW_NODE)
                .unwrap_or(command)
                .to_string();
            self.node_setup = Some(NodeSetup::Missing { then });
        } else if !matches!(self.node_setup, Some(NodeSetup::Installing { .. })) {
            self.node_setup = None;
        }
        cx.notify();
    }

    /// Put away what's shown about setting up Node.js (not while it installs).
    pub fn dismiss_node_setup(&mut self, cx: &mut gpui_kit::Context<Self>) {
        if !matches!(self.node_setup, Some(NodeSetup::Installing { .. })) {
            self.node_setup = None;
            cx.notify();
        }
    }
}
