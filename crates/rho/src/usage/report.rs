//! Read-side summary of the usage ledger for `/spend`.
//!
//! [`load_spend_reports`] reads the ledger once in read-only mode, prices each
//! request, and folds it straight into a [`SpendReport`] for every
//! [`SpendRange`], so memory grows with the number of models and providers,
//! not requests.
//!
//! A request's dollar value is its actual cost (as the provider reported it)
//! when present, else a cost computed from catalog prices as of today. Local
//! models count as priced at zero. Anything else is unpriced and stays
//! visible in [`SpendTotals`] rather than being guessed.

use std::{
    collections::{BTreeMap, HashMap},
    ops::AddAssign,
    path::Path,
    sync::Arc,
};

use chrono::{Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use rho_providers::model::{ModelMetadata, ModelUsage};
use rusqlite::{Connection, OpenFlags};

use super::{migrations::SCHEMA_VERSION, pricing::catalog_cost_usd_micros, UsageLedgerError};
use crate::sqlite_support::BUSY_TIMEOUT;

/// Time window a [`SpendReport`] covers. Windows other than
/// [`Self::AllTime`] start at local midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpendRange {
    Today,
    /// Today and the 6 days before it.
    Last7Days,
    /// Today and the 29 days before it.
    Last30Days,
    AllTime,
}

impl SpendRange {
    /// Display order: most recent first.
    pub(crate) const ALL: [Self; 4] = [
        Self::Today,
        Self::Last7Days,
        Self::Last30Days,
        Self::AllTime,
    ];

    fn index(self) -> usize {
        match self {
            Self::Today => 0,
            Self::Last7Days => 1,
            Self::Last30Days => 2,
            Self::AllTime => 3,
        }
    }

    /// The range `step` places away in [`Self::ALL`], wrapping at both ends.
    pub(crate) fn cycled(self, step: isize) -> Self {
        let len = Self::ALL.len() as isize;
        Self::ALL[(self.index() as isize + step).rem_euclid(len) as usize]
    }

    /// First local day of a fixed window; `None` for all time.
    fn first_day(self, today: NaiveDate) -> Option<NaiveDate> {
        match self {
            Self::AllTime => None,
            Self::Last30Days => Some(today - Duration::days(29)),
            Self::Last7Days => Some(today - Duration::days(6)),
            Self::Today => Some(today),
        }
    }

    fn timeline_unit(self) -> TimelineUnit {
        match self {
            Self::Today => TimelineUnit::Hour,
            Self::AllTime | Self::Last30Days | Self::Last7Days => TimelineUnit::Day,
        }
    }
}

/// How requests on one provider route are priced when the provider reported
/// no cost.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RoutePricing {
    /// Runs on this machine; priced at zero rather than left unpriced.
    Local,
    /// Catalog metadata carrying a default price.
    Catalog(Arc<ModelMetadata>),
    Unknown,
}

/// Production pricing: built-in Ollama is local; everything else uses the
/// cached models.dev row the statusline uses, including a custom host's
/// configured catalog lookup. Stale rows are fine for an estimate.
pub(crate) fn catalog_route_pricing(provider: &str, model: &str) -> RoutePricing {
    use rho_providers::{
        model::models_dev::cached_model_metadata,
        provider::{provider_descriptor, ProviderId},
    };

    if provider_descriptor(provider).is_some_and(|descriptor| descriptor.id == ProviderId::Ollama) {
        return RoutePricing::Local;
    }
    match cached_model_metadata(provider, model) {
        Some(metadata) if metadata.cost_default.is_some() => {
            RoutePricing::Catalog(Arc::new(metadata))
        }
        _ => RoutePricing::Unknown,
    }
}

