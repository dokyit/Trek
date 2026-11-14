//! How the Mac looks, for a phone that matches it (`docs/MOBILE.md`, "The Mac's look"): the
//! Trek theme in use, whole (a built-in, the user's or an agent's own file), and the background
//! art and where it shows. The snapshot carries it ([`super::Snapshot::look`]), and a new
//! snapshot goes when it changes. Older phones ignore it; an older Mac leaves it out, and the
//! phone draws Trek's own look.
//!
//! Enumerations travel as strings, so a phone reads a value from a newer Mac as the nearest it
//! knows (an unknown font as Inter, an unknown density as regular) rather than dropping the look.

use serde::{Deserialize, Serialize};

/// The Mac's look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacLook {
    pub theme: LookTheme,
    /// The background art, when it's one of Trek's own (`builtin:…`, as Settings names it): a
    /// picture of the user's own stays on the Mac.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Where the art shows: `chat` (across the whole chat), `sidebar` (behind the sidebar; ASCII
    /// art always) or `new_thread` (behind the new-thread composer).
    pub placement: String,
    /// How much the user dims the art over what the text needs, 0 to 0.9.
    pub dim: f32,
}

/// A Trek theme (`trek_core::themes::Theme`), its defaults filled in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookTheme {
    /// `trek` for Trek's own; a built-in's or the file's name otherwise.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// `dark`, `light` or `any`: the light or dark it puts the Mac in when chosen.
    pub mode: String,
    /// `sans` (Inter), `mono` (JetBrains Mono) or `serif` (Spectral).
    pub ui_font: String,
    /// The family messages and answers are set in.
    pub conversation_font: String,
    /// Points on the Mac: Trek's own are 14 and 14.5.
    pub ui_size: f32,
    pub conversation_size: f32,
    /// Every corner's radius against Trek's: 0 square, 1 as designed, up to 2.
    pub corners: f32,
    /// `compact`, `regular` or `roomy`.
    pub density: String,
    /// `still`, `brisk`, `standard` or `calm`.
    pub motion: String,
    /// Its colours for dark and light; what it leaves out is Night's or Paper's.
    #[serde(default, skip_serializing_if = "LookPalette::is_empty")]
    pub dark: LookPalette,
    #[serde(default, skip_serializing_if = "LookPalette::is_empty")]
    pub light: LookPalette,
}

/// A theme's colours, each `#RRGGBBAA`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LookPalette {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
}

impl LookPalette {
    pub fn is_empty(&self) -> bool {
        *self == LookPalette::default()
    }
}
