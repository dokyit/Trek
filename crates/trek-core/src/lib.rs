//! Trek's engine: domain types, persistence, agent discovery, thread import and updates.
//! Nothing in this crate depends on the UI toolkit.

pub mod agent_update;
pub mod artifact;
pub mod basecamp;
pub mod catalog;
pub mod changelog;
pub mod changes;
pub mod checkpoint;
pub mod detect;
pub mod elevated;
pub mod feeds;
pub mod file_kind;
pub mod git;
pub mod hunks;
pub mod import;
pub mod inline_edit;
pub mod keychain;
pub mod limit;
pub mod lineage;
pub mod mcp;
pub mod media;
pub mod node;
pub mod notes;
pub mod open_in;
pub mod orchestrate;
pub mod paths;
pub mod pricing;
pub mod procs;
pub mod registry;
pub mod restate;
pub mod rewind;
pub mod settings;
pub mod skills;
pub mod store;
pub mod transcript;
pub mod types;
pub mod update;
pub mod verification;
pub mod visualization;
pub mod winget;
pub mod worktree;

pub use types::*;

/// Title for a new thread from its first prompt.
pub fn import_title(text: &str) -> String {
    import::title_from(text)
}

use std::sync::LazyLock;

/// Shared tokio runtime for all I/O (subprocesses, HTTP, file scanning).
/// GPUI's executors are not tokio; UI code hands work to this runtime and
/// receives results over channels.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("trek-io")
            .enable_all()
            .build()
            .expect("tokio runtime")
    });
    &RT
}

pub const APP_NAME: &str = "Trek";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
