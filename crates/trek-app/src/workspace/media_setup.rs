//! ffmpeg and yt-dlp for the video watcher, downloaded by Trek into its own folder
//! (`trek_core::media_tools`) from Settings › Tools › Video, on a computer where Homebrew or
//! winget isn't there to install them. The watcher itself downloads them the first time an agent
//! needs one (`trek-mcp video`); this is the same download with its progress shown.

use super::{Workspace, WorkspaceEvent};
use trek_core::media_tools::{self, Progress, Tool};

/// Downloading one of the video watcher's tools, while there's something to show about it.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaSetup {
    Downloading {
        tool: Tool,
        progress: Option<Progress>,
    },
    /// Trek couldn't download it.
    Failed { tool: Tool, error: String },
}

impl MediaSetup {
    pub fn tool(&self) -> Tool {
        match self {
            MediaSetup::Downloading { tool, .. } | MediaSetup::Failed { tool, .. } => *tool,
        }
    }

    /// The line the Settings row shows while it's downloading ("Downloading ffmpeg · 42%").
    pub fn label(&self) -> Option<String> {
        match self {
            MediaSetup::Downloading { tool, progress } => Some(
                progress
                    .as_ref()
                    .map_or_else(|| format!("Starting {}…", tool.name()), |p| p.label(*tool)),
            ),
            MediaSetup::Failed { .. } => None,
        }
    }
}

enum Step {
    Progress(Progress),
    Done(Result<(), String>),
}

impl Workspace {
    /// Download `tool` into Trek's folder (`media_tools::install`), showing how far it's got.
    /// One at a time.
    pub fn install_media_tool(&mut self, tool: Tool, cx: &mut gpui_kit::Context<Self>) {
        if matches!(self.media_setup, Some(MediaSetup::Downloading { .. })) {
            return;
        }
        self.media_setup = Some(MediaSetup::Downloading {
            tool,
            progress: None,
        });
        cx.notify();
        let (tx, rx) = async_channel::unbounded();
        trek_core::runtime().spawn(async move {
            let progress = tx.clone();
            let done = media_tools::install(tool, &move |p| {
                let _ = progress.try_send(Step::Progress(p));
            })
            .await
            .map_err(|e| {
                tracing::warn!("downloading {}: {e:#}", tool.name());
                format!("{e:#}")
            });
            let _ = tx.send(Step::Done(done)).await;
        });
        let task = cx.spawn(async move |this, cx| {
            while let Ok(step) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    match step {
                        Step::Progress(p) => {
                            if let Some(MediaSetup::Downloading { progress, .. }) =
                                &mut this.media_setup
                            {
                                *progress = Some(p);
                            }
                        }
                        Step::Done(Ok(())) => {
                            this.media_setup = None;
                            this.media_revision += 1;
                            cx.emit(WorkspaceEvent::Toast {
                                message: format!("{} is installed for Trek", tool.name()),
                                undo: None,
                            });
                        }
                        Step::Done(Err(error)) => {
                            this.media_setup = Some(MediaSetup::Failed { tool, error });
                        }
                    }
                    cx.notify();
                });
            }
        });
        self.keep(task);
    }

    /// Put away what's shown about a failed download.
    pub fn dismiss_media_setup(&mut self, cx: &mut gpui_kit::Context<Self>) {
        if matches!(self.media_setup, Some(MediaSetup::Failed { .. })) {
            self.media_setup = None;
            cx.notify();
        }
    }
}
