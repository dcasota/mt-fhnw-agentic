//! Contamination signals: cross-index verification against Crossref,
//! OpenAlex, and Semantic Scholar.
//!
//! Implements ADR-0014. Four boolean signals per reference:
//!   * `preprint_post_llm_inflection` — year ≥ 2024 AND venue ∈ preprint hosts
//!   * `semantic_scholar_unmatched`
//!   * `openalex_unmatched`
//!   * `crossref_unmatched`
//!
//! A likely-fabricated academic reference (academic type, no DOI, unmatched in
//! all three indices while online) BLOCKS (Error, ADR-0036 references gate);
//! other signals remain advisory (Info / Warn).

use std::collections::HashSet;

use agentic_core::passport;
use anyhow::Result;
use chrono::Utc;
use reqwest::Client;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::{CheckReport, Finding, Severity};

const PREPRINT_HOSTS: &[&str] = &[
    "arxiv",
    "biorxiv",
    "medrxiv",
    "ssrn",
    "preprints.org",
    "researchgate",
    "techrxiv",
    "chemrxiv",
    "engrxiv",
    "psyarxiv",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Signals {
    pub preprint_post_llm_inflection: bool,
    pub semantic_scholar_unmatched: bool,
    pub openalex_unmatched: bool,
    pub crossref_unmatched: bool,
    /// #7: the reference is an arXiv/preprint-DOI work. Crossref does not index
    /// arXiv DOIs (`10.48550/arXiv.*`), so a Crossref miss is EXPECTED and must
    /// not count as a fabrication signal — OpenAlex/S2 (which do index arXiv)
    /// are authoritative here.
    pub is_arxiv: bool,
    /// #1 (ADR-0036): a DOI was supplied and resolved, but the resolved work's
    /// title/year does NOT match the reference — i.e. the DOI points at a
    /// different paper (a wrong or fabricated DOI). This is the strongest signal.
    pub doi_metadata_mismatch: bool,
    /// The title the supplied DOI actually resolves to (mismatch evidence).
    pub doi_resolved_title: Option<String>,
    pub checked_at: String,
}

/// #4 (ADR-0026): PRISMA-style disposition of one reference, separating a
/// legitimately-not-indexed item from a genuinely suspect one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefClass {
    /// Matched in at least one index (or DOI resolved with consistent metadata).
    Matched,
    /// Not in any index but plausibly so (grey literature, brand-new arXiv) —
    /// advisory, not a contamination signal.
    NotIndexed,
    /// Borderline: partial/ambiguous evidence — route to cross-model (#5) or
    /// HITL/consensus (#6) before judging.
    Suspect,
    /// DOI-metadata mismatch or academic-no-DOI-unmatched-everywhere — blocks.
    Fabricated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub citation_key: String,
    pub title: Option<String>,
    pub year: Option<u16>,
    pub doi: Option<String>,
    pub venue: Option<String>,
    pub ref_type: Option<String>,
}

#[must_use]
fn is_preprint_post_2024(year: Option<u16>, venue: Option<&str>) -> bool {
    let Some(year) = year else { return false };
    if year < 2024 {
        return false;
    }
    let Some(venue) = venue else { return false };
    let v = venue.to_lowercase();
    PREPRINT_HOSTS.iter().any(|h| v.contains(h))
}

/// #7: is this an arXiv (or arXiv-DOI) reference? Crossref does not index
/// arXiv DOIs, so a Crossref miss for these is expected, not suspicious.
#[must_use]
fn is_arxiv_ref(reference: &Reference) -> bool {
    let doi_arxiv = reference
        .doi
        .as_deref()
        .is_some_and(|d| d.to_lowercase().contains("arxiv"));
    let venue_arxiv = reference
        .venue
        .as_deref()
        .is_some_and(|v| v.to_lowercase().contains("arxiv"));
    doi_arxiv || venue_arxiv
}

/// Significant (>2-char) lowercased word tokens of a title.
fn title_tokens(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_owned)
        .collect()
}

/// #1: do two titles plausibly denote the same work? Token-overlap over the
/// smaller token set. Empty/whitespace titles are treated as "can't compare"
/// (returns true) so we never flag a mismatch we cannot actually establish.
#[must_use]
fn titles_consistent(a: &str, b: &str) -> bool {
    let ta = title_tokens(a);
    let tb = title_tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return true;
    }
    let inter = ta.intersection(&tb).count();
    let denom = ta.len().min(tb.len());
    (inter as f64 / denom as f64) >= 0.6
}

