//! # trek-remote
//!
//! The Mac side of Trek Mobile: a WebSocket server the iPhone app talks to. It pairs phones with
//! one-time codes, authenticates them with per-device tokens, answers their requests through a
//! [`RemoteHost`] and pushes changes to them. It knows nothing about GPUI or Trek's own types; the
//! protocol and its design live in `docs/MOBILE.md`.
//!
//! ```ignore
//! let config = ServerConfig { advertise: Some("192.168.1.20:7420".into()), ..ServerConfig::new(host_info) };
//! let handle = RemoteServer::start(config, Arc::new(my_host)).await?;
//! let offer = handle.pairing_offer();          // show offer.url as a QR code, offer.code as text
//! handle.push(HostEvent::Thread(summary));      // whenever something changes
//! ```
//!
//! ## Security
//!
//! - Every connection must send `pair` (with the code shown on the Mac) or `hello` (with its
//!   device token) first, within `auth_timeout`; anything else closes it.
//! - Upgrade requests with an `Origin` header (browsers) are refused with 403.
//! - Codes are single use, expire, and burn after 5 wrong attempts. Tokens are 32 random bytes;
//!   only their SHA-256 is stored, compared in constant time; revoking drops live connections.
//! - Messages are capped at `max_message` (1 MiB). One connection per device.
//! - The server never answers an approval, question or plan by itself and has no timeouts on them.
//!
//! ## Integrating with Trek
//!
//! trek-app implements [`RemoteHost`] (or uses [`ChannelHost`] and drains its
//! [`HostRequest`]s on the GPUI foreground executor, replying through each request's oneshot):
//!
//! - **`snapshot`**: every thread in the `Store` that the sidebar shows, as a [`ThreadSummary`]:
//!   `section` from the sidebar's `Section` (pinned/inbox/working/snoozed/settled; snoozed threads
//!   that raise their hand are reported as `inbox`), `run_state` from `RunState`, `needs` from the
//!   thread's pending approval/question/plan, failure or usage limit (one-line text), `unseen`,
//!   `pinned`, branch/worktree, current activity line, diff stat and `updated_at`. Projects with
//!   [`project_hue`] (the chosen colour or the name hash) and [`monogram`]; agents with
//!   `AgentId::key()`, display name, default model and model list.
//! - **`transcript`**: the thread's store items mapped to [`Item`]s. Item `id`s must be stable
//!   (Trek's item id), and `seq` must be a **per-thread monotonic counter kept by the host** (not
//!   the store's positional seq): every new item *and every update of an item* takes the next
//!   value, and `Transcript::seq` is the highest one handed out. Tool rows: [`ToolKind::from_title`],
//!   output cut with [`truncate_output`], `added`/`removed` from the file change's line counts.
//!   [`RemoteHost::transcript_for`] may serve a `subscribe`'s `limit` and `after_seq` itself, and
//!   [`RemoteHost::transcript_before`] pages of earlier items (both default to cutting the whole
//!   transcript); [`RemoteHost::unwatch`] says when no phone has a thread open any more.
//! - **`turn_action`**: undo, retry, fork or rewind, as the Mac's buttons do.
//! - **`send`**: the Workspace's send for an idle thread; for a working one, `SendMode::Steer`
//!   injects into the running turn and `SendMode::Queue` queues a follow-up; `None` follows the
//!   user's follow-up setting.
//! - **`new_thread`**: create a thread in the project with the agent (`AgentId` from its key) and
//!   model (or the project's/agent's default), in a worktree when asked and the project is a repo;
//!   send `text` as its first message; return the thread id (then push `HostEvent::Thread`).
//! - **`answer`**: `AnswerResponse::Approval` → `respond(Decision)` on the pending approval
//!   (allow / allow for session / deny); `Questions` and `Plan` → `Command::Answer` for the
//!   request. An already-answered request → `HostError::conflict`; unknown → `HostError::not_found`.
//! - **`interrupt`**: `Command::Interrupt` on the thread's session.
//! - **`mark_seen`**: clear the thread's unseen flag, as opening it on the desktop does.
//! - **Push** through [`RemoteHandle::push`]: `HostEvent::Thread` whenever a thread's row changes
//!   (state, title, needs, activity, diff stat), `ThreadRemoved` on delete/archive, `Item` for every
//!   new or changed transcript item (streaming text, tool finishing, a request resolving on the Mac:
//!   send the item with its new `state`), `TranscriptReset` after a rewind or rewrite, and a
//!   `Snapshot` when many things change at once (projects or agents added).
//! - **Settings › Mobile**: start the server with [`ServerConfig`] (`devices_path` in Trek's
//!   support folder, `advertise` = the LAN or tailnet address), show [`RemoteHandle::pairing_offer`]
//!   as a QR code, list [`RemoteHandle::devices`] with a Revoke button ([`RemoteHandle::revoke`]),
//!   toast [`ServerNotice`]s from [`RemoteHandle::notices`], and [`RemoteHandle::shutdown`] when
//!   the toggle goes off.

pub mod host;
pub mod pairing;
pub mod protocol;
pub mod server;
pub mod tls;

pub use host::{ChannelHost, HostError, HostEvent, HostRequest, HostResult, RemoteHost, Reply};
pub use pairing::{DeviceInfo, DeviceRegistry, PairingError, PairingOffer};
pub use protocol::*;
pub use server::{DEFAULT_PORT, RemoteHandle, RemoteServer, ServerConfig, ServerNotice};
pub use tls::TlsIdentity;
