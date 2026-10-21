//! Local weighted BM25 ranking shared by `tool_search` and script `search_tools`.
//!
//! Every catalog entry is one document with three weighted fields:
//!
//! - the name: a generated `<kind>__<server>__<tool>` name without its kind;
//! - the summary: the description's first paragraph;
//! - the details: the rest of the description, the top-level parameter
//!   names, and the name's kind (`mcp`, `openapi`). An OpenAPI operation's
//!   route and tags land here, after its summary.
//!
//! Ranking is BM25F: each field's term frequency is length-normalized against
//! that field's average, weighted, and summed before saturation, so a word in
//! a short name outranks the same word deep in a long description. Everything
//! is local and deterministic; no model or network is involved.

use std::collections::BTreeMap;

use super::exposure::ToolCatalogEntry;

/// BM25 term-frequency saturation. 1.2 is the standard default.
const K1: f64 = 1.2;
/// BM25 length normalization. 0.75 is the standard default.
const B: f64 = 0.75;

/// How much a related word counts against an exact one. A related word shares
/// the query word's stem (`issues` for `issue`), or one of the two stems
/// extends the other (`repository` and `repo`). Counting related words fully
/// ranked inflected noise such as `list_issue_types` level with exact hits;
/// ignoring them lost `repo` -> `repository`. Tuned on real MCP and GitHub REST tools (see the PR that
/// introduced this ranker).
const RELATED_WEIGHT: f64 = 0.3;

/// Shortest stem that may match by prefix, so `repo` finds `repository`
/// but `get` does not find `getter`.
const MIN_PREFIX_LEN: usize = 4;

/// One searchable part of a catalog entry.
#[derive(Clone, Copy)]
enum Field {
    Name,
    Summary,
    Details,
}

impl Field {
    const ALL: [Self; 3] = [Self::Name, Self::Summary, Self::Details];

    /// How much a word in this field counts. A name says what a tool does in a
    /// few words; a summary says it in a sentence; details only hint at the
    /// domain. Equal weights ranked noticeably worse on the tuning set.
    fn weight(self) -> f64 {
        match self {
            Self::Name => 3.0,
            Self::Summary => 1.5,
            Self::Details => 1.0,
        }
    }
}

/// One value per [`Field`], read by field rather than by position.
struct PerField<T> {
    name: T,
    summary: T,
    details: T,
}

impl<T> PerField<T> {
    fn get(&self, field: Field) -> &T {
        match field {
            Field::Name => &self.name,
            Field::Summary => &self.summary,
            Field::Details => &self.details,
        }
    }

    fn from_fn(mut value: impl FnMut(Field) -> T) -> Self {
        Self {
            name: value(Field::Name),
            summary: value(Field::Summary),
            details: value(Field::Details),
        }
    }
}

/// Ranks `entries` against `query` and returns at most `limit` matches.
///
/// An entry matches when any query word, or a word related to it, occurs in
/// any of its fields. Higher scores rank first and ties keep catalog (name)
/// order. A query that is exactly an entry's name ranks that entry first, since
/// `mcp__github__get-issue` and `mcp__github__get_issue` normalize to the same
/// words. A query without any words lists every entry; a query made only of
/// filler such as `a` or `to` matches nothing, since filler would otherwise
/// match every description.
pub(super) fn search_entries<'a>(
    entries: &'a [ToolCatalogEntry],
    query: &str,
    limit: usize,
) -> Vec<&'a ToolCatalogEntry> {
    if !query.chars().any(char::is_alphanumeric) {
        return entries.iter().take(limit).collect();
    }
    let index = Index::new(entries);
    let mut query_words: Vec<_> = words(query).collect();
    query_words.sort_unstable();
    query_words.dedup();
    let exact_name = query.trim();
    let terms: Vec<_> = query_words
        .iter()
        .map(|word| index.term(word))
        .filter(|term| !term.matches.is_empty())
        .collect();
    let mut hits: Vec<_> = entries
        .iter()
        .zip(&index.documents)
        .filter_map(|(entry, document)| {
            let score = if entry.name == exact_name {
                f64::INFINITY
            } else {
                index.score(document, &terms)
            };
            (score > 0.0).then_some((score, entry))
        })
        .collect();
    // Stable sort keeps name order within equal scores.
    hits.sort_by(|(left, _), (right, _)| right.total_cmp(left));
    hits.into_iter()
        .take(limit)
        .map(|(_, entry)| entry)
        .collect()
}

/// Word counts and length of one field of one entry.
#[derive(Default)]
struct FieldWords {
    counts: BTreeMap<String, usize>,
    len: usize,
}