/// #1: fetch the title/year a DOI actually resolves to (via Crossref), so a
/// supplied DOI can be checked against the reference it claims to support.
async fn crossref_doi_metadata(
    client: &Client,
    conn: &Connection,
    doi: &str,
) -> Option<(String, Option<u16>)> {
    let url = format!(
        "https://api.crossref.org/works/{}",
        urlencoding::encode(doi)
    );
    let key = cache_key("crossref", &url);
    let body = if let Some(b) = cached_response(conn, &key) {
        b
    } else {
        let mailto = std::env::var("AGENTIC_API_MAILTO").unwrap_or_default();
        let agent = format!("agentic-mt-fhnw (mailto:{mailto})");
        match client.get(&url).header("User-Agent", agent).send().await {
            Ok(resp) if resp.status().is_success() => {
                let b = resp.text().await.unwrap_or_default();
                store_cache(conn, &key, "crossref", &url, &b);
                b
            }
            _ => return None,
        }
    };
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let msg = v.get("message")?;
    let title = msg
        .get("title")
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|s| s.as_str())
        .map(str::to_owned)?;
    let year = msg
        .get("issued")
        .or_else(|| msg.get("published"))
        .and_then(|p| p.get("date-parts"))
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u16::try_from(n).ok());
    Some((title, year))
}

fn cache_key(service: &str, key: &str) -> String {
    let mut h = Sha256::new();
    h.update(service.as_bytes());
    h.update(b"|");
    h.update(key.as_bytes());
    hex::encode(h.finalize())
}

fn cached_response(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT response_json FROM api_cache WHERE cache_key = ?1",
        rusqlite::params![key],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

fn store_cache(conn: &Connection, key: &str, service: &str, endpoint: &str, response_json: &str) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO api_cache (cache_key, service, endpoint, response_json, bytes) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            key,
            service,
            endpoint,
            response_json,
            response_json.len() as i64
        ],
    );
}

async fn crossref_lookup(
    client: &Client,
    conn: &Connection,
    doi: Option<&str>,
    title: Option<&str>,
) -> bool {
    let url = if let Some(doi) = doi {
        format!(
            "https://api.crossref.org/works/{}",
            urlencoding::encode(doi)
        )
    } else if let Some(title) = title {
        format!(
            "https://api.crossref.org/works?query.title={}&rows=1",
            urlencoding::encode(title),
        )
    } else {
        return false;
    };
    let key = cache_key("crossref", &url);
    if let Some(body) = cached_response(conn, &key) {
        return crossref_matched(&body);
    }
    let mailto = std::env::var("AGENTIC_API_MAILTO").unwrap_or_default();
    let agent = format!("agentic-mt-fhnw (mailto:{mailto})");
    match client.get(&url).header("User-Agent", agent).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await.unwrap_or_default();
            store_cache(conn, &key, "crossref", &url, &body);
            crossref_matched(&body)
        }
        Ok(_) | Err(_) => false,
    }
}

fn crossref_matched(body: &str) -> bool {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if let Some(msg) = v.get("message") {
        if msg.get("DOI").is_some() {
            return true;
        }
        if let Some(items) = msg.get("items").and_then(|i| i.as_array()) {
            if !items.is_empty() {
                return true;
            }
        }
    }
    false
}

async fn openalex_lookup(
    client: &Client,
    conn: &Connection,
    doi: Option<&str>,
    title: Option<&str>,
) -> bool {
    let url = if let Some(doi) = doi {
        format!(
            "https://api.openalex.org/works/doi:{}",
            urlencoding::encode(doi)
        )
    } else if let Some(title) = title {
        format!(
            "https://api.openalex.org/works?search={}&per_page=1",
            urlencoding::encode(title)
        )
    } else {
        return false;
    };
    let mailto = std::env::var("AGENTIC_API_MAILTO").unwrap_or_default();
    let url_with_mailto = if mailto.is_empty() {
        url.clone()
    } else if url.contains('?') {
        format!("{url}&mailto={}", urlencoding::encode(&mailto))
    } else {
        format!("{url}?mailto={}", urlencoding::encode(&mailto))
    };
    let key = cache_key("openalex", &url_with_mailto);
    if let Some(body) = cached_response(conn, &key) {
        return openalex_matched(&body);
    }
    match client.get(&url_with_mailto).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await.unwrap_or_default();
            store_cache(conn, &key, "openalex", &url_with_mailto, &body);
            openalex_matched(&body)
        }
        _ => false,
    }
}