/// The model name without its routing prefix, so `x-ai/grok-4.6` served by
/// one host and `grok-4.6` served by another group as one model.
fn model_leaf(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// One report per [`SpendRange`], built from a single ledger read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpendReports([SpendReport; 4]);

impl SpendReports {
    pub(crate) fn get(&self, range: SpendRange) -> &SpendReport {
        &self.0[range.index()]
    }

    /// Reports for an empty ledger, with `now` in local time.
    pub(crate) fn empty(now: NaiveDateTime) -> Self {
        match Self::build(now, None, |_| Ok::<_, std::convert::Infallible>(())) {
            Ok(reports) => reports,
        }
    }

    /// Run `feed` with a sink that folds each request into every range it
    /// falls in, then finish all four reports.
    fn build<E>(
        now: NaiveDateTime,
        first_day: Option<NaiveDate>,
        feed: impl FnOnce(&mut dyn FnMut(&PricedRequest<'_>)) -> Result<(), E>,
    ) -> Result<Self, E> {
        let mut builders = SpendRange::ALL.map(|range| ReportBuilder::new(range, now, first_day));
        feed(&mut |request| {
            for builder in &mut builders {
                builder.add(request);
            }
        })?;
        Ok(Self(builders.map(ReportBuilder::finish)))
    }
}

/// Reads the ledger at `path` without creating or migrating it and builds a
/// report for every range, with `now` in `tz` local time. A missing file is
/// an empty ledger. `price_route` is called once per distinct route.
pub(crate) fn load_spend_reports<Tz: TimeZone>(
    path: &Path,
    tz: &Tz,
    now: NaiveDateTime,
    mut price_route: impl FnMut(&str, &str) -> RoutePricing,
) -> Result<SpendReports, UsageLedgerError> {
    if !path.is_file() {
        return Ok(SpendReports::empty(now));
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(UsageLedgerError::UnsupportedSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version < 1 {
        return Ok(SpendReports::empty(now));
    }

    let pricing = resolve_route_pricing(&connection, &mut price_route)?;
    let local_date = |ms: i64| {
        chrono::DateTime::from_timestamp_millis(ms).map(|utc| utc.with_timezone(tz).naive_local())
    };
    let first_ms: Option<i64> =
        connection.query_row("SELECT MIN(occurred_at_ms) FROM usage_events", [], |row| {
            row.get(0)
        })?;
    let first_day = first_ms.and_then(local_date).map(|at| at.date());

    SpendReports::build(now, first_day, |add| {
        let mut statement = connection.prepare(
            "SELECT occurred_at_ms, provider, model, purpose, input_tokens,
                    output_tokens, cache_read_tokens, cache_write_tokens, total_tokens,
                    cost_usd_micros
             FROM usage_events",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            // A timestamp chrono cannot represent has no local day to land on.
            let Some(at) = local_date(row.get(0)?) else {
                continue;
            };
            let count = |index: usize| -> rusqlite::Result<Option<u64>> {
                Ok(row
                    .get::<_, Option<i64>>(index)?
                    .and_then(|value| u64::try_from(value).ok()))
            };
            let text =
                |index: usize| -> rusqlite::Result<&str> { Ok(row.get_ref(index)?.as_str()?) };
            let usage = ModelUsage {
                input_tokens: count(4)?,
                output_tokens: count(5)?,
                cache_read_tokens: count(6)?,
                cache_write_tokens: count(7)?,
                total_tokens: count(8)?,
                ..ModelUsage::default()
            };
            let (provider, model) = (text(1)?, text(2)?);
            let cost = match count(9)? {
                Some(micros) => RowCost::Actual(micros),
                None => match pricing.get(provider).and_then(|models| models.get(model)) {
                    Some(RoutePricing::Local) => RowCost::Local,
                    Some(RoutePricing::Catalog(metadata)) => {
                        catalog_cost_usd_micros(&usage, metadata)
                            .map_or(RowCost::Unpriced, RowCost::Computed)
                    }
                    Some(RoutePricing::Unknown) | None => RowCost::Unpriced,
                },
            };
            add(&PricedRequest {
                at,
                provider,
                model: model_leaf(model),
                purpose: text(3)?,
                totals: SpendTotals::request(row_tokens(&usage), cost),
            });
        }
        Ok::<_, UsageLedgerError>(())
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowCost {
    Actual(u64),
    Computed(u64),
    Local,
    Unpriced,
}

/// One priced request in local time, borrowed from the current ledger row.
struct PricedRequest<'a> {
    at: NaiveDateTime,
    provider: &'a str,
    /// Already reduced to [`model_leaf`].
    model: &'a str,
    purpose: &'a str,
    totals: SpendTotals,
}

/// Accumulates one range's report as requests stream past.
struct ReportBuilder {
    /// Local start of the range; `None` for an empty all-time range.
    start: Option<NaiveDateTime>,
    /// Exclusive local end: rows dated after today (clock skew) are skipped.
    end: NaiveDateTime,
    unit: TimelineUnit,
    totals: SpendTotals,
    providers: HashMap<String, SpendTotals>,
    models: HashMap<String, SpendTotals>,
    purposes: HashMap<String, SpendTotals>,
    buckets: Vec<u64>,
}

impl ReportBuilder {
    /// `first_day` is the oldest request's local day, which starts all time.
    fn new(range: SpendRange, now: NaiveDateTime, first_day: Option<NaiveDate>) -> Self {
        let today = now.date();
        let first_day = range.first_day(today).or(first_day);
        let unit = range.timeline_unit();
        let bucket_count = match (unit, first_day) {
            (TimelineUnit::Hour, _) => 24,
            (TimelineUnit::Day, Some(first)) => (today - first).num_days().max(0) as usize + 1,
            (TimelineUnit::Day, None) => 0,
        };
        Self {
            start: first_day.map(midnight),
            end: midnight(today + Duration::days(1)),
            unit,
            totals: SpendTotals::default(),
            providers: HashMap::new(),
            models: HashMap::new(),
            purposes: HashMap::new(),
            buckets: vec![0; bucket_count],
        }
    }

    fn add(&mut self, request: &PricedRequest<'_>) {
        let Some(start) = self.start else {
            return;
        };
        if request.at < start || request.at >= self.end {
            return;
        }
        let totals = request.totals;
        self.totals += totals;
        for (groups, name) in [
            (&mut self.providers, request.provider),
            (&mut self.models, request.model),
            (&mut self.purposes, request.purpose),
        ] {
            match groups.get_mut(name) {
                Some(group) => *group += totals,
                None => {
                    groups.insert(name.to_owned(), totals);
                }
            }
        }
        let index = match self.unit {
            TimelineUnit::Hour => request.at.hour() as usize,
            TimelineUnit::Day => (request.at.date() - start.date()).num_days() as usize,
        };
        let last = self.buckets.len() - 1;
        self.buckets[index.min(last)] += totals.equivalent_usd_micros();
    }

    fn finish(self) -> SpendReport {
        SpendReport {
            totals: self.totals,
            providers: ranked(self.providers),
            models: ranked(self.models),
            purposes: ranked(self.purposes),
            timeline: Timeline {
                unit: self.unit,
                start: self.start.unwrap_or(self.end),
                equivalent_usd_micros: self.buckets,
            },
        }
    }
}

fn midnight(day: NaiveDate) -> NaiveDateTime {
    day.and_hms_opt(0, 0, 0).expect("midnight is valid")
}

/// Request counts and dollars for one slice of the ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SpendTotals {
    pub(crate) requests: u64,
    pub(crate) tokens: u64,
    pub(crate) actual_usd_micros: u64,
    pub(crate) computed_usd_micros: u64,
    pub(crate) local_requests: u64,
    pub(crate) unpriced_requests: u64,
}

impl SpendTotals {
    /// Totals for a single request.
    fn request(tokens: u64, cost: RowCost) -> Self {
        let mut totals = Self {
            requests: 1,
            tokens,
            ..Self::default()
        };
        match cost {
            RowCost::Actual(micros) => totals.actual_usd_micros = micros,
            RowCost::Computed(micros) => totals.computed_usd_micros = micros,
            RowCost::Local => totals.local_requests = 1,
            RowCost::Unpriced => totals.unpriced_requests = 1,
        }
        totals
    }

    /// Actual plus computed dollars: what this usage would cost on
    /// metered API pricing.
    pub(crate) fn equivalent_usd_micros(&self) -> u64 {
        self.actual_usd_micros
            .saturating_add(self.computed_usd_micros)
    }

    /// The dollar value to show for this slice.
    pub(crate) fn valuation(&self) -> Valuation {
        if self.requests > 0 && self.local_requests == self.requests {
            Valuation::Local
        } else if self.local_requests + self.unpriced_requests == self.requests {
            Valuation::Unpriced
        } else {
            Valuation::Usd(self.equivalent_usd_micros())
        }
    }
}

impl AddAssign for SpendTotals {
    fn add_assign(&mut self, other: Self) {
        self.requests += other.requests;
        self.tokens = self.tokens.saturating_add(other.tokens);
        self.actual_usd_micros = self
            .actual_usd_micros
            .saturating_add(other.actual_usd_micros);
        self.computed_usd_micros = self
            .computed_usd_micros
            .saturating_add(other.computed_usd_micros);
        self.local_requests += other.local_requests;
        self.unpriced_requests += other.unpriced_requests;
    }
}

/// What a slice of spend is worth in dollars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Valuation {
    /// Every request ran locally, so zero by design.
    Local,
    /// No request has a price (local ones aside).
    Unpriced,
    /// Actual plus computed dollars; unpriced requests add nothing.
    Usd(u64),
}

/// One provider, model, or purpose row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpendGroup {
    pub(crate) name: String,
    pub(crate) totals: SpendTotals,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimelineUnit {
    Hour,
    Day,
}

/// Equivalent spend per hour (today) or per local day, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Timeline {
    pub(crate) unit: TimelineUnit,
    /// Local start of the first bucket.
    pub(crate) start: NaiveDateTime,
    pub(crate) equivalent_usd_micros: Vec<u64>,
}

/// Aggregated spend for one [`SpendRange`]. Groups are sorted by equivalent
/// cost, then request count, then name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpendReport {
    pub(crate) totals: SpendTotals,
    pub(crate) providers: Vec<SpendGroup>,
    pub(crate) models: Vec<SpendGroup>,
    pub(crate) purposes: Vec<SpendGroup>,
    pub(crate) timeline: Timeline,
}

fn ranked(groups: HashMap<String, SpendTotals>) -> Vec<SpendGroup> {
    let mut groups: Vec<_> = groups
        .into_iter()
        .map(|(name, totals)| SpendGroup { name, totals })
        .collect();
    groups.sort_by(|left, right| {
        right
            .totals
            .equivalent_usd_micros()
            .cmp(&left.totals.equivalent_usd_micros())
            .then(right.totals.requests.cmp(&left.totals.requests))
            .then_with(|| left.name.cmp(&right.name))
    });
    groups
}

/// Provider total when reported, else prompt plus output.
fn row_tokens(usage: &ModelUsage) -> u64 {
    usage.total_tokens.unwrap_or_else(|| {
        usage
            .inclusive_prompt_tokens()
            .unwrap_or_default()
            .saturating_add(usage.output_tokens.unwrap_or_default())
    })
}

/// Price every distinct route once, keyed provider then model. A route
/// without its own price borrows the catalog price of another route serving
/// the same model leaf, so a proxy's `x-ai/grok-4.6` shares the price of a
/// catalogued `grok-4.6`. When several routes price one leaf, the first in
/// `(provider, model)` order wins.
fn resolve_route_pricing(
    connection: &Connection,
    price_route: &mut impl FnMut(&str, &str) -> RoutePricing,
) -> Result<HashMap<String, HashMap<String, RoutePricing>>, UsageLedgerError> {
    let mut statement = connection
        .prepare("SELECT DISTINCT provider, model FROM usage_events ORDER BY provider, model")?;
    let routes = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let resolved: Vec<_> = routes
        .into_iter()
        .map(|(provider, model)| {
            let pricing = price_route(&provider, &model);
            (provider, model, pricing)
        })
        .collect();
    let mut leaf_prices: BTreeMap<&str, &Arc<ModelMetadata>> = BTreeMap::new();
    for (_, model, pricing) in &resolved {
        if let RoutePricing::Catalog(metadata) = pricing {
            leaf_prices.entry(model_leaf(model)).or_insert(metadata);
        }
    }
    let borrowed: Vec<_> = resolved
        .iter()
        .map(|(_, model, pricing)| match pricing {
            RoutePricing::Unknown => leaf_prices
                .get(model_leaf(model))
                .map(|metadata| RoutePricing::Catalog(Arc::clone(metadata))),
            RoutePricing::Local | RoutePricing::Catalog(_) => None,
        })
        .collect();
    let mut pricing: HashMap<String, HashMap<String, RoutePricing>> = HashMap::new();
    for ((provider, model, own), borrowed) in resolved.into_iter().zip(borrowed) {
        pricing
            .entry(provider)
            .or_default()
            .insert(model, borrowed.unwrap_or(own));
    }
    Ok(pricing)
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
