//! Parsing and running a palette query against the artifact manifest.
//!
//! This is the pure half of Ctrl-P: turning what someone typed into a filtered,
//! ranked, bounded list. It holds no manifest of its own — Alfalfa owns the
//! index, and Canvas is a client of it — so everything here operates on entries
//! handed in by the caller.
//!
//! Two decisions in here are worth knowing about before reading the code.
//!
//! **A bare `pdf` becomes a format filter, and the palette must say so.** Typing
//! `pdf` almost always means "show me PDFs", not "find things whose name
//! contains the letters p-d-f", so the parser promotes it. But an inference the
//! user cannot see is one they cannot correct, so [`Query::inferred`] reports
//! every promotion for the surface to render as a removable chip. Infer, then
//! disclose — never infer silently.
//!
//! **An empty query lists recent artifacts rather than nothing.** A palette that
//! is blank until you type makes you guess what it can even find.

use serde::{Deserialize, Serialize};

/// One row of the manifest, as much of it as a palette needs.
///
/// Deliberately a subset: Alfalfa defines the wire shape, and Canvas should be
/// able to keep rendering when it grows fields this does not know about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Content hash. Identity and fetch key.
    pub id: String,
    pub name: String,
    /// Full media type, e.g. `image/png`. Both halves are matchable: `fmt:img`
    /// matches the type, `fmt:png` the subtype.
    pub content_type: String,
    pub bytes: u64,
    pub store: Store,
    /// Projects holding a backlink to this artifact.
    pub projects: Vec<String>,
    pub created_at_ms: u64,
    /// When search last indexed it. `None` means saved but not yet searchable —
    /// which the palette shows, because "not indexed yet" and "does not exist"
    /// must not look the same.
    pub indexed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Store {
    /// First-party files in a project.
    Project,
    /// Public provenance.
    Research,
    /// Generated or imported.
    Wiki,
}

impl Store {
    fn token(self) -> &'static str {
        match self {
            Store::Project => "project",
            Store::Research => "research",
            Store::Wiki => "wiki",
        }
    }
}

/// A promotion the parser made on the user's behalf, so the surface can show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inferred {
    /// What was typed.
    pub typed: String,
    /// What it was read as, e.g. `fmt:pdf`.
    pub read_as: String,
}

/// A parsed palette query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Media type or subtype tokens. Multiple are OR'd: `fmt:img fmt:pdf` means
    /// "either", because that is what someone narrowing a list expects.
    pub formats: Vec<String>,
    pub stores: Vec<String>,
    pub projects: Vec<String>,
    /// Everything not recognised as a filter, matched against the name. AND'd:
    /// each extra word narrows.
    pub terms: Vec<String>,
    pub inferred: Vec<Inferred>,
}

/// Short tokens that mean a format even without the `fmt:` prefix.
///
/// Only unambiguous ones. `text` is left out on purpose: it is a plausible thing
/// to search for by name, and guessing wrong on a common word is worse than not
/// guessing at all.
const BARE_FORMATS: &[&str] = &[
    "img", "image", "pdf", "png", "jpeg", "jpg", "svg", "gif", "webp", "md", "markdown", "csv",
    "json", "html", "video", "audio",
];

pub fn parse(input: &str) -> Query {
    let mut query = Query::default();
    for word in input.split_whitespace() {
        let lower = word.to_lowercase();
        if let Some(value) = lower.strip_prefix("fmt:") {
            push(&mut query.formats, value);
        } else if let Some(value) = lower.strip_prefix("store:") {
            push(&mut query.stores, value);
        } else if let Some(value) = lower.strip_prefix("project:") {
            push(&mut query.projects, value);
        } else if BARE_FORMATS.contains(&lower.as_str()) {
            push(&mut query.formats, &lower);
            query.inferred.push(Inferred {
                typed: word.to_string(),
                read_as: format!("fmt:{lower}"),
            });
        } else {
            query.terms.push(lower);
        }
    }
    query
}

fn push(target: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && !target.iter().any(|held| held == value) {
        target.push(value.to_string());
    }
}

/// A matched entry and why it ranked where it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    pub entry: Entry,
    /// Higher is better. Exposed so a surface can group or explain, not just
    /// order.
    pub score: u32,
}

/// What a palette should draw.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Results {
    pub hits: Vec<Hit>,
    /// Matches beyond `limit`. Shown, because a list capped at ten with a
    /// thousand behind it reads as "there are ten".
    pub more: usize,
    /// Total matches, shown or not.
    pub matched: usize,
}