impl FieldWords {
    fn new(words: impl IntoIterator<Item = String>) -> Self {
        let mut field = Self::default();
        for word in words {
            *field.counts.entry(word).or_default() += 1;
            field.len += 1;
        }
        field
    }
}

type Document = PerField<FieldWords>;

fn document(entry: &ToolCatalogEntry) -> Document {
    // Generated names are `<kind>__<segment>__<segment>` (see `tools::tool_name`).
    // Every tool of a kind shares it, so it would only dilute the name field.
    let (kind, name) = entry
        .name
        .split_once("__")
        .unwrap_or(("", entry.name.as_str()));
    let (summary, rest) = entry
        .description
        .split_once("\n\n")
        .unwrap_or((&entry.description, ""));
    let parameters = entry.parameters["properties"]
        .as_object()
        .into_iter()
        .flat_map(|properties| properties.keys())
        .flat_map(|name| words(name));
    PerField {
        name: FieldWords::new(words(name)),
        summary: FieldWords::new(words(summary)),
        details: FieldWords::new(words(rest).chain(parameters).chain(words(kind))),
    }
}

/// One query word resolved against the catalog vocabulary.
struct Term<'a> {
    /// Catalog words that count for this query word, with their weight.
    matches: Vec<(&'a str, f64)>,
    idf: f64,
}

/// Corpus statistics BM25 needs, computed per search from the live catalog.
struct Index {
    documents: Vec<Document>,
    average_len: PerField<f64>,
    /// Every catalog word and its stem. Ordered, so float sums over matches
    /// add up in the same order on every run.
    vocabulary: BTreeMap<String, String>,
}

impl Index {
    fn new(entries: &[ToolCatalogEntry]) -> Self {
        let documents: Vec<_> = entries.iter().map(document).collect();
        let count = documents.len().max(1) as f64;
        let average_len = PerField::from_fn(|field| {
            documents
                .iter()
                .map(|document| document.get(field).len as f64)
                .sum::<f64>()
                / count
        });
        let vocabulary = documents
            .iter()
            .flat_map(|document| Field::ALL.map(|field| document.get(field)))
            .flat_map(|field| field.counts.keys())
            .map(|word| (word.clone(), stem(word)))
            .collect();
        Self {
            documents,
            average_len,
            vocabulary,
        }
    }

    /// Finds the catalog words that count for `word` and how rare they are.
    ///
    /// Rarity comes from the words sharing `word`'s stem when the catalog has
    /// any, so common words that only extend it, like `data` for `database`,
    /// do not make a rare word look common.
    fn term(&self, word: &str) -> Term<'_> {
        let word_stem = stem(word);
        let mut matches = Vec::new();
        let mut same_stem = Vec::new();
        for (candidate, candidate_stem) in &self.vocabulary {
            let weight = if candidate == word {
                1.0
            } else if *candidate_stem == word_stem || extends(&word_stem, candidate_stem) {
                RELATED_WEIGHT
            } else {
                continue;
            };
            matches.push((candidate.as_str(), weight));
            if *candidate_stem == word_stem {
                same_stem.push(candidate.as_str());
            }
        }
        let rarity_words = if same_stem.is_empty() {
            matches.iter().map(|(word, _)| *word).collect()
        } else {
            same_stem
        };
        let containing = self
            .documents
            .iter()
            .filter(|document| {
                Field::ALL.into_iter().any(|field| {
                    let counts = &document.get(field).counts;
                    rarity_words.iter().any(|word| counts.contains_key(*word))
                })
            })
            .count() as f64;
        let total = self.documents.len() as f64;
        // Positive even for a word in every entry, so any match still counts.
        let idf = (1.0 + (total - containing + 0.5) / (containing + 0.5)).ln();
        Term { matches, idf }
    }

    /// Sums each term's saturated, field-weighted frequency in `document`.
    fn score(&self, document: &Document, terms: &[Term<'_>]) -> f64 {
        terms
            .iter()
            .map(|term| {
                let frequency: f64 = Field::ALL
                    .into_iter()
                    .map(|field| {
                        let words = document.get(field);
                        let count: f64 = term
                            .matches
                            .iter()
                            .filter_map(|(word, weight)| {
                                words.counts.get(*word).map(|count| *count as f64 * weight)
                            })
                            .sum();
                        if count == 0.0 {
                            return 0.0;
                        }
                        // A match means this field has words somewhere, so its
                        // average length is positive.
                        let relative_len = words.len as f64 / self.average_len.get(field);
                        field.weight() * count / (1.0 - B + B * relative_len)
                    })
                    .sum();
                term.idf * frequency / (K1 + frequency)
            })
            .sum()
    }
}

