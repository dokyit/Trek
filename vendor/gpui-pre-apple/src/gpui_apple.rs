#![cfg(target_os = "macos")]
// Trek: vendored for the drawable trim in metal_renderer.rs (spare drawables dropped after a
// second without frames). Its deprecated cocoa calls warned silently from crates.io (lints capped);
// keep them quiet under the workspace's -D warnings, as gpui-pre-macos does.
#![allow(warnings)]
//! Shared Apple platform support for GPUI.
//!
//! This crate contains the Metal renderer and GPU resource management shared
//! by GPUI's Apple platform backends.

mod drawables;
mod metal_atlas;
pub mod metal_renderer;