fn openalex_matched(body: &str) -> bool {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if v.get("id").is_some() {
        return true;
    }
    if let Some(results) = v.get("results").and_then(|r| r.as_array()) {
        if !results.is_empty() {
            return true;
        }
    }
    false
}

async fn s2_lookup(
    client: &Client,
    conn: &Connection,
    doi: Option<&str>,
    title: Option<&str>,
) -> bool {
    let url = if let Some(doi) = doi {
        format!(
            "https://api.semanticscholar.org/graph/v1/paper/DOI:{}",
            urlencoding::encode(doi),
        )
    } else if let Some(title) = title {
        format!(
            "https://api.semanticscholar.org/graph/v1/paper/search?query={}&limit=1",
            urlencoding::encode(title),
        )
    } else {
        return false;
    };
    let key = cache_key("semantic_scholar", &url);
    if let Some(body) = cached_response(conn, &key) {
        return s2_matched(&body);
    }
    let mut req = client.get(&url);
    if let Ok(api_key) = std::env::var("AGENTIC_SEMANTIC_SCHOLAR_KEY") {
        req = req.header("x-api-key", api_key);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await.unwrap_or_default();
            store_cache(conn, &key, "semantic_scholar", &url, &body);
            s2_matched(&body)
        }
        _ => false,
    }
}

fn s2_matched(body: &str) -> bool {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if v.get("paperId").is_some() {
        return true;
    }
    if let Some(data) = v.get("data").and_then(|d| d.as_array()) {
        if !data.is_empty() {
            return true;
        }
    }
    false
}

/// Compute signals for one reference.
pub async fn signals_for(
    client: &Client,
    conn: &Connection,
    reference: &Reference,
    use_network: bool,
) -> Signals {
    let mut s = Signals {
        preprint_post_llm_inflection: is_preprint_post_2024(
            reference.year,
            reference.venue.as_deref(),
        ),
        is_arxiv: is_arxiv_ref(reference),
        checked_at: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        ..Default::default()
    };
    if !use_network {
        s.crossref_unmatched = reference.doi.is_none();
        s.openalex_unmatched = reference.doi.is_none();
        s.semantic_scholar_unmatched = reference.doi.is_none();
        return s;
    }
    s.crossref_unmatched = !crossref_lookup(
        client,
        conn,
        reference.doi.as_deref(),
        reference.title.as_deref(),
    )
    .await;
    s.openalex_unmatched = !openalex_lookup(
        client,
        conn,
        reference.doi.as_deref(),
        reference.title.as_deref(),
    )
    .await;
    s.semantic_scholar_unmatched = !s2_lookup(
        client,
        conn,
        reference.doi.as_deref(),
        reference.title.as_deref(),
    )
    .await;
    // #1 (ADR-0036): if a non-arXiv DOI is supplied, resolve it and confirm it
    // points at THIS reference (title/year). A resolved-but-mismatched DOI is a
    // wrong/fabricated identifier — far stronger than a mere index miss.
    if let (Some(doi), Some(title)) = (reference.doi.as_deref(), reference.title.as_deref()) {
        if !s.is_arxiv {
            if let Some((resolved_title, resolved_year)) =
                crossref_doi_metadata(client, conn, doi).await
            {
                let title_ok = titles_consistent(title, &resolved_title);
                let year_ok = match (reference.year, resolved_year) {
                    (Some(a), Some(b)) => a.abs_diff(b) <= 1,
                    _ => true,
                };
                if !title_ok || !year_ok {
                    s.doi_metadata_mismatch = true;
                    s.doi_resolved_title = Some(resolved_title);
                }
            }
        }
    }
    s
}

/// Number of indices the reference matched in (Crossref / OpenAlex / S2).
#[must_use]
fn match_count(s: &Signals) -> u8 {
    u8::from(!s.crossref_unmatched)
        + u8::from(!s.openalex_unmatched)
        + u8::from(!s.semantic_scholar_unmatched)
}

