//! The editor's AI side bar. `pane` owns the header (chat tabs, new chat, history, more) and
//! lays out the rest: the compact transcript (`transcript`, with its `cards`), the bar of
//! changes pending review (`review`) and the input (`input`, with its `context` chips).

mod cards;
mod commands;
pub mod context;
mod input;
mod pane;
mod restore;
mod review;
mod transcript;

pub use input::UndoAllOrStop;
#[cfg(test)]
pub use input::AiInput;
pub use pane::AiPane;
