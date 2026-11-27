//! The start of a long thread on another machine. A thread opens with its last
//! [`super::OPEN_LIMIT`] items, as on a phone; the ones before them are read from the machine a
//! page at a time (`transcript_before`) once it's open, so the whole conversation can be
//! scrolled, searched and rewound from here. Until 0.4.67 they never came: a long thread began
//! in the middle, with no way to see how it started.

use super::{Workspace, local_id};
use gpui_kit::Context;
use trek_remote as tr;
use trek_remote::client::ClientError;

/// The most items of one thread kept here. Past it the thread begins where the pages stopped.
const MOST: usize = 12_000;

/// The item a page is asked for before: the first of `items` the transcript holds for good (an
/// item `i…` or a turn's changed files `c…`; open requests `r…` aren't places in it).
pub(super) fn first_place(items: &[tr::Item]) -> Option<&str> {
    items
        .iter()
        .map(|i| i.id.as_str())
        .find(|id| id.starts_with('i') || id.starts_with('c'))
}

/// What a page's answer came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    /// Its items are in front now: shown, and the page before them asked for if there's more.
    Added,
    /// It's of a transcript that has since been replaced (`before` is no longer where `items`
    /// starts): nothing of it is used, and what the new transcript says of earlier items
    /// stands. They're asked for again from where it starts.
    Stale,
    /// It added nothing, or couldn't be read: nothing more is asked for (a reconnect's fresh
    /// transcript asks again).
    Over,
}

/// `page` (the items just before `before`) put in front of `items`. Nothing is added when
/// `before` is no longer where `items` starts (the transcript was replaced while the page was
/// on its way), and an item already there isn't added again.
pub(super) fn prepend(items: &mut Vec<tr::Item>, before: &str, page: Vec<tr::Item>) -> Page {
    if first_place(items) != Some(before) {
        return Page::Stale;
    }
    let mut all: Vec<tr::Item> = page
        .into_iter()
        .filter(|p| !items.iter().any(|i| i.id == p.id))
        .collect();
    if all.is_empty() {
        return Page::Over;
    }
    all.append(items);
    *items = all;
    Page::Added
}

/// Take the answer to the page asked for before `before` (`None`: it couldn't be read) into a
/// followed transcript: its `items`, and whether earlier ones are still on the machine (`more`).
/// An answer about a transcript that was replaced meanwhile leaves both as the new one has them:
/// it used to switch `more` off, and the thread's start then never came.
pub(super) fn take_page(
    items: &mut Vec<tr::Item>,
    more: &mut bool,
    before: &str,
    page: Option<(Vec<tr::Item>, bool)>,
) -> Page {
    if first_place(items) != Some(before) {
        return Page::Stale;
    }
    let took = match page {
        Some((page, earlier)) => match prepend(items, before, page) {
            Page::Added => {
                *more = earlier;
                return Page::Added;
            }
            other => other,
        },
        None => Page::Over,
    };
    // A page that adds nothing would be asked for again for ever.
    *more = false;
    took
}

impl Workspace {
    /// Read the page before what's here of thread `tid` of machine `host`, when the machine left
    /// earlier items out and none is being read already.
    pub(super) fn machine_read_earlier(&mut self, host: &str, tid: &str, cx: &mut Context<Self>) {
        let Some(m) = self.machines.get_mut(host) else {
            return;
        };
        let Some(client) = m.client.clone() else {
            return;
        };
        let Some(f) = m.followed.get_mut(tid) else {
            return;
        };
        if !f.more || f.paging.is_some() || f.items.len() >= MOST {
            return;
        }
        let Some(before) = first_place(&f.items).map(str::to_string) else {
            return;
        };
        let (host, tid) = (host.to_string(), tid.to_string());
        let req = tr::ClientMessage::TranscriptBefore {
            thread_id: tid.clone(),
            before: before.clone(),
            limit: tr::MAX_PAGE,
        };
        f.paging = Some(cx.spawn(async move |this, cx| {
            let reply = client.request(req).await;
            let _ = this.update(cx, |ws, cx| {
                ws.machine_earlier_page(&host, &tid, &before, reply, cx)
            });
        }));
    }

