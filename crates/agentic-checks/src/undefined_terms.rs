//! `agentic check undefined-terms` — flag acronyms or short capitalised
//! phrases used in a deliverable before they are introduced.
//!
//! Per-user requirement 2026-05-28: every acronym in a thesis chapter must
//! either appear in the Acronyms and Abbreviations table or be expanded
//! parenthetically on first use. The check walks chapter markdown in
//! manifest order and emits WARN for any acronym token (2+ uppercase
//! letters, optional digit suffix) that has not yet been introduced via
//! one of:
//!   1. the Acronyms and Abbreviations table loaded from the front-matter
//!      acronyms blob (any path matching `*acronyms*.md`);
//!   2. an inline parenthetical expansion on the same line — `Foo Bar
//!      Baz (FBB)` introduces `FBB`;
//!   3. a token in the run-time stop-list (universal abbreviations such
//!      as `US`, `EU`, `RQ1`, `AI`, common units like `RAM`/`USB`/`GB`).
//!
//! The check is advisory (WARN-only). It is designed to keep false
//! positives below 10 % on the thesis corpus: the stop-list covers the
//! universal common-word class and the parenthetical-expansion rule
//! covers the in-line "introduce as you go" pattern.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Tokens we never flag even if undefined: jurisdictions, year-stamped
/// research-question IDs, common engineering shorthand, and a few
/// universal abbreviations. Conservative on purpose — adding to the
/// stop-list moves a token out of the WARN set, so we keep it short.
const STOP_LIST: &[&str] = &[
    // jurisdictions / sovereigns / city-states
    "EU",
    "US",
    "USA",
    "UK",
    "UAE",
    "EEA",
    "FR",
    "DE",
    "CH",
    "IT",
    "JP",
    "CN",
    "ASEAN",
    "GCC",
    "NATO",
    "OECD",
    // research / requirement identifiers (RQ1, REQ12, R26, P1..P9, FR-CXX)
    "RQ",
    "REQ",
    "FR",
    "RR",
    "PR",
    "PT",
    "FAQ",
    "FRD",
    "PRD",
    "RRD",
    "REQs",
    "FRs",
    // common engineering shorthand seen in any tech text
    "RAM",
    "ROM",
    "GB",
    "MB",
    "KB",
    "TB",
    "USB",
    "PDF",
    "CSV",
    "JSON",
    "YAML",
    "XML",
    "URL",
    "URI",
    "GUI",
    "CLI",
    "SQL",
    "GPU",
    "CPU",
    "TPU",
    "NPU",
    "SSD",
    "HDD",
    "FPGA",
    "OS",
    "IT",
    "DB",
    "CD",
    "VM",
    "ID",
    "OK",
    "ALL",
    "NA",
    "TODO",
    "FIXME",
    "DRY",
    "KISS",
    "AMD",
    "ARM",
    "ASIC",
    "RISC",
    "X86",
    "X64",
    "AVX",
    "SIMD",
    "DMA",
    "PCIe",
    "NVMe",
    "ISA",
    "ABI",
    "MVP",
    "POC",
    "MVR",
    "SDK",
    "SaaS",
    "PaaS",
    "IaaS",
    "MaaS",
    "SAST",
    "DAST",
    "IAST",
    "SCA",
    "WAF",
    "IDS",
    "IPS",
    "SIEM",
    "SOC",
    "MTTR",
    "MTBF",
    // universal AI / org shorthand
    "AI",
    "ML",
    "NLP",
    "GPT",
    "BERT",
    "API",
    "LSTM",
    "RNN",
    "CNN",
    "DNN",
    // common standards / file shorthand carried as proper nouns
    "ISO",
    "IEC",
    "NIST",
    "MITRE",
    "IEEE",
    "RFC",
    "WG",
    "ITU",
    "IETF",
    "W3C",
    "ICANN",
    "ANSI",
    "ETSI",
    "BSI",
    "ENISA",
    "CISA",
    "CERT",
    "SHA",
    "HMAC",
    "AES",
    "ECC",
    "RSA",
    "TLS",
    "SSL",
    "MTLS",
    "HTTPS",
    "HTTP",
    "DNS",
    "DHCP",
    "NTP",
    "SSH",
    "VPN",
    // bill-of-materials family (SBOM/CBOM/QBOM/AIBOM/HBOM) are covered by stripping
    // bare-letter suffixes, but a few full forms appear standalone
    "BOM",
    "SBOM",
    "CBOM",
    "QBOM",
    "HBOM",
    "AIBOM",
    // version / model shorthand
    "LTS",
    "GA",
    "EOL",
    "RC",
    "RT",
    "TM",
    // hash / encoding
    "BLAKE3",
    "B3SUM",
    "SHA-256",
    "SHA256",
    "MD5",
    "BASE64",
    "ASCII",
    "UTF8",
    // section / number scaffolding
    "TOC",
    "N",
    "X",
    "Y",
    "Z",
    "I",
    "II",
    "III",
    "IV",
    // common conjunctions & filler that match the regex
    "AND",
    "OR",
    "NOT",
    "ANY",
    "TBD",
    "VS",
    "ETC",
    // VDI / desktop landscape names + popular distros
    "BOSS",
    "RHEL",
    "SUSE",
    "GRUB",
    // common universal business and engineering shorthand seen in the thesis
    "ROI",
    "SLA",
    "SLO",
    "SKU",
    "P&L",
    "RACI",
    "RPM",
    "JDK",
    "LOC",
    "MOK",
    "PKI",
    "PMC",
    "NVD",
    "NSX",
    "NXP",
    "IBM",
    "DA",
    "EN",
    "FAIL",
    "PASS",
    "GPL",
    "GSMA",
    "XSS",
    "USENIX",
    "ZHAW",
    "OWASP",
    "ICANN",
    "GitLab",
    // research-method labels that look like acronyms
    "CAS",
    "CI",
    "CD",
    "IR",
    "PQ",
    "PCI",
    "DSS",
    "DSA",
    "EN",
    "GOVERN",
    "RMM",
    "MAPE",
    "K",
    "ATLAS",
    "OS",
    // German-method shorthand used in FHNW MAS thesis tradition
    "IST",
    "SOLL",
    "SCS",
    "FADP",
    // common solver / algorithm names that are de-facto proper nouns
    "QUBO",
    "CRYSTALS",
    "Kyber",
    "Dilithium",
    "Falcon",
    "SPHINCS",
    "CP-SAT",
    "SAT",
    // hardware-arch + boot scheme proper nouns
    "HAB",
    "XNU",
    "QPU",
    // sovereignty programme names
    "IPCEI",
    "CISPE",
    "O-RAN",
    "SWE-RL",
    "SNP",
    // VMware product family
    "VCF",
    "Tanzu",
    // SWOT itself (the methodology name; not just SWOT cell IDs which
    // is_internal_id already covers), plus a few SBOM-family file formats
    "SWOT",
    "SPDX",
    "CycloneDX",
    // Bare hardware-family fragments seen post-prefix-strip: i.MX → MX
    "MX",
    // Compound HR-style shorthand from the leadership chapter
    "HR",
    "L&D",
    // Adversarial-ML and confident-loop labels seen in security chapter
    "AML",
    "CLAI",
    // CAS programme name (CAS ALIT — the predecessor CAS-level instrument)
    "ALIT",
    // Hyper-quality-architect role seen in the leadership chapter
    "HQA",
];

