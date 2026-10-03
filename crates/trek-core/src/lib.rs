//! Trek's engine: domain types, persistence, agent discovery, thread import and updates.
//! Nothing in this crate depends on the UI toolkit.

pub mod catalog;
pub mod changelog;
pub mod checkpoint;
pub mod detect;
pub mod git;
pub mod import;
pub mod orchestrate;
pub mod paths;
pub mod rewind;
pub mod settings;
pub mod skills;
pub mod store;
pub mod transcript;
pub mod types;
pub mod update;
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