/// Run a query.
///
/// `now_ms` orders an empty query by recency. Passed in rather than read so the
/// same manifest and query always produce the same list.
pub fn run(query: &Query, entries: &[Entry], limit: usize) -> Results {
    let mut hits: Vec<Hit> = entries
        .iter()
        .filter(|entry| matches(query, entry))
        .map(|entry| Hit {
            score: score(query, entry),
            entry: entry.clone(),
        })
        .collect();
    // Deterministic: score, then recency, then id — never leaving ties to the
    // order the manifest happened to arrive in, because a list that reshuffles
    // between keystrokes is unusable.
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.entry.created_at_ms.cmp(&a.entry.created_at_ms))
            .then(a.entry.id.cmp(&b.entry.id))
    });
    let matched = hits.len();
    hits.truncate(limit);
    Results {
        more: matched.saturating_sub(hits.len()),
        matched,
        hits,
    }
}

fn matches(query: &Query, entry: &Entry) -> bool {
    if !query.formats.is_empty() && !query.formats.iter().any(|fmt| format_matches(fmt, entry)) {
        return false;
    }
    if !query.stores.is_empty()
        && !query
            .stores
            .iter()
            .any(|store| entry.store.token().starts_with(store.as_str()))
    {
        return false;
    }
    if !query.projects.is_empty()
        && !query.projects.iter().any(|wanted| {
            entry
                .projects
                .iter()
                .any(|held| held.to_lowercase() == *wanted)
        })
    {
        return false;
    }
    let name = entry.name.to_lowercase();
    query.terms.iter().all(|term| name.contains(term.as_str()))
}

/// `img` matches the type half, `png` the subtype half, and `image/png` matches
/// whole. One rule, no lookup table to drift out of date as media types are
/// added.
fn format_matches(wanted: &str, entry: &Entry) -> bool {
    let content = entry.content_type.to_lowercase();
    let (kind, subtype) = content.split_once('/').unwrap_or((content.as_str(), ""));
    // `image/svg+xml` is an SVG. The structured suffix after `+` names the
    // encoding, which is not what anyone is searching for.
    let base = subtype.split('+').next().unwrap_or(subtype);
    // Fold the words people actually type onto the ones media types use, so the
    // arms below stay about matching rather than about spelling.
    let wanted = match wanted {
        "image" => "img",
        "md" => "markdown",
        "jpg" => "jpeg",
        other => other,
    };
    match wanted {
        "img" => kind == "image",
        // A file named `.md` is markdown even when the store called it
        // text/plain, which is common enough to be worth handling.
        "markdown" => base == "markdown" || entry.name.to_lowercase().ends_with(".md"),
        other => kind == other || base == other || subtype == other || content == other,
    }
}

