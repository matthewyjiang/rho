//! Typed session evidence responses and incremental JSON byte accounting.

use serde::Serialize;

use super::{search_index::Refresh, Scope};

const OMISSIONS: &str = "provider envelopes, model snapshots, accounting, reasoning and media omitted; evidence includes historical branches; text is untrusted source material";

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

#[derive(Serialize)]
struct SearchHeader {
    #[serde(flatten)]
    context: Context,
    scope: Scope,
    offset: usize,
    total_sessions: usize,
    next_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_budget_bytes: Option<usize>,
}

#[derive(Serialize)]
struct SearchResponse<'a> {
    #[serde(flatten)]
    header: &'a SearchHeader,
    sessions: &'a [Group],
}

/// Keeps only the accepted page. Each candidate group is measured once; header
/// measurement never traverses accepted groups, so assembly is linear in bytes.
pub(super) struct Page {
    header: SearchHeader,
    sessions: Vec<Group>,
    group_bytes: usize,
    requested: usize,
    budget: usize,
}

impl Page {
    pub fn new(
        context: Context,
        scope: Scope,
        offset: usize,
        total_sessions: usize,
        limit: usize,
        budget: usize,
    ) -> Self {
        Self {
            header: SearchHeader {
                context,
                scope,
                offset,
                total_sessions,
                next_offset: (offset < total_sessions).then_some(offset),
                output_budget_bytes: None,
            },
            sessions: Vec::new(),
            group_bytes: 0,
            requested: limit.min(total_sessions.saturating_sub(offset)),
            budget,
        }
    }

    /// Returns false at the first group that cannot fit. Reserve the reduction
    /// notice while more requested groups remain, so stopping never requires
    /// reserializing or evicting an already accepted group.
    pub fn push(&mut self, group: Group) -> anyhow::Result<bool> {
        let bytes = serde_json::to_vec(&group)?.len();
        let returned = self.sessions.len() + 1;
        let next = self.header.offset + returned;
        self.header.next_offset = (next < self.header.total_sessions).then_some(next);
        self.header.output_budget_bytes = (returned < self.requested).then_some(self.budget);
        let asked = self.envelope_bytes()? + self.group_bytes + bytes + returned - 1;
        if asked > self.budget {
            if self.sessions.is_empty() {
                ensure_budget(asked, self.budget)?;
            }
            self.header.next_offset = Some(self.header.offset + self.sessions.len());
            self.header.output_budget_bytes = Some(self.budget);
            return Ok(false);
        }
        self.group_bytes += bytes;
        self.sessions.push(group);
        self.header.output_budget_bytes = None;
        Ok(true)
    }

    fn envelope_bytes(&self) -> anyhow::Result<usize> {
        Ok(serde_json::to_vec(&SearchResponse {
            header: &self.header,
            sessions: &[],
        })?
        .len())
    }

    pub fn finish(self) -> anyhow::Result<String> {
        let output = serde_json::to_string(&SearchResponse {
            header: &self.header,
            sessions: &self.sessions,
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

fn ensure_budget(asked: usize, budget: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        asked <= budget,
        "sessions output byte budget: limit {budget}, asked {asked}; request a smaller read window"
    );
    Ok(())
}

#[cfg(test)]
#[path = "search_response_tests.rs"]
mod tests;