/// Whether the reference is academic-typed (vs. grey literature).
#[must_use]
fn is_academic(ref_type: Option<&str>) -> bool {
    matches!(
        ref_type,
        Some(
            "article"
                | "inproceedings"
                | "conference"
                | "journal"
                | "book"
                | "incollection"
                | "phdthesis"
                | "proceedings"
                | "techreport"
        )
    )
}

/// #2/#4: classify a reference from its signals (offline runs stay advisory).
#[must_use]
pub fn classify(reference: &Reference, s: &Signals, use_network: bool) -> RefClass {
    if !use_network {
        return RefClass::NotIndexed;
    }
    // #1: a resolved-but-wrong DOI is fabrication regardless of type.
    if s.doi_metadata_mismatch {
        return RefClass::Fabricated;
    }
    // For an arXiv work, Crossref is expected to miss; OpenAlex/S2 are decisive.
    let matched = if s.is_arxiv {
        !s.openalex_unmatched || !s.semantic_scholar_unmatched
    } else {
        match_count(s) >= 1
    };
    if matched {
        return RefClass::Matched;
    }
    // Unmatched everywhere. An academic-typed reference with no DOI that no index
    // knows is treated as fabricated (ADR-0036); grey literature / arXiv-with-DOI
    // is merely not-indexed; anything else is borderline-suspect.
    if is_academic(reference.ref_type.as_deref()) && reference.doi.is_none() {
        RefClass::Fabricated
    } else if !is_academic(reference.ref_type.as_deref()) || s.is_arxiv {
        RefClass::NotIndexed
    } else {
        RefClass::Suspect
    }
}

/// Parse references from passport `literature_corpus` entries.
pub fn references_from_passport(conn: &Connection, project_id: &str) -> Result<Vec<Reference>> {
    let entries = passport::current(conn, project_id, passport::Section::LiteratureCorpus)?;
    let mut refs = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in entries {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&entry.payload_json) else {
            continue;
        };
        let key = v
            .get("citation_key")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_owned();
        if key.is_empty() || !seen.insert(key.clone()) {
            continue;
        }
        refs.push(Reference {
            citation_key: key,
            title: v.get("title").and_then(|x| x.as_str()).map(str::to_owned),
            year: v
                .get("year")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u16::try_from(n).ok()),
            doi: v.get("doi").and_then(|x| x.as_str()).map(str::to_owned),
            venue: v.get("venue").and_then(|x| x.as_str()).map(str::to_owned),
            ref_type: v.get("type").and_then(|x| x.as_str()).map(str::to_owned),
        });
    }
    Ok(refs)
}

