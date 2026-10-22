//! Search from the phone, answered by the sidebar's own search: titles as typed (the plain
//! substring test, and the full-text index's word prefixes), then messages, the best match in
//! each thread with an excerpt. Every thread the sidebar's search would list may come back
//! (settled long ago, a sub-agent), each with its whole row, so the phone can open it.

use crate::workspace::Workspace;
use gpui_kit::Context;
use std::collections::HashSet;
use trek_core::store::{SearchHit, now_ms};
use trek_remote as tr;

/// Threads a search returns when the phone doesn't say.
const DEFAULT_LIMIT: u32 = 50;

impl Workspace {
    pub(super) fn remote_search(
        &mut self,
        req: tr::SearchRequest,
        reply: tr::Reply<tr::SearchResults>,
        cx: &mut Context<Self>,
    ) {
        let query = req.query.trim().to_string();
        let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, tr::MAX_SEARCH) as usize;
        if trek_core::store::fts_query(&query).is_none() {
            return drop(reply.send(Ok(tr::SearchResults {
                query: req.query,
                results: vec![],
            })));
        }
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let q = query.clone();
            let hits = cx
                .background_executor()
                .spawn(async move { store.search(&q, limit) })
                .await;
            let answer = match hits {
                Ok(hits) => this
                    .read_with(cx, |ws, _| ws.search_results_for_phone(&query, hits, limit))
                    .map_err(|_| tr::HostError::other("Trek is closing")),
                Err(e) => Err(tr::HostError::other(format!("Couldn't search: {e:#}"))),
            };
            let _ = reply.send(answer.map(|results| tr::SearchResults {
                query: req.query,
                results,
            }));
        })
        .detach();
    }

    /// `hits` as the phone shows them: title matches first (the index's, best first, then the
    /// substring test's, newest first), then message matches, at most `limit` threads.
    fn search_results_for_phone(
        &self,
        query: &str,
        hits: Vec<SearchHit>,
        limit: usize,
    ) -> Vec<tr::SearchResult> {
        let now = now_ms();
        let shown = |id: &str| {
            self.thread(id)
                .filter(|t| t.own_section(now).is_some() && t.import_hidden.is_none())
        };
        let lower = query.to_lowercase();
        let (titles, messages): (Vec<SearchHit>, Vec<SearchHit>) =
            hits.into_iter().partition(|h| h.position.is_none());
        let mut by_substring: Vec<&trek_core::store::Thread> = self
            .threads
            .iter()
            .filter(|t| t.title.to_lowercase().contains(&lower))
            .collect();
        by_substring.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        let mut seen = HashSet::new();
        let mut out = vec![];
        let title_ids = titles
            .iter()
            .map(|h| h.thread_id.as_str())
            .chain(by_substring.iter().map(|t| t.id.as_str()));
        for id in title_ids {
            if let Some(t) = shown(id).filter(|_| out.len() < limit && seen.insert(id.to_string()))
            {
                out.push(tr::SearchResult {
                    thread: self.remote_summary(t, now),
                    snippet: None,
                    ranges: vec![],
                });
            }
        }
        for hit in messages {
            if let Some(t) = shown(&hit.thread_id)
                .filter(|_| out.len() < limit && seen.insert(hit.thread_id.clone()))
            {
                let ranges = char_ranges(&hit.snippet, &hit.ranges);
                out.push(tr::SearchResult {
                    thread: self.remote_summary(t, now),
                    snippet: Some(hit.snippet),
                    ranges,
                });
            }
        }
        out
    }
}

/// Byte ranges in `text` as character ranges (what the phone counts in).
pub(crate) fn char_ranges(text: &str, ranges: &[std::ops::Range<usize>]) -> Vec<[u32; 2]> {
    let at = |byte: usize| text.char_indices().take_while(|(i, _)| *i < byte).count() as u32;
    ranges
        .iter()
        .filter(|r| r.start < r.end && r.end <= text.len())
        .map(|r| [at(r.start), at(r.end)])
        .collect()
}
