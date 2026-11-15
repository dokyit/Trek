//! `trek:commit-trail`: a project's year of commits drawn as a climb. Each week is a point of a
//! ridge as high as that week's commits, laid as three bands like a contour map's (the lowest the
//! strongest), with a dotted trail along the top, a flag on the busiest week and the hiker
//! standing at today.
//!
//! The year is read with git off the UI thread (`trek_core::plugins::data::commit_year`), the
//! first time a project's trail is drawn and again when it's ten minutes old, and kept: drawing
//! reads only what's kept.

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use trek_core::plugins::data::{CommitYear, WEEKS};

/// How long a year read stands before it's read again.
const FRESH: Duration = Duration::from_secs(600);
/// The climb's height.
const HEIGHT: f32 = 68.;

/// Every project's year as last read, and the reads asked for.
pub struct Trails {
    kept: HashMap<(PathBuf, bool), (Option<CommitYear>, Instant)>,
    asked: RefCell<HashSet<(PathBuf, bool)>>,
    tx: async_channel::Sender<(PathBuf, bool)>,
}

impl Global for Trails {}

/// Hear read requests, read off the UI thread, keep what comes back. Made with the plugins.
pub fn init(cx: &mut App) {
    let (tx, rx) = async_channel::unbounded::<(PathBuf, bool)>();
    cx.set_global(Trails {
        kept: HashMap::new(),
        asked: RefCell::new(HashSet::new()),
        tx,
    });
    cx.spawn(async move |cx| {
        while let Ok((dir, mine)) = rx.recv().await {
            let read = {
                let dir = dir.clone();
                cx.background_spawn(async move {
                    let now = chrono::Utc::now().timestamp();
                    trek_core::plugins::data::commit_year(&dir, mine, now)
                })
                .await
            };
            cx.update(|cx| {
                let t = cx.global_mut::<Trails>();
                t.asked.borrow_mut().remove(&(dir.clone(), mine));
                t.kept.insert((dir, mine), (read, Instant::now()));
                cx.refresh_windows();
            });
        }
    })
    .detach();
}

/// `project`'s year as kept, asking for a read when there's none or it's old.
fn year(project: &Path, mine: bool, cx: &App) -> Option<Option<CommitYear>> {
    let t = cx.try_global::<Trails>()?;
    let key = (project.to_path_buf(), mine);
    let kept = t.kept.get(&key);
    let stale = kept.is_none_or(|(_, at)| at.elapsed() > FRESH);
    if stale && t.asked.borrow_mut().insert(key.clone()) {
        let _ = t.tx.try_send(key);
    }
    kept.map(|(y, _)| y.clone())
}

fn month(week_start: i64) -> &'static str {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    chrono::DateTime::from_timestamp(week_start, 0)
        .map(|d| MONTHS[chrono::Datelike::month0(&d) as usize])
        .unwrap_or("")
}

fn day(week_start: i64) -> String {
    chrono::DateTime::from_timestamp(week_start, 0)
        .map(|d| d.format("%b %-d").to_string())
        .unwrap_or_default()
}

/// The ridge's heights, 0 to 1, a little smoothed so it reads as ground and not as bars.
fn ridge(weeks: &[u32]) -> Vec<f32> {
    let max = weeks.iter().copied().max().unwrap_or(0).max(1) as f32;
    let raw: Vec<f32> = weeks.iter().map(|w| *w as f32 / max).collect();
    (0..raw.len())
        .map(|i| {
            let a = raw[i.saturating_sub(1)];
            let b = raw[i];
            let c = raw[(i + 1).min(raw.len() - 1)];
            (a + 2. * b + c) / 4.
        })
        .collect()
}