/// How well the name matches, ignoring filters — a filter decides membership,
/// not rank.
fn score(query: &Query, entry: &Entry) -> u32 {
    if query.terms.is_empty() {
        return 0;
    }
    let name = entry.name.to_lowercase();
    query
        .terms
        .iter()
        .map(|term| {
            if name == *term {
                100
            } else if name.starts_with(term.as_str()) {
                60
            } else if name
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word.starts_with(term.as_str()))
            {
                40
            } else {
                10
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, content_type: &str, store: Store, at: u64) -> Entry {
        Entry {
            id: format!("{:064x}", at),
            name: name.into(),
            content_type: content_type.into(),
            bytes: 1024,
            store,
            projects: vec!["dock".into()],
            created_at_ms: at,
            indexed_at_ms: Some(at),
        }
    }

    fn corpus() -> Vec<Entry> {
        vec![
            entry("packing-slip.pdf", "application/pdf", Store::Project, 10),
            entry("wiring-diagram.png", "image/png", Store::Project, 20),
            entry("thermal-report.pdf", "application/pdf", Store::Research, 30),
            entry("widget-quality.md", "text/markdown", Store::Wiki, 40),
            entry("pdf-tooling-notes.md", "text/markdown", Store::Wiki, 50),
            entry("logo.svg", "image/svg+xml", Store::Project, 60),
        ]
    }

    fn names(results: &Results) -> Vec<String> {
        results
            .hits
            .iter()
            .map(|hit| hit.entry.name.clone())
            .collect()
    }

    #[test]
    fn a_bare_format_word_filters_and_says_that_it_did() {
        let query = parse("pdf");
        assert_eq!(query.formats, vec!["pdf"]);
        assert!(query.terms.is_empty());
        assert_eq!(
            query.inferred,
            vec![Inferred {
                typed: "pdf".into(),
                read_as: "fmt:pdf".into()
            }],
            "an inference the user cannot see is one they cannot correct"
        );
    }

    #[test]
    fn an_explicit_filter_is_not_reported_as_an_inference() {
        let query = parse("fmt:pdf");
        assert_eq!(query.formats, vec!["pdf"]);
        assert!(
            query.inferred.is_empty(),
            "nothing was guessed, so nothing should be shown as guessed"
        );
    }

    #[test]
    fn bare_pdf_finds_pdfs_not_things_named_pdf() {
        let results = run(&parse("pdf"), &corpus(), 10);
        assert_eq!(
            names(&results),
            vec!["thermal-report.pdf", "packing-slip.pdf"]
        );
        assert!(
            !names(&results).contains(&"pdf-tooling-notes.md".to_string()),
            "typing pdf means show me PDFs"
        );
    }

    #[test]
    fn a_word_that_is_not_a_format_searches_the_name() {
        let results = run(&parse("thermal"), &corpus(), 10);
        assert_eq!(names(&results), vec!["thermal-report.pdf"]);
    }

    #[test]
    fn filters_and_terms_compose() {
        let results = run(&parse("fmt:md pdf-tooling"), &corpus(), 10);
        assert_eq!(names(&results), vec!["pdf-tooling-notes.md"]);
    }

    #[test]
    fn extra_words_narrow_rather_than_widen() {
        let one = run(&parse("report"), &corpus(), 10).matched;
        let two = run(&parse("report thermal"), &corpus(), 10).matched;
        assert!(two <= one, "each word must narrow: {one} then {two}");
    }

    #[test]
    fn several_formats_mean_either() {
        let results = run(&parse("fmt:pdf fmt:png"), &corpus(), 10);
        assert_eq!(results.matched, 3);
    }

    #[test]
    fn img_matches_the_type_and_png_matches_the_subtype() {
        assert_eq!(run(&parse("img"), &corpus(), 10).matched, 2);
        assert_eq!(
            names(&run(&parse("png"), &corpus(), 10)),
            vec!["wiring-diagram.png"]
        );
        assert_eq!(names(&run(&parse("svg"), &corpus(), 10)), vec!["logo.svg"]);
    }

    #[test]
    fn a_store_filter_separates_generated_pages_from_first_party_files() {
        assert_eq!(run(&parse("store:wiki"), &corpus(), 10).matched, 2);
        assert_eq!(run(&parse("store:project"), &corpus(), 10).matched, 3);
    }

    #[test]
    fn an_exact_name_outranks_a_prefix_which_outranks_a_substring() {
        let entries = vec![
            entry("report", "text/plain", Store::Project, 1),
            entry("report-final", "text/plain", Store::Project, 2),
            entry("quarterly-report", "text/plain", Store::Project, 3),
        ];
        assert_eq!(
            names(&run(&parse("report"), &entries, 10)),
            vec!["report", "report-final", "quarterly-report"]
        );
    }

    #[test]
    fn the_result_list_says_how_many_it_is_not_showing() {
        let entries: Vec<Entry> = (0..50)
            .map(|n| entry(&format!("note-{n:02}.md"), "text/markdown", Store::Wiki, n))
            .collect();
        let results = run(&parse("note"), &entries, 8);
        assert_eq!(results.hits.len(), 8);
        assert_eq!(results.matched, 50);
        assert_eq!(
            results.more, 42,
            "eight rows with a thousand behind them reads as eight"
        );
    }

    #[test]
    fn an_empty_query_lists_recent_artifacts_rather_than_nothing() {
        let results = run(&parse(""), &corpus(), 3);
        assert_eq!(
            results.hits.len(),
            3,
            "a blank palette should still be useful"
        );
        assert_eq!(
            names(&results),
            vec!["logo.svg", "pdf-tooling-notes.md", "widget-quality.md"],
            "most recent first"
        );
        assert_eq!(results.matched, 6);
    }

    #[test]
    fn a_query_that_matches_nothing_says_so_rather_than_falling_back() {
        let results = run(&parse("nonexistent-thing"), &corpus(), 10);
        assert!(results.hits.is_empty());
        assert_eq!(results.matched, 0);
        assert_eq!(results.more, 0);
    }

    #[test]
    fn ordering_is_stable_between_keystrokes() {
        let entries = corpus();
        for _ in 0..5 {
            assert_eq!(
                names(&run(&parse("fmt:md"), &entries, 10)),
                names(&run(&parse("fmt:md"), &entries, 10)),
                "a list that reshuffles between identical queries is unusable"
            );
        }
    }

    #[test]
    fn case_does_not_matter_to_a_person_typing_quickly() {
        assert_eq!(
            names(&run(&parse("THERMAL"), &corpus(), 10)),
            names(&run(&parse("thermal"), &corpus(), 10))
        );
        assert_eq!(parse("FMT:PDF").formats, vec!["pdf"]);
    }

    #[test]
    fn a_project_filter_uses_the_backlinks() {
        let mut entries = corpus();
        entries[0].projects = vec!["bifrost".into()];
        assert_eq!(run(&parse("project:bifrost"), &entries, 10).matched, 1);
        assert_eq!(run(&parse("project:dock"), &entries, 10).matched, 5);
    }
}
