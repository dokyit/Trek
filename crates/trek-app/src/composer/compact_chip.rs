//! The Compact chip: back at a large thread after the provider's prompt cache has gone, the next
//! message re-reads everything at full price, and compacting first makes that smaller. It says
//! why in its tooltip, one click compacts, and it goes once used or dismissed. It stays away from
//! small threads (`workspace::compact_offer`).

use super::Composer;
use crate::fluent::{self, PillExt as _};
use crate::scale::text;
use crate::workspace::compact_offer::CompactOffer;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex};
use gpui_kit::*;
use std::time::Duration;

impl Composer {
    /// The offer standing for `id`: the thread is large, its cache has gone, and the user hasn't
    /// dismissed it since the thread was last active.
    pub(super) fn compact_chip_offer(&self, id: &str, cx: &App) -> Option<CompactOffer> {
        let ws = self.workspace.read(cx);
        ws.compact_offer(id, &self.scope)
            .filter(|o| o.lapsed(ws.now()))
            .filter(|o| self.compact_dismissed.get(id) != Some(&o.last_active))
    }

    /// Nothing else may redraw the composer as the cache's life ends on an idle thread, so it asks
    /// for a redraw then.
    pub(super) fn redraw_when_cache_lapses(&mut self, cx: &mut Context<Self>) {
        let (at, now) = {
            let ws = self.workspace.read(cx);
            let at = ws
                .thread_id_in(&self.scope)
                .and_then(|id| ws.compact_offer(id, &self.scope))
                .map(|o| o.lapses_at());
            (at, ws.now())
        };
        let Some(at) = at.filter(|at| *at > now) else {
            self._cache_lapse = None;
            return;
        };
        if self._cache_lapse.as_ref().is_some_and(|(t, _)| *t == at) {
            return;
        }
        let wait = Duration::from_millis((at - now) as u64 + 50);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        });
        self._cache_lapse = Some((at, task));
    }

    /// "Compact", and a ✕ to dismiss it by.
    pub(super) fn compact_chip(
        &self,
        id: &str,
        offer: &CompactOffer,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let why = SharedString::from(offer.why());
        let thread = id.to_string();
        let at = offer.last_active;
        h_flex()
            .id("compact-chip")
            .test_support()
            .aria_label("Compact the conversation")
            .flex_none()
            .h(px(31.))
            .pl(px(12.))
            .pr(px(6.))
            // The icon and the word spaced as the changes and PR chips beside it space theirs.
            .gap(px(6.))
            .items_center()
            .rounded_pill()
            .bg(crate::pr_card::chip_fill(cx))
            .border_1()
            .border_color(crate::pr_card::chip_border(cx))
            .shadow(fluent::card_shadow(crate::pr_card::chip_shadow()))
            .cursor_pointer()
            .hover(|s| s.border_color(theme.foreground.opacity(0.16)))
            .text_size(px(text::BODY))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(why.clone()).build(window, cx)
            })
            .child(
                Icon::new(crate::assets::Lucide::ChevronsDownUp)
                    .size(px(12.))
                    .text_color(theme.muted_foreground),
            )
            .child(div().text_color(theme.foreground).child("Compact"))
            .child(
                div()
                    .id("compact-chip-dismiss")
                    .test_support()
                    .aria_label("Dismiss")
                    .flex_none()
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .hover(|s| s.bg(theme.foreground.opacity(0.08)))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(10.))
                            .text_color(theme.muted_foreground.opacity(0.7)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.compact_dismissed.insert(thread.clone(), at);
                        cx.notify();
                    })),
            )
            .on_click(cx.listener(|this, _, _, cx| this.compact_now(cx)))
            .into_any_element()
    }
}
