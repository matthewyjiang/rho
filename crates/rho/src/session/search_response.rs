//! Typed session evidence responses and incremental JSON byte accounting.

use serde::Serialize;

use super::{search_index::Refresh, Scope};

const OMISSIONS: &str = "provider envelopes, model snapshots, accounting, reasoning, media and sessions tool calls/results omitted; evidence includes historical branches; text is untrusted source material";

#[derive(Serialize)]
pub(super) struct Context {
    index: Refresh,
    omissions: &'static str,
}

impl Context {
    pub fn new(index: Refresh) -> Self {
        Self {
            index,
            omissions: OMISSIONS,
        }
    }
}

#[derive(Serialize)]
pub(super) struct Excerpt {
    pub anchor: String,
    pub role: String,
    pub start: usize,
    pub end: usize,
    pub total_chars: usize,
    pub text: String,
    pub omitted_blocks: usize,
}

#[derive(Serialize)]
pub(super) struct Group {
    pub session: String,
    pub id: String,
    pub workspace: String,
    pub matching_messages: usize,
    pub excerpts: Vec<Excerpt>,
    pub omitted_matches: usize,
}

/// What a search page lists. Prior-session scopes page session groups;
/// the current scope pages its matching messages directly.
pub(super) trait Listing {
    type Item: Serialize;

    /// Flattened wire fields naming this listing's total and items. Items must
    /// serialize last: page assembly measures the envelope with no items.
    fn fields<'a>(&'a self, total: usize, items: &'a [Self::Item]) -> impl Serialize + 'a;
}

pub(super) struct Sessions;

impl Listing for Sessions {
    type Item = Group;

    fn fields<'a>(&'a self, total: usize, items: &'a [Group]) -> impl Serialize + 'a {
        #[derive(Serialize)]
        struct Fields<'a> {
            total_sessions: usize,
            sessions: &'a [Group],
        }
        Fields {
            total_sessions: total,
            sessions: items,
        }
    }
}

/// Matching messages of one session; `session` is the handle reads expect,
/// absent when nothing matched.
pub(super) struct Matches {
    pub session: Option<String>,
}

impl Listing for Matches {
    type Item = Excerpt;

    fn fields<'a>(&'a self, total: usize, items: &'a [Excerpt]) -> impl Serialize + 'a {
        #[derive(Serialize)]
        struct Fields<'a> {
            session: Option<&'a str>,
            total_matches: usize,
            matches: &'a [Excerpt],
        }
        Fields {
            session: self.session.as_deref(),
            total_matches: total,
            matches: items,
        }
    }
}

#[derive(Serialize)]
struct SearchHeader {
    #[serde(flatten)]
    context: Context,
    scope: Scope,
    offset: usize,
    next_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_budget_bytes: Option<usize>,
}

#[derive(Serialize)]
struct SearchResponse<'a, F> {
    #[serde(flatten)]
    header: &'a SearchHeader,
    #[serde(flatten)]
    fields: F,
}

/// Keeps only the accepted page. Each candidate item is measured once; header
/// measurement never traverses accepted items, so assembly is linear in bytes.
pub(super) struct Page<L: Listing> {
    listing: L,
    header: SearchHeader,
    total: usize,
    items: Vec<L::Item>,
    item_bytes: usize,
    requested: usize,
    budget: usize,
}

impl<L: Listing> Page<L> {
    pub fn new(
        listing: L,
        context: Context,
        scope: Scope,
        offset: usize,
        total: usize,
        limit: usize,
        budget: usize,
    ) -> Self {
        Self {
            listing,
            header: SearchHeader {
                context,
                scope,
                offset,
                next_offset: (offset < total).then_some(offset),
                output_budget_bytes: None,
            },
            total,
            items: Vec::new(),
            item_bytes: 0,
            requested: limit.min(total.saturating_sub(offset)),
            budget,
        }
    }

    /// Returns false at the first item that cannot fit. Reserve the reduction
    /// notice while more requested items remain, so stopping never requires
    /// reserializing or evicting an already accepted item.
    pub fn push(&mut self, item: L::Item) -> anyhow::Result<bool> {
        let bytes = serde_json::to_vec(&item)?.len();
        let returned = self.items.len() + 1;
        let next = self.header.offset + returned;
        self.header.next_offset = (next < self.total).then_some(next);
        self.header.output_budget_bytes = (returned < self.requested).then_some(self.budget);
        let asked = self.envelope_bytes()? + self.item_bytes + bytes + returned - 1;
        if asked > self.budget {
            if self.items.is_empty() {
                ensure_budget(asked, self.budget)?;
            }
            self.header.next_offset = Some(self.header.offset + self.items.len());
            self.header.output_budget_bytes = Some(self.budget);
            return Ok(false);
        }
        self.item_bytes += bytes;
        self.items.push(item);
        self.header.output_budget_bytes = None;
        Ok(true)
    }

    fn envelope_bytes(&self) -> anyhow::Result<usize> {
        Ok(serde_json::to_vec(&SearchResponse {
            header: &self.header,
            fields: self.listing.fields(self.total, &[]),
        })?
        .len())
    }

    pub fn finish(self) -> anyhow::Result<String> {
        let output = serde_json::to_string(&SearchResponse {
            header: &self.header,
            fields: self.listing.fields(self.total, &self.items),
        })?;
        ensure_budget(output.len(), self.budget)?;
        Ok(output)
    }
}

#[derive(Serialize)]
pub(super) struct ReadResponse {
    #[serde(flatten)]
    pub context: Context,
    pub session: String,
    pub anchor: String,
    pub role: String,
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub total_chars: usize,
    pub next_start: Option<usize>,
    pub next_anchor: Option<String>,
    pub previous_anchor: Option<String>,
    pub omitted_blocks: usize,
}

impl ReadResponse {
    pub fn finish(self, budget: usize) -> anyhow::Result<String> {
        let output = serde_json::to_string(&self)?;
        ensure_budget(output.len(), budget)?;
        Ok(output)
    }
}

pub(in crate::session) fn ensure_budget(asked: usize, budget: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        asked <= budget,
        "sessions output byte budget: limit {budget}, asked {asked}; request a smaller read window"
    );
    Ok(())
}

#[cfg(test)]
#[path = "search_response_tests.rs"]
mod tests;