/// The trail for `project`'s year: its count, the climb, and the months under it.
pub fn element(project: &Path, mine: bool, dots_only: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    // Not read yet, or not a repository: nothing to draw (yet).
    let Some(Some(year)) = year(project, mine, cx) else {
        return div().into_any_element();
    };
    let count = trek_core::plugins::ui::Template::parse("{n|plural:commit}")
        .map(|t| t.render(&|_| Some(serde_json::json!(year.total))))
        .unwrap_or_default();
    let said = if year.mine {
        count.replacen(' ', " of your ", 1)
    } else {
        count
    };
    let busiest = year
        .weeks
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| **n)
        .filter(|(_, n)| **n > 0)
        .map(|(i, n)| (i, *n));
    let header = h_flex()
        .w_full()
        .justify_between()
        .text_size(px(crate::scale::text::LABEL))
        .child(
            h_flex()
                .gap(px(5.))
                .child(
                    div()
                        .text_color(theme.foreground)
                        .font_weight(FontWeight::MEDIUM)
                        .child(said),
                )
                .child(div().text_color(muted).child("in the last year")),
        )
        .when_some(busiest, |el, (i, n)| {
            el.child(div().text_color(muted).child(format!(
                "highest week {} · {n}",
                day(year.start + i as i64 * 7 * 86_400)
            )))
        });
    // A project younger than the year: the trail starts a week before its first commit rather
    // than twelve months back, so the climb fills the width (eight weeks at least).
    let from = year
        .weeks
        .iter()
        .position(|n| *n > 0)
        .map_or(0, |first| first.saturating_sub(1).min(WEEKS - 8));
    let span = WEEKS - from;
    let heights = ridge(&year.weeks[from..]);
    let accent = theme.primary;
    let ink = theme.foreground;
    let dark = theme.mode.is_dark();
    let peak = busiest.map(|(i, _)| i.saturating_sub(from));
    let climb = div().w_full().h(px(HEIGHT)).child(
        canvas(
            |_, _, _| {},
            move |b, _, window, _| paint(b, &heights, peak, dots_only, accent, ink, dark, window),
        )
        .size_full(),
    );
    // A month's name under the first week that starts in it, every other month when there's
    // little room for them.
    let mut months = Vec::new();
    let mut last = "";
    for w in from..WEEKS {
        let m = month(year.start + w as i64 * 7 * 86_400);
        if m != last {
            // Not the first week (a month already under way), nor so near the end that its name
            // would run past the edge.
            if w > from && w < WEEKS - 2 {
                months.push((w, m));
            }
            last = m;
        }
    }
    let labels = div()
        .relative()
        .w_full()
        .h(px(14.))
        .children(months.into_iter().map(|(w, m)| {
            div()
                .absolute()
                .left(relative((w - from) as f32 / (span - 1) as f32))
                .text_size(px(crate::scale::text::META))
                .text_color(muted.opacity(0.75))
                .child(m)
        }));
    v_flex()
        .id("commit-trail")
        .test_support()
        .w_full()
        .gap(px(8.))
        .child(header)
        .child(climb)
        .child(labels)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn paint(
    b: Bounds<Pixels>,
    heights: &[f32],
    peak: Option<usize>,
    dots_only: bool,
    accent: Hsla,
    ink: Hsla,
    dark: bool,
    window: &mut Window,
) {
    let n = heights.len();
    if n < 2 {
        return;
    }
    let scale = window.scale_factor();
    let snap = |v: f32| trek_mascot::snap(v, scale);
    let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
    let (w, h) = (b.size.width.as_f32(), b.size.height.as_f32());
    // Room at the top for the flag, and at the right for the hiker.
    let cell = trek_mascot::cell_size(scale);
    let (sprite_w, sprite_h) = trek_mascot::SPRITE;
    let right = sprite_w as f32 * cell * 0.5 + 2.;
    let top = 12.;
    let ground = y0 + h - 1.;
    let x_of = |i: usize| x0 + (w - right) * i as f32 / (n - 1) as f32;
    let y_of = |v: f32, k: f32| ground - v * k * (h - top);
    if !dots_only {
        // Three bands, the full ridge faintest and the lowest strongest, as contours stack.
        for (k, alpha) in [(1.0, 0.10), (0.68, 0.16), (0.38, 0.26)] {
            let mut path = PathBuilder::fill();
            path.move_to(point(px(x_of(0)), px(ground)));
            for (i, v) in heights.iter().enumerate() {
                path.line_to(point(px(x_of(i)), px(y_of(*v, k))));
            }
            path.line_to(point(px(x_of(n - 1)), px(ground)));
            path.close();
            if let Ok(p) = path.build() {
                window.paint_path(p, accent.opacity(alpha));
            }
        }
    }
    // The trail along the ridge: dots a step apart, along the line between weeks.
    // scale: ok, a trail's dot, a whole number of device pixels across
    let dot = ((2. * scale).round() / scale).max(1. / scale);
    let step = if dots_only { 4. } else { 5. };
    let mut carry = 0.;
    for i in 0..n - 1 {
        let (ax, ay) = (x_of(i), y_of(heights[i], 1.));
        let (bx, by) = (x_of(i + 1), y_of(heights[i + 1], 1.));
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let mut s = carry;
        while s < len {
            let t = s / len.max(1e-3);
            let (x, y) = (ax + (bx - ax) * t, ay + (by - ay) * t);
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(px(snap(x - dot / 2.)), px(snap(y - dot / 2.))),
                        size(px(dot), px(dot)),
                    ),
                    accent.opacity(0.9),
                )
                // scale: ok, a trail's dot is round
                .corner_radii(px(dot / 2.)),
            );
            s += step;
        }
        carry = s - len;
    }
    // The ground line, faint.
    window.paint_quad(fill(
        Bounds::new(point(px(x0), px(snap(ground))), size(px(w), px(1. / scale))),
        ink.opacity(0.1),
    ));
    // A flag on the busiest week: a pole and a pennant.
    if let Some(p) = peak {
        let (fx, fy) = (snap(x_of(p)), snap(y_of(heights[p], 1.) - 1.));
        let pole = 9.;
        window.paint_quad(fill(
            Bounds::new(
                point(px(fx), px(fy - pole)),
                size(px((1.5 * scale).round() / scale), px(pole)),
            ),
            ink.opacity(0.7),
        ));
        let mut pennant = PathBuilder::fill();
        pennant.move_to(point(px(fx + 1.), px(fy - pole)));
        pennant.line_to(point(px(fx + 7.), px(fy - pole + 2.5)));
        pennant.line_to(point(px(fx + 1.), px(fy - pole + 5.)));
        pennant.close();
        if let Ok(path) = pennant.build() {
            window.paint_path(path, accent);
        }
    }
    // The hiker at today, standing on the ridge's end.
    let last = n - 1;
    let feet = point(px(x_of(last)), px(y_of(heights[last], 1.) - 1.));
    let _ = sprite_h;
    trek_mascot::paint_turned(feet, cell, trek_mascot::STAND, 0, dark, window);
}