/// Whether one stem extends the other and the shorter is at least
/// [`MIN_PREFIX_LEN`] long. Both directions count: `repo` finds `repository`,
/// and `repository` finds the `repos` in `repos_get`.
fn extends(left: &str, right: &str) -> bool {
    left.len().min(right.len()) >= MIN_PREFIX_LEN
        && (left.starts_with(right) || right.starts_with(left))
}

/// Lowercase words of `text`, split on punctuation (`_`, `-`, `.`, `/`,
/// spaces, ...) and camelCase boundaries, without filler. A camelCase token
/// also counts whole, so `GitHub` matches `github` as well as `git` and `hub`.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .flat_map(|token| {
            let parts = camel_case_words(token);
            let whole = (parts.len() > 1).then_some(token);
            whole.into_iter().chain(parts)
        })
        .map(str::to_lowercase)
        .filter(|word| !STOPWORDS.contains(&word.as_str()))
}

/// Splits `getRepository` into `get`, `Repository` and `HTTPServer` into
/// `HTTP`, `Server`. Digits stay attached, so `s3` and `base64` survive.
fn camel_case_words(word: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut words = Vec::new();
    let mut start = 0;
    for (position, &(index, current)) in chars.iter().enumerate().skip(1) {
        let previous = chars[position - 1].1;
        let next = chars.get(position + 1).map(|&(_, next)| next);
        let lower_to_upper =
            current.is_uppercase() && (previous.is_lowercase() || previous.is_numeric());
        let acronym_end = current.is_uppercase()
            && previous.is_uppercase()
            && next.is_some_and(char::is_lowercase);
        if lower_to_upper || acronym_end {
            words.push(&word[start..index]);
            start = index;
        }
    }
    if start < word.len() {
        words.push(&word[start..]);
    }
    words
}

/// Function words that occur in nearly every description. Queries are short
/// phrases like `post a message to slack`, where these carry no intent.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "into", "is", "it",
    "its", "of", "on", "or", "that", "the", "this", "to", "with",
];

/// Shortest stem a suffix rule may leave, so short words such as `need` or
/// `bus` are not cut down to fragments.
const MIN_STEM_LEN: usize = 3;

/// A light English stemmer: plural `-s`/`-es`/`-ies`, then `-ing`/`-ed`, then
/// a final `-e`. It only needs to map inflections of a word to one key, not
/// to produce real words: `create`, `created`, and `creates` all become
/// `creat`.
fn stem(word: &str) -> String {
    let mut stem = word.to_owned();
    // `-ies` needs a two-letter base, so `copies` becomes `copy` but `lies`
    // only loses its `-s`.
    if let Some(base) = stem.strip_suffix("ies").filter(|base| base.len() >= 2) {
        stem = format!("{base}y");
    } else if stem.ends_with('s') && !stem.ends_with("ss") && !stem.ends_with("us") {
        trim_to_stem(&mut stem, "s");
    }
    if trim_verb_to_stem(&mut stem, "ing") || trim_verb_to_stem(&mut stem, "ed") {
        undouble(&mut stem);
    }
    trim_to_stem(&mut stem, "e");
    stem
}

/// Removes `suffix` when at least [`MIN_STEM_LEN`] characters remain.
fn trim_to_stem(word: &mut String, suffix: &str) -> bool {
    let keep = word.len().saturating_sub(suffix.len());
    let strips = word.ends_with(suffix) && word.is_char_boundary(keep) && keep >= MIN_STEM_LEN;
    if strips {
        word.truncate(keep);
    }
    strips
}

/// Like [`trim_to_stem`], but only when a vowel remains, so `string` and
/// `red` keep their endings while `listing` and `created` lose theirs.
fn trim_verb_to_stem(word: &mut String, suffix: &str) -> bool {
    let has_vowel = word
        .strip_suffix(suffix)
        .is_some_and(|base| base.contains(['a', 'e', 'i', 'o', 'u', 'y']));
    has_vowel && trim_to_stem(word, suffix)
}

/// `getting` loses `ing` as `gett`; drop the doubled consonant so it matches
/// `get`. `ll`, `ss`, and `zz` are usually part of the word, as in `install`.
fn undouble(word: &mut String) {
    let mut tail = word.chars().rev();
    if let (Some(last), Some(previous)) = (tail.next(), tail.next()) {
        if last == previous
            && last.is_ascii_alphabetic()
            && !matches!(last, 'a' | 'e' | 'i' | 'o' | 'u' | 'l' | 's' | 'z')
        {
            word.pop();
        }
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
