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

/// `page` (the items just before `before`) put in front of `items`. Nothing is added when
/// `before` is no longer where `items` starts (the transcript was replaced while the page was
/// on its way), and an item already there isn't added again.
pub(super) fn prepend(items: &mut Vec<tr::Item>, before: &str, page: Vec<tr::Item>) -> bool {
    if first_place(items) != Some(before) {
        return false;
    }
    let mut all: Vec<tr::Item> = page
        .into_iter()
        .filter(|p| !items.iter().any(|i| i.id == p.id))
        .collect();
    if all.is_empty() {
        return false;
    }
    all.append(items);
    *items = all;
    true
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
        let added = match reply {
            Ok(tr::ServerMessage::TranscriptPage { items, more, .. }) => {
                let added = prepend(&mut f.items, before, items);
                // A page that adds nothing would be asked for again for ever.
                f.more = more && added;
                added
            }
            // It can't page (a Trek before `transcript_before`), or it dropped: what's here stays,
            // and a reconnect's fresh transcript asks again.
            Ok(_) => {
                f.more = false;
                false
            }
            Err(e) => {
                tracing::info!(
                    thread = local_id(host, tid),
                    "couldn't read the earlier part of a machine's thread: {e}"
                );
                f.more = false;
                false
            }
        };
        if added {
            self.machine_live(host, tid, cx);
            self.machine_read_earlier(host, tid, cx);
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
        assert!(prepend(
            &mut items,
            "i300",
            vec![item("i298"), item("i299")]
        ));
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["i298", "i299", "r9", "i300", "i301"]);
        // The same page again (the answer to a request made twice) adds nothing.
        assert!(!prepend(&mut items, "i298", vec![item("i298")]));
        // A page for where the transcript no longer starts (it was replaced meanwhile) is dropped.
        assert!(!prepend(&mut items, "i300", vec![item("i100")]));
        assert_eq!(items.len(), 5);
    }
}