/// Run the contamination check for a project. If `use_network` is false,
/// skips Crossref / OpenAlex / S2 calls (useful in offline tests).
pub async fn run(conn: &Connection, project_id: &str, use_network: bool) -> Result<CheckReport> {
    let refs = references_from_passport(conn, project_id)?;
    if refs.is_empty() {
        return Ok(CheckReport::new(
            "contamination",
            vec![Finding {
                category: "CONTAMINATION".into(),
                severity: Severity::Info,
                message: "no literature_corpus entries to check".into(),
                location: None,
            }],
        ));
    }
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| Client::new());
    let mut findings = Vec::new();
    // #4 (ADR-0026): PRISMA buckets.
    let (mut n_matched, mut n_not_indexed, mut n_suspect, mut n_fabricated) = (0u32, 0, 0, 0);
    // #3 (ADR-0016): per-reference disposition recorded to the passport.
    let mut statuses: Vec<serde_json::Value> = Vec::new();
    for r in &refs {
        let s = signals_for(&client, conn, r, use_network).await;
        let class = classify(r, &s, use_network);
        statuses.push(serde_json::json!({
            "citation_key": r.citation_key,
            "class": class,
            "is_arxiv": s.is_arxiv,
            "doi_metadata_mismatch": s.doi_metadata_mismatch,
            "doi_resolved_title": s.doi_resolved_title,
            "match_count": match_count(&s),
            "preprint_post_2024": s.preprint_post_llm_inflection,
            "checked_at": s.checked_at,
        }));
        match class {
            RefClass::Matched => n_matched += 1,
            RefClass::NotIndexed => n_not_indexed += 1,
            RefClass::Suspect => n_suspect += 1,
            RefClass::Fabricated => n_fabricated += 1,
        }
        // #2: class-aware severity.
        match class {
            RefClass::Fabricated => {
                let msg = if s.doi_metadata_mismatch {
                    format!(
                        "{}: WRONG/FABRICATED DOI -- the supplied DOI resolves to \"{}\", which \
                         does not match this reference (ADR-0036). Correct the DOI or remove.",
                        r.citation_key,
                        s.doi_resolved_title
                            .as_deref()
                            .unwrap_or("a different work")
                    )
                } else {
                    format!(
                        "{}: LIKELY FABRICATED -- academic reference with no DOI, unmatched in \
                         Crossref/OpenAlex/Semantic Scholar (ADR-0036). Verify or remove.",
                        r.citation_key
                    )
                };
                findings.push(Finding {
                    category: "REFERENCE_UNVERIFIED".into(),
                    severity: Severity::Error,
                    message: msg,
                    location: None,
                });
            }
            // #5/#6: borderline → advise cross-model (ADR-0028) / HITL consensus
            // (ADR-0009) rather than blocking on ambiguous evidence.
            RefClass::Suspect => {
                findings.push(Finding {
                    category: "REFERENCE_SUSPECT".into(),
                    severity: Severity::Warn,
                    message: format!(
                        "{}: borderline -- unmatched but not conclusively fabricated. Route to \
                         cross-model (`agentic verify cross-model`) or HITL/consensus before \
                         judging.",
                        r.citation_key
                    ),
                    location: None,
                });
            }
            // #7: an arXiv/grey-lit miss is advisory, not a contamination signal.
            RefClass::NotIndexed if use_network => {
                findings.push(Finding {
                    category: "CONTAMINATION".into(),
                    severity: Severity::Info,
                    message: format!(
                        "{}: not indexed{} -- advisory (grey literature or new preprint).",
                        r.citation_key,
                        if s.is_arxiv {
                            " (arXiv; Crossref miss expected)"
                        } else {
                            ""
                        }
                    ),
                    location: None,
                });
            }
            RefClass::Matched | RefClass::NotIndexed => {}
        }
    }
    // #3: write the per-reference disposition to the passport (HEAD-bound).
    if use_network {
        let head = agentic_core::worktree::head_commit(conn, project_id)
            .ok()
            .flatten()
            .map(|c| c.sha256);
        let payload = serde_json::json!({
            "report": "contamination_status",
            "prisma": { "matched": n_matched, "not_indexed": n_not_indexed,
                        "suspect": n_suspect, "fabricated": n_fabricated },
            "references": statuses,
            "checked_at": Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        });
        let _ = passport::append(
            conn,
            project_id,
            passport::Section::ComplianceReports,
            &payload.to_string(),
            head.as_deref(),
            None,
        );
    }
    // #4: PRISMA summary line (separates not-in-DB from suspect/fabricated).
    findings.push(Finding {
        category: "PRISMA_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "PRISMA: {n_matched} matched, {n_not_indexed} not-indexed (advisory), \
             {n_suspect} suspect, {n_fabricated} fabricated, of {} references.",
            refs.len()
        ),
        location: None,
    });
    warn!(
        refs = refs.len(),
        matched = n_matched,
        not_indexed = n_not_indexed,
        suspect = n_suspect,
        fabricated = n_fabricated,
        "contamination scan complete"
    );
    Ok(CheckReport::new("contamination", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn detects_post_2024_preprint() {
        assert!(is_preprint_post_2024(Some(2024), Some("arXiv")));
        assert!(is_preprint_post_2024(Some(2025), Some("preprints.org")));
        assert!(!is_preprint_post_2024(Some(2023), Some("arXiv")));
        assert!(!is_preprint_post_2024(Some(2024), Some("Nature")));
    }

    #[tokio::test]
    async fn signals_offline_marks_unmatched_when_no_doi() {
        let conn = open_in_memory().unwrap();
        let client = Client::new();
        let r = Reference {
            citation_key: "x2024".into(),
            title: Some("No DOI".into()),
            year: Some(2024),
            doi: None,
            venue: Some("Some Journal".into()),
            ref_type: None,
        };
        let s = signals_for(&client, &conn, &r, false).await;
        assert!(s.crossref_unmatched);
        assert!(s.openalex_unmatched);
        assert!(s.semantic_scholar_unmatched);
    }

    #[tokio::test]
    async fn run_offline_emits_signals_for_corpus_entries() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            passport::Section::LiteratureCorpus,
            r#"{"citation_key":"no_doi_2024","title":"X","year":2024,"venue":"arXiv"}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid, false).await.unwrap();
        // Offline is advisory: per-reference signals are not emitted (they need
        // the network), but the PRISMA summary always reports the disposition —
        // the single arXiv-2024 reference lands in the not-indexed bucket.
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "PRISMA_SUMMARY" && f.message.contains("1 not-indexed"))
        );
    }

    fn rf(key: &str, doi: Option<&str>, venue: Option<&str>, ty: Option<&str>) -> Reference {
        Reference {
            citation_key: key.into(),
            title: Some("A Study of Things".into()),
            year: Some(2025),
            doi: doi.map(str::to_owned),
            venue: venue.map(str::to_owned),
            ref_type: ty.map(str::to_owned),
        }
    }

    #[test]
    fn arxiv_detection_by_doi_or_venue() {
        assert!(is_arxiv_ref(&rf(
            "a",
            Some("10.48550/arXiv.2510.02251"),
            None,
            None
        )));
        assert!(is_arxiv_ref(&rf("a", None, Some("arXiv"), None)));
        assert!(!is_arxiv_ref(&rf(
            "a",
            Some("10.1109/MS.2021.3073045"),
            Some("IEEE"),
            None
        )));
    }

    #[test]
    fn title_similarity_threshold() {
        assert!(titles_consistent(
            "Reproducible Builds for Quantum Computing",
            "Reproducible builds for quantum computing"
        ));
        assert!(!titles_consistent(
            "Reproducible Builds for Quantum Computing",
            "A Taxonomy of Phishing Attacks in Banking"
        ));
        // Empty side cannot be compared → not a mismatch.
        assert!(titles_consistent("", "anything at all here"));
    }

    #[test]
    fn classify_metadata_mismatch_is_fabricated() {
        let mut s = Signals::default();
        s.doi_metadata_mismatch = true;
        assert_eq!(
            classify(
                &rf("x", Some("10.1/wrong"), None, Some("article")),
                &s,
                true
            ),
            RefClass::Fabricated
        );
    }

    #[test]
    fn classify_arxiv_matched_via_openalex_not_fabricated() {
        // arXiv ref: Crossref miss is expected; OpenAlex match makes it Matched.
        let mut s = Signals {
            is_arxiv: true,
            ..Default::default()
        };
        s.crossref_unmatched = true; // expected for arXiv
        s.openalex_unmatched = false; // found
        s.semantic_scholar_unmatched = true;
        assert_eq!(
            classify(
                &rf(
                    "a",
                    Some("10.48550/arXiv.1"),
                    Some("arXiv"),
                    Some("article")
                ),
                &s,
                true
            ),
            RefClass::Matched
        );
    }

    #[test]
    fn classify_academic_no_doi_unmatched_is_fabricated() {
        let s = Signals {
            crossref_unmatched: true,
            openalex_unmatched: true,
            semantic_scholar_unmatched: true,
            ..Default::default()
        };
        assert_eq!(
            classify(&rf("g", None, None, Some("article")), &s, true),
            RefClass::Fabricated
        );
    }

    #[test]
    fn classify_grey_lit_unmatched_is_not_indexed() {
        let s = Signals {
            crossref_unmatched: true,
            openalex_unmatched: true,
            semantic_scholar_unmatched: true,
            ..Default::default()
        };
        assert_eq!(
            classify(&rf("w", None, None, Some("misc")), &s, true),
            RefClass::NotIndexed
        );
    }

    #[test]
    fn classify_offline_is_advisory() {
        let s = Signals::default();
        assert_eq!(
            classify(&rf("x", None, None, Some("article")), &s, false),
            RefClass::NotIndexed
        );
    }

    #[tokio::test]
    async fn run_with_no_corpus_returns_info() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let report = run(&conn, &pid, false).await.unwrap();
        assert_eq!(report.findings.len(), 1);
        assert!(matches!(report.findings[0].severity, Severity::Info));
    }
}