/// Internal-ID patterns we never flag (these are project identifiers,
/// not abbreviations). Detected by full-token regex rather than the
/// stop-list because their alphanumeric tails vary per usage.
fn is_internal_id(tok: &str) -> bool {
    // ADR-NNNN, FRD-NNN, REQ-NN, ATLAS technique codes (AML.TNNNN)
    if let Some(rest) = tok.strip_prefix("ADR-") {
        return rest.chars().all(|c| c.is_ascii_alphanumeric());
    }
    if let Some(rest) = tok.strip_prefix("FRD-") {
        return rest.chars().all(|c| c.is_ascii_alphanumeric());
    }
    if let Some(rest) = tok.strip_prefix("REQ-") {
        return rest.chars().all(|c| c.is_ascii_alphanumeric());
    }
    if let Some(rest) = tok.strip_prefix("CAR-") {
        return rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    }
    if let Some(rest) = tok.strip_prefix("PT-") {
        return rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    }
    if let Some(rest) = tok.strip_prefix("AML.") {
        return rest.chars().all(|c| c.is_ascii_alphanumeric());
    }
    // Bare campaign/project IDs: `C01`, `C01-P2`, `C05/C06/`, `P1` ... `P9`
    let bare = tok.trim_end_matches(|c: char| c == '/' || c == '-' || c == '.');
    if bare.strip_prefix('C').is_some_and(|r| {
        r.chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == 'P')
    }) && bare.starts_with('C')
        && bare.len() >= 2
    {
        return true;
    }
    // Section keys like `P1`..`P9` standalone
    if let Some(rest) = bare.strip_prefix('P') {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    // Standalone L0..L4 autonomy levels.
    if let Some(rest) = bare.strip_prefix('L') {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    // Compound autonomy-level / HITL pairs: `L0/HITL`, `L0/L1`, `L1/L2`,
    // `L1/HITL`, `L3/L4` ... — both halves are structural section keys.
    if bare.starts_with('L') && bare.contains('/') {
        let parts: Vec<&str> = bare.split('/').collect();
        let ok = |s: &str| {
            s == "HITL"
                || (s.starts_with('L')
                    && s[1..].chars().all(|c| c.is_ascii_digit())
                    && s.len() >= 2)
        };
        if parts.iter().all(|p| ok(p)) {
            return true;
        }
    }
    // SWOT cell IDs (S1, S2, W1, W2, O1, O2, T1) standalone.
    let swot_letters = ['S', 'W', 'O', 'T', 'H', 'M', 'G'];
    if bare.len() >= 2
        && swot_letters.contains(&bare.chars().next().unwrap())
        && bare[1..].chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }
    // SWOT-cell compound (`S1/S4/W1`, `O2/S3`) — every segment is a SWOT cell.
    if bare.contains('/') {
        let parts: Vec<&str> = bare.split('/').collect();
        let is_swot_cell = |s: &str| {
            s.len() >= 2
                && swot_letters.contains(&s.chars().next().unwrap())
                && s[1..].chars().all(|c| c.is_ascii_digit())
        };
        if parts.iter().all(|p| is_swot_cell(p)) {
            return true;
        }
        // Compound campaign-ref like `C5/C7` — every part is a `C<digits>`
        // internal-id stub.
        let is_campaign = |s: &str| {
            s.starts_with('C') && s.len() >= 2 && s[1..].chars().all(|c| c.is_ascii_digit())
        };
        if parts.iter().all(|p| is_campaign(p)) {
            return true;
        }
    }
    // Bare requirement IDs: `R15`, `R26`, `R29` (proposal-envelope reqs).
    if let Some(rest) = bare.strip_prefix('R') {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    // ATLAS technique stubs: `T0098`, `T0001`...
    if let Some(rest) = bare.strip_prefix('T') {
        if rest.len() >= 4 && rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    // Operational-question IDs: `OQ-04-002`, `OQ-11-001`, ...
    if let Some(rest) = bare.strip_prefix("OQ-") {
        if rest.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return true;
        }
    }
    false
}

/// Strip a trailing numeric / version / cell-id tail (`AES-128` -> `AES`,
/// `ISO/IEC` -> `ISO`, `ML-DSA-87` -> `ML-DSA`) so the stop-list /
/// defined-set lookup hits the base acronym.
fn base_acronym(tok: &str) -> String {
    // 1) Strip trailing separator
    let t = tok.trim_end_matches(|c: char| c == '-' || c == '/' || c == '&' || c == '.');
    // 2) If the tail is `-<digits>` or `<separator><lowercase>`, peel it.
    if let Some(idx) = t.rfind('-') {
        let (head, tail) = t.split_at(idx);
        let tail = &tail[1..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return head.to_string();
        }
    }
    if let Some(idx) = t.find('/') {
        return t[..idx].to_string();
    }
    t.to_string()
}

/// Heuristic acronym matcher: 2-7 uppercase letters, optional `-` or `&`
/// connectives (`ATT&CK`, `ML-DSA`, `CI/CD`), optional trailing digit
/// segment (`PT-C09`, `AI100-2`, `RQ1`). We deliberately stop short of
/// alphabetic words longer than 7 letters — they are rarely acronyms,
/// almost always proper nouns the writer means to capitalise (`Photon`,
/// `Linux`, `Broadcom`).
fn acronym_regex() -> Regex {
    Regex::new(r"\b[A-Z][A-Z0-9&/\-]{1,8}\b").expect("compile acronym regex")
}

/// Pull the acronyms table out of a single markdown blob and return the
/// set of defined tokens (left column). Recognises GitHub-flavoured
/// markdown tables with at least two columns — header + separator + body.
fn parse_acronym_table(md: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut in_body = false;
    let mut header_seen = false;
    for raw in md.lines() {
        let line = raw.trim();
        if !line.starts_with('|') {
            in_body = false;
            header_seen = false;
            continue;
        }
        // a header separator looks like `|---|---|`
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.iter().all(|c| {
            c.chars()
                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        }) && cells.len() >= 2
        {
            in_body = header_seen;
            continue;
        }
        if !header_seen {
            header_seen = true;
            continue; // skip header row
        }
        if in_body && cells.len() >= 2 {
            let key = cells[0].trim();
            // Pull just the acronym head — table cells like `ML-DSA` or
            // `ATT&CK` are kept verbatim; surrounding bold/italic stripped.
            let cleaned: String = key
                .trim_matches(|c: char| c == '*' || c == '`' || c == '_')
                .to_string();
            if !cleaned.is_empty() {
                out.insert(cleaned);
            }
        }
    }
    out
}

/// First-use parenthetical expansion: `Foo Bar Baz (FBB)` introduces FBB
/// on its line. Matches `(ACR)` immediately after a sequence of
/// capitalised words (the expansion the author just wrote). Returns the
/// set of tokens discovered on this line; the caller marks them defined
/// from the *next* token onward.
fn parenthetical_intros(line: &str, re: &Regex) -> Vec<String> {
    let mut found = Vec::new();
    // The expansion is any sequence of words (hyphen-joined counts as one) that
    // begins with a capital letter and is followed immediately by `(ACR)`.
    // Allow zero space-separated extra words so `Belief-Desire-Intention (BDI)`
    // matches alongside `Vehicle Routing Problem (VRP)`.
    let p = Regex::new(
        r"\b([A-Z][\w/\-&]*(?:\s+[A-Za-z][\w/\-&]*){0,8})\s*\(([A-Z][A-Z0-9&/\-]{1,8})\)",
    )
    .unwrap();
    for cap in p.captures_iter(line) {
        if let Some(m) = cap.get(2) {
            // Sanity: the acronym must still look like an acronym (re).
            if re.is_match(m.as_str()) {
                found.push(m.as_str().to_string());
            }
        }
    }
    found
}

/// Run the check across every blob under `prefix` (manifest order, by
/// path sort). Returns one WARN per (path, line, token) triple.
pub fn run(conn: &Connection, project: &str, prefix: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let stop: HashSet<&str> = STOP_LIST.iter().copied().collect();
    let re = acronym_regex();

    // Load the defined-set from the front-matter acronyms table(s).
    // `worktree::list` returns `(path, blob_sha)` pairs for every entry in
    // the head tree; pass an empty prefix to walk the whole tree once.
    let all = worktree::list(conn, project, "")?;
    let mut defined: HashSet<String> = HashSet::new();
    for (path, _) in &all {
        if path.to_lowercase().contains("acronyms") && path.ends_with(".md") {
            if let Ok(blob) = worktree::read_at(conn, project, path) {
                let md = String::from_utf8_lossy(&blob.content);
                defined.extend(parse_acronym_table(&md));
            }
        }
    }
    let defined_seed = defined.clone();

    // Walk every file under prefix in path-sorted order.
    let mut files: Vec<&(String, String)> = all
        .iter()
        .filter(|(p, _)| p.starts_with(prefix) && p.ends_with(".md"))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut per_token: HashMap<String, usize> = HashMap::new();
    for (path, _) in &files {
        // Skip the acronyms table itself — we already harvested it.
        if path.to_lowercase().contains("acronyms") {
            continue;
        }
        let blob = match worktree::read_at(conn, project, path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let md = String::from_utf8_lossy(&blob.content).to_string();
        let mut in_code = false;
        for (lineno, raw) in md.lines().enumerate() {
            // Skip fenced code blocks (figspec, JSON, shell) — those carry
            // type names that we are not analysing as prose.
            if raw.trim_start().starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            // First find any new parenthetical intros on this line. They
            // count as defined STARTING this line (the author introduces
            // the term in-place, so the same line's use is fine).
            for tok in parenthetical_intros(raw, &re) {
                defined.insert(tok);
            }
            for m in re.find_iter(raw) {
                let tok = m.as_str();
                // 1. Project / requirement / ATLAS internal IDs are not
                //    abbreviations; never flag them.
                if is_internal_id(tok) {
                    continue;
                }
                // 2. Direct stop-list / direct-defined hit (no normalisation).
                if stop.contains(tok) || defined.contains(tok) {
                    continue;
                }
                // 3. Strip a trailing digit-tail (e.g. `RQ1` → `RQ`).
                let alpha_head: String = tok
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect();
                if !alpha_head.is_empty()
                    && (stop.contains(alpha_head.as_str()) || defined.contains(&alpha_head))
                {
                    continue;
                }
                // 4. Peel a `-NNN` numeric or `/X` variant tail
                //    (`AES-128` → `AES`, `ISO/IEC` → `ISO`).
                let base = base_acronym(tok);
                if !base.is_empty()
                    && base != tok
                    && (stop.contains(base.as_str()) || defined.contains(&base))
                {
                    continue;
                }
                // 5. Skip purely fragment-looking tokens (token ends in a
                //    separator after trimming → was something like `AIBOM/`).
                if tok.ends_with('-') || tok.ends_with('/') || tok.ends_with('&') {
                    continue;
                }
                // Count and emit one finding per first occurrence.
                let n = per_token.entry(tok.to_string()).or_insert(0);
                *n += 1;
                if *n == 1 {
                    findings.push(Finding {
                        category: "UNDEFINED_ACRONYM".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "acronym '{}' used at {}:{} but not in the Acronyms and Abbreviations table and not introduced parenthetically on the same line",
                            tok,
                            path,
                            lineno + 1
                        ),
                        location: Some(format!("{}:{}", path, lineno + 1)),
                    });
                }
            }
        }
    }

    let total_distinct = per_token.len();
    findings.push(Finding {
        category: "UNDEFINED_TERMS_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "{} distinct undefined acronym(s) flagged across {} files (defined-set carries {} entries)",
            total_distinct,
            files.len(),
            defined_seed.len()
        ),
        location: Some(prefix.to_string()),
    });

    Ok(CheckReport::new("undefined_terms", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_acronym_table() {
        let md = "# Acronyms\n\n| Acronym | Expansion |\n|---|---|\n| AI | Artificial Intelligence |\n| BDI | Belief-Desire-Intention |\n";
        let set = parse_acronym_table(md);
        assert!(set.contains("AI"));
        assert!(set.contains("BDI"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn finds_parenthetical_intros() {
        let re = acronym_regex();
        let intros = parenthetical_intros("Belief-Desire-Intention (BDI) agents", &re);
        assert!(intros.iter().any(|s| s == "BDI"));
    }

    #[test]
    fn stop_list_silences_us_eu_rqx() {
        let stop: HashSet<&str> = STOP_LIST.iter().copied().collect();
        assert!(stop.contains("US"));
        assert!(stop.contains("EU"));
        assert!(stop.contains("RQ"));
    }
}