    fn machine_earlier_page(
        &mut self,
        host: &str,
        tid: &str,
        before: &str,
        reply: Result<tr::ServerMessage, ClientError>,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self
            .machines
            .get_mut(host)
            .and_then(|m| m.followed.get_mut(tid))
        else {
            return;
        };
        f.paging = None;
        let page = match reply {
            Ok(tr::ServerMessage::TranscriptPage { items, more, .. }) => Some((items, more)),
            // It can't page (a Trek before `transcript_before`).
            Ok(_) => None,
            Err(e) => {
                tracing::info!(
                    thread = local_id(host, tid),
                    "couldn't read the earlier part of a machine's thread: {e}"
                );
                None
            }
        };
        match take_page(&mut f.items, &mut f.more, before, page) {
            Page::Added => {
                self.machine_live(host, tid, cx);
                self.machine_read_earlier(host, tid, cx);
            }
            Page::Stale => self.machine_read_earlier(host, tid, cx),
            Page::Over => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> tr::Item {
        tr::Item {
            id: id.into(),
            seq: 0,
            at: None,
            body: tr::ItemBody::Notice { text: id.into() },
        }
    }

    #[test]
    fn an_earlier_page_goes_in_front_once_and_only_where_the_transcript_still_starts() {
        let mut items = vec![item("r9"), item("i300"), item("i301")];
        assert_eq!(first_place(&items), Some("i300"), "a request is no place");
        assert_eq!(
            prepend(&mut items, "i300", vec![item("i298"), item("i299")]),
            Page::Added
        );
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["i298", "i299", "r9", "i300", "i301"]);
        // The same page again (the answer to a request made twice) adds nothing.
        assert_eq!(prepend(&mut items, "i298", vec![item("i298")]), Page::Over);
        // A page for where the transcript no longer starts (it was replaced meanwhile) is dropped.
        assert_eq!(prepend(&mut items, "i300", vec![item("i100")]), Page::Stale);
        assert_eq!(items.len(), 5);
    }

    /// What a review of the first cut found: a page's answer that landed after the transcript
    /// was sent again (a reconnect) failed to go in front, and then set `more` to false, over
    /// what the new transcript had just said. The thread's start never loaded. Such an answer
    /// now changes nothing, and says to ask again from where the new transcript starts.
    #[test]
    fn a_page_of_a_transcript_since_replaced_leaves_what_the_new_one_says_of_earlier_items() {
        // Asked for before i800; meanwhile the transcript came again, starting at i850.
        let mut items = vec![item("i850"), item("i851")];
        let mut more = true;
        let late = Some((vec![item("i798"), item("i799")], false));
        assert_eq!(take_page(&mut items, &mut more, "i800", late), Page::Stale);
        assert!(more, "the new transcript's word stands");
        assert_eq!(items.len(), 2, "nothing of the old page is used");
        // So does a failed read of the old one.
        assert_eq!(take_page(&mut items, &mut more, "i800", None), Page::Stale);
        assert!(more);
        // Its own page goes in, with what that page says of still earlier ones.
        let own = Some((vec![item("i848"), item("i849")], true));
        assert_eq!(take_page(&mut items, &mut more, "i850", own), Page::Added);
        assert!(more);
        assert_eq!(first_place(&items), Some("i848"));
        // A page that adds nothing, or can't be read, ends the asking.
        assert_eq!(
            take_page(&mut items, &mut more, "i848", Some((vec![], true))),
            Page::Over
        );
        assert!(!more);
        more = true;
        assert_eq!(take_page(&mut items, &mut more, "i848", None), Page::Over);
        assert!(!more);
    }
}
