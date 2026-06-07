//! Regulation-timeline figure (port of `regulation_timeline_v3_kit`).
//!
//! Ports `scripts/_render_regulation_timeline_v3.py` from the
//! `inbox/regulation_timeline_v3_kit/` source to native Rust so the
//! thesis no longer carries a Python dependency at render time. The
//! kit's reference output (`output_reference/regulation_timeline_v3*.png`)
//! is the acceptance gate: every per-language render produced here must
//! be visually equivalent to the corresponding reference PNG.
//!
//! This first commit ships the **data layer** in full plus a stub
//! rendering API. The three panels (A density / B hot-spots / C Gantt)
//! and the `LANGUAGES` per-language string table follow in subsequent
//! commits — the data is the half of the port that is content-stable;
//! the rendering is the half that needs pixel-parity iteration against
//! `plotters` (where matplotlib's defaults differ).
//!
//! Data lineage:
//! - `REGS` / `MUTUAL` / `CONFLICTS` are curated against the kit's
//!   `data/_regulations.json` (32 anchors, 150 mentions with
//!   ±180-char context) plus external research on announcement /
//!   enforcement / sunset years (see `sources.md` in the kit).
//! - Field schema (in python tuple order):
//!   `(label, goal_key, jur, pub, applies, sunset, milestones, note,
//!     sector, et, teeth, cadence, chapters)`.

#![allow(dead_code)] // Renderers land in follow-up commits.

use std::path::Path;

use anyhow::{Result, bail};

// ===========================================================================
// X-axis bounds (Panels A / B / C all share the same time range).
// ===========================================================================

pub const X_LO: i32 = 2007;
pub const X_HI: i32 = 2036;

// ===========================================================================
// Goal taxonomy (drives Panel B Y-axis order and Panel C grouping).
// ===========================================================================

pub const GOAL_KEYS: &[&str] = &[
    "data_protection",
    "cybersecurity",
    "operational_resilience",
    "critical_infra",
    "sovereign_cloud",
    "ai_governance",
    "crypto_module",
    "pqc_migration",
    "tls_cert_lifetime",
    "classified_info",
    "qkd_quantum_channels",
    "methodology",
];

// ===========================================================================
// Regulation struct + table.
// ===========================================================================

#[derive(Debug, Clone, Copy)]
pub struct Regulation {
    /// Canonical regulation label — used as the join key for
    /// `MUTUAL` and `CONFLICTS`. Must be unique across `REGS`.
    pub label: &'static str,
    /// Goal-taxonomy key (must appear in [`GOAL_KEYS`]).
    pub goal_key: &'static str,
    /// Jurisdiction key (must appear in [`COLOURS`]).
    pub jur: &'static str,
    /// Year the instrument was first published (fade-in start).
    pub pub_year: i32,
    /// Year the instrument first applies (in-force start, end of fade-in).
    pub applies_year: i32,
    /// Optional sunset year (fade-out start; `None` = open-ended).
    pub sunset_year: Option<i32>,
    /// Hard-deadline milestones inside the bar (year + short label).
    pub milestones: &'static [(i32, &'static str)],
    /// Short free-text note (used for the Panel-C row title tooltip).
    pub note: &'static str,
    /// Sector codes (HOR / FIN / IND / DEF / GOV / RET / RES …).
    pub sector: &'static [&'static str],
    /// Extraterritorial reach (true = applies beyond jurisdiction borders).
    pub et: bool,
    /// Enforcement teeth (1 = soft / 2 = mixed / 3 = hard).
    pub teeth: u8,
    /// Update cadence: `"stat"` (statutory amend), `"rev"` (revision),
    /// `"ann"` (annual edition).
    pub cadence: &'static str,
    /// Thesis chapter citations.
    pub chapters: &'static [u8],
}

/// Curated regulation table (32 entries; matches the python `REGS` list
/// 1-for-1 — same labels, same fields, same order).
pub const REGS: &[Regulation] = &[
    // ---- Data protection ----
    Regulation {
        label: "GDPR (Reg 2016/679)",
        goal_key: "data_protection",
        jur: "EU",
        pub_year: 2016,
        applies_year: 2018,
        sunset_year: None,
        milestones: &[],
        note: "baseline data protection",
        sector: &["HOR"],
        et: true,
        teeth: 3,
        cadence: "stat",
        chapters: &[1, 2, 3],
    },
    Regulation {
        label: "Swiss revFADP",
        goal_key: "data_protection",
        jur: "CH",
        pub_year: 2020,
        applies_year: 2023,
        sunset_year: None,
        milestones: &[(2023, "1 Sep 2023: in force")],
        note: "data protection (revised)",
        sector: &["HOR", "FIN"],
        et: true,
        teeth: 3,
        cadence: "stat",
        chapters: &[1, 7],
    },
    // ---- Cybersecurity / supply chain integrity ----
    Regulation {
        label: "US EO 14028",
        goal_key: "cybersecurity",
        jur: "US",
        pub_year: 2021,
        applies_year: 2021,
        sunset_year: None,
        milestones: &[],
        note: "federal cybersecurity / SBOM",
        sector: &["GOV", "HOR"],
        et: true,
        teeth: 2,
        cadence: "rev",
        chapters: &[1, 2, 3, 5],
    },
    Regulation {
        label: "EU CRA (Reg 2024/2847)",
        goal_key: "cybersecurity",
        jur: "EU",
        pub_year: 2024,
        applies_year: 2027,
        sunset_year: None,
        milestones: &[
            (2026, "11 Dec 2026: reporting"),
            (2027, "11 Dec 2027: full apply"),
        ],
        note: "security-by-design for digital products",
        sector: &["HOR"],
        et: true,
        teeth: 3,
        cadence: "stat",
        chapters: &[1, 2, 3, 5, 7],
    },
    Regulation {
        label: "CERT-In v2.0",
        goal_key: "cybersecurity",
        jur: "IN",
        pub_year: 2022,
        applies_year: 2025,
        sunset_year: None,
        milestones: &[(2025, "2025: v2.0 effective")],
        note: "incident reporting / SBOM",
        sector: &["HOR"],
        et: true,
        teeth: 2,
        cadence: "rev",
        chapters: &[1, 3, 5],
    },
    Regulation {
        label: "PCI DSS v4.0.1",
        goal_key: "cybersecurity",
        jur: "Global",
        pub_year: 2024,
        applies_year: 2025,
        sunset_year: None,
        milestones: &[(2025, "31 Mar 2025: enforced")],
        note: "payment-card security",
        sector: &["RET", "FIN"],
        et: true,
        teeth: 3,
        cadence: "rev",
        chapters: &[1, 3, 5],
    },
    // ---- Operational resilience ----
    Regulation {
        label: "EU DORA (Reg 2022/2554)",
        goal_key: "operational_resilience",
        jur: "EU",
        pub_year: 2022,
        applies_year: 2025,
        sunset_year: None,
        milestones: &[(2025, "17 Jan 2025: applies")],
        note: "financial-sector ICT resilience",
        sector: &["FIN"],
        et: true,
        teeth: 3,
        cadence: "stat",
        chapters: &[1, 3, 5, 7],
    },
    Regulation {
        label: "FINMA Circular 2023/1",
        goal_key: "operational_resilience",
        jur: "CH",
        pub_year: 2023,
        applies_year: 2024,
        sunset_year: None,
        milestones: &[],
        note: "operational risk & resilience",
        sector: &["FIN"],
        et: false,
        teeth: 3,
        cadence: "rev",
        chapters: &[5, 7],
    },
    Regulation {
        label: "FINMA Guidance 05/2025",
        goal_key: "operational_resilience",
        jur: "CH",
        pub_year: 2025,
        applies_year: 2025,
        sunset_year: None,
        milestones: &[],
        note: "sectoral ICT/AI guidance",
        sector: &["FIN"],
        et: false,
        teeth: 2,
        cadence: "rev",
        chapters: &[7],
    },
    // ---- Critical-infrastructure baselines ----
    Regulation {
        label: "EU NIS Coop crypto-inventory",
        goal_key: "critical_infra",
        jur: "EU",
        pub_year: 2024,
        applies_year: 2026,
        sunset_year: None,
        milestones: &[(2026, "31 Dec 2026: inventory due")],
        note: "cryptographic inventories",
        sector: &["IND", "HOR"],
        et: true,
        teeth: 2,
        cadence: "rev",
        chapters: &[5],
    },
    Regulation {
        label: "BACS IKT-Minimalstandard",
        goal_key: "critical_infra",
        jur: "CH",
        pub_year: 2018,
        applies_year: 2018,
        sunset_year: None,
        milestones: &[],
        note: "critical-infrastructure ICT baseline",
        sector: &["IND"],
        et: false,
        teeth: 1,
        cadence: "stat",
        chapters: &[7],
    },
    // ---- Sovereign cloud ----
    Regulation {
        label: "EU SCS / CISPE codes",
        goal_key: "sovereign_cloud",
        jur: "EU",
        pub_year: 2021,
        applies_year: 2023,
        sunset_year: None,
        milestones: &[],
        note: "sovereign-cloud reference stacks",
        sector: &["HOR", "GOV"],
        et: false,
        teeth: 1,
        cadence: "rev",
        chapters: &[2, 5, 7],
    },
    Regulation {
        label: "EU IPCEI-CIS",
        goal_key: "sovereign_cloud",
        jur: "EU",
        pub_year: 2022,
        applies_year: 2023,
        sunset_year: None,
        milestones: &[],
        note: "sovereign cloud industrial policy",
        sector: &["HOR", "GOV"],
        et: false,
        teeth: 1,
        cadence: "stat",
        chapters: &[2, 5, 7],
    },
    // ---- AI governance ----
    Regulation {
        label: "NIST AI RMF 1.0 (NIST AI 100-1)",
        goal_key: "ai_governance",
        jur: "US",
        pub_year: 2023,
        applies_year: 2023,
        sunset_year: None,
        milestones: &[],
        note: "Govern-Map-Measure-Manage",
        sector: &["HOR"],
        et: false,
        teeth: 1,
        cadence: "rev",
        chapters: &[1, 2, 5],
    },
    Regulation {
        label: "NIST AI 100-2 (adversarial-ML)",
        goal_key: "ai_governance",
        jur: "US",
        pub_year: 2024,
        applies_year: 2024,
        sunset_year: None,
        milestones: &[],
        note: "evasion/poisoning/extraction taxonomy",
        sector: &["HOR"],
        et: false,
        teeth: 1,
        cadence: "rev",
        chapters: &[2, 5],
    },
    Regulation {
        label: "ISO/IEC 42001:2023 (AIMS)",
        goal_key: "ai_governance",
        jur: "Intl",
        pub_year: 2023,
        applies_year: 2024,
        sunset_year: None,
        milestones: &[(2024, "2024: certification starts")],
        note: "AI management system standard",
        sector: &["HOR"],
        et: false,
        teeth: 2,
        cadence: "rev",
        chapters: &[1, 2, 5, 7],
    },
    Regulation {
        label: "US AI EO (EO 14179 / 14365)",
        goal_key: "ai_governance",
        jur: "US",
        pub_year: 2025,
        applies_year: 2025,
        sunset_year: None,
        milestones: &[],
        note: "federally permissive AI directives",
        sector: &["GOV"],
        et: true,
        teeth: 2,
        cadence: "rev",
        chapters: &[1, 5],
    },
    Regulation {
        label: "EU AI Act",
        goal_key: "ai_governance",
        jur: "EU",
        pub_year: 2024,
        applies_year: 2026,
        sunset_year: None,
        milestones: &[
            (2026, "2 Aug 2026: marking duties"),
            (2027, "2027: high-risk obligations"),
        ],
        note: "AI lifecycle obligations",
        sector: &["HOR"],
        et: true,
        teeth: 3,
        cadence: "stat",
        chapters: &[1, 2, 5, 7],
    },
    // ---- Cryptographic module validation ----
    Regulation {
        label: "FIPS 140-3 + ESV (mandatory)",
        goal_key: "crypto_module",
        jur: "US",
        pub_year: 2019,
        applies_year: 2022,
        sunset_year: None,
        milestones: &[(2022, "Oct 2022: ESV-cert submission")],
        note: "cryptographic module validation",
        sector: &["DEF", "GOV", "FIN"],
        et: true,
        teeth: 3,
        cadence: "rev",
        chapters: &[5, 7],
    },
    Regulation {
        label: "NIST CMVP / SP 800-90B (entropy)",
        goal_key: "crypto_module",
        jur: "US",
        pub_year: 2018,
        applies_year: 2022,
        sunset_year: None,
        milestones: &[],
        note: "entropy-source validation",
        sector: &["DEF", "GOV"],
        et: true,
        teeth: 2,
        cadence: "rev",
        chapters: &[5, 7],
    },
    Regulation {
        label: "ANSSI CSPN",
        goal_key: "crypto_module",
        jur: "FR",
        pub_year: 2008,
        applies_year: 2009,
        sunset_year: None,
        milestones: &[],
        note: "first-level security certification",
        sector: &["DEF", "GOV"],
        et: false,
        teeth: 2,
        cadence: "rev",
        chapters: &[7],
    },
    // ---- PQC migration ----
    Regulation {
        label: "FIPS 203/204/205 (ML-KEM/DSA/SLH-DSA)",
        goal_key: "pqc_migration",
        jur: "US",
        pub_year: 2024,
        applies_year: 2024,
        sunset_year: None,
        milestones: &[],
        note: "PQC standardised replacements",
        sector: &["HOR", "DEF"],
        et: true,
        teeth: 2,
        cadence: "stat",
        chapters: &[2, 5],
    },
    Regulation {
        label: "NIST IR 8547",
        goal_key: "pqc_migration",
        jur: "US",
        pub_year: 2024,
        applies_year: 2024,
        sunset_year: Some(2035),
        milestones: &[
            (2030, "2030: deprecate classical"),
            (2035, "2035: disallow classical"),
        ],
        note: "PQC transition timetable",
        sector: &["HOR", "DEF", "GOV"],
        et: true,
        teeth: 2,
        cadence: "rev",
        chapters: &[2, 5, 7],
    },
    Regulation {
        label: "CNSA 2.0",
        goal_key: "pqc_migration",
        jur: "US",
        pub_year: 2022,
        applies_year: 2027,
        sunset_year: Some(2033),
        milestones: &[
            (2027, "Jan 2027: procurement gate"),
            (2030, "2030: hybrid"),
            (2033, "2033: PQC-only"),
        ],
        note: "national-security PQC suite",
        sector: &["DEF", "GOV"],
        et: true,
        teeth: 3,
        cadence: "rev",
        chapters: &[2, 5, 7],
    },
    Regulation {
        label: "BSI TR-02102 KRITIS hybrid-by-2030",
        goal_key: "pqc_migration",
        jur: "DE",
        pub_year: 2024,
        applies_year: 2030,
        sunset_year: None,
        milestones: &[(2030, "2030: hybrid required")],
        note: "KRITIS PQC migration line",
        sector: &["IND", "GOV"],
        et: false,
        teeth: 2,
        cadence: "ann",
        chapters: &[5, 7],
    },
    Regulation {
        label: "BSI TR-02102-1 (annual)",
        goal_key: "pqc_migration",
        jur: "DE",
        pub_year: 2008,
        applies_year: 2026,
        sunset_year: None,
        milestones: &[(2026, "Jan 2026 edition")],
        note: "cryptographic recommendations",
        sector: &["IND", "GOV", "FIN"],
        et: false,
        teeth: 2,
        cadence: "ann",
        chapters: &[5, 7],
    },
    // ---- TLS / cert lifetime ----
    Regulation {
        label: "TLS 47-day certificate lifetime",
        goal_key: "tls_cert_lifetime",
        jur: "Global",
        pub_year: 2024,
        applies_year: 2027,
        sunset_year: Some(2033),
        milestones: &[
            (2027, "2027: start"),
            (2033, "2033: PQC-only target"),
        ],
        note: "CA/Browser-Forum cert lifetime",
        sector: &["HOR"],
        et: true,
        teeth: 2,
        cadence: "stat",
        chapters: &[2, 5, 7],
    },
    // ---- Classified-info handling / federal authorisation ----
    Regulation {
        label: "BSI VS-NfD",
        goal_key: "classified_info",
        jur: "DE",
        pub_year: 2007,
        applies_year: 2007,
        sunset_year: None,
        milestones: &[],
        note: "classified-information handling level",
        sector: &["DEF", "GOV"],
        et: false,
        teeth: 2,
        cadence: "stat",
        chapters: &[5, 7],
    },
    Regulation {
        label: "FedRAMP High",
        goal_key: "classified_info",
        jur: "US",
        pub_year: 2011,
        applies_year: 2014,
        sunset_year: None,
        milestones: &[],
        note: "federal cloud authorisation",
        sector: &["GOV"],
        et: true,
        teeth: 3,
        cadence: "rev",
        chapters: &[5, 7],
    },
    // ---- QKD ----
    Regulation {
        label: "ETSI GS QKD 014",
        goal_key: "qkd_quantum_channels",
        jur: "Intl",
        pub_year: 2019,
        applies_year: 2019,
        sunset_year: None,
        milestones: &[],
        note: "QKD REST-based key-delivery API",
        sector: &["HOR", "RES"],
        et: false,
        teeth: 1,
        cadence: "rev",
        chapters: &[5, 7],
    },
    // ---- Methodology ----
    Regulation {
        label: "PRISMA 2020 (reporting std)",
        goal_key: "methodology",
        jur: "Intl",
        pub_year: 2020,
        applies_year: 2021,
        sunset_year: None,
        milestones: &[],
        note: "systematic-review reporting baseline",
        sector: &["RES"],
        et: false,
        teeth: 1,
        cadence: "stat",
        chapters: &[1, 2],
    },
    // ---- Extra-territorial / trade-secrets counterparties (referenced
    //      from Panel-C conflicts; needed so both sides are present) ----
    Regulation {
        label: "US CLOUD Act",
        goal_key: "data_protection",
        jur: "US",
        pub_year: 2018,
        applies_year: 2018,
        sunset_year: None,
        milestones: &[],
        note: "US extraterritorial data-access law (counterparty to GDPR sovereignty)",
        sector: &["HOR", "GOV"],
        et: true,
        teeth: 3,
        cadence: "stat",
        chapters: &[1, 5, 7],
    },
    Regulation {
        label: "EU Trade Secrets Directive (2016/943)",
        goal_key: "data_protection",
        jur: "EU",
        pub_year: 2016,
        applies_year: 2018,
        sunset_year: None,
        milestones: &[(2018, "9 Jun 2018: transposition deadline")],
        note: "trade-secret protection (counterparty to AI Act provenance)",
        sector: &["HOR"],
        et: true,
        teeth: 2,
        cadence: "stat",
        chapters: &[2, 5],
    },
];

// ===========================================================================
// Mutual-recognition arcs.
// ===========================================================================

#[derive(Debug, Clone, Copy)]
pub struct MutualRecognition {
    pub src: &'static str,
    pub tgt: &'static str,
    /// Arc strength (1 = weak / 2 = mid / 3 = strong).
    pub strength: u8,
}

pub const MUTUAL: &[MutualRecognition] = &[
    MutualRecognition { src: "ISO/IEC 42001:2023 (AIMS)", tgt: "EU AI Act", strength: 3 },
    MutualRecognition { src: "ISO/IEC 42001:2023 (AIMS)", tgt: "NIST AI RMF 1.0 (NIST AI 100-1)", strength: 2 },
    MutualRecognition { src: "ISO/IEC 42001:2023 (AIMS)", tgt: "FINMA Guidance 05/2025", strength: 2 },
    MutualRecognition { src: "NIST AI RMF 1.0 (NIST AI 100-1)", tgt: "EU AI Act", strength: 2 },
    MutualRecognition { src: "NIST AI 100-2 (adversarial-ML)", tgt: "EU AI Act", strength: 2 },
    MutualRecognition { src: "FIPS 140-3 + ESV (mandatory)", tgt: "BSI VS-NfD", strength: 2 },
    MutualRecognition { src: "FIPS 140-3 + ESV (mandatory)", tgt: "ANSSI CSPN", strength: 2 },
    MutualRecognition { src: "FIPS 140-3 + ESV (mandatory)", tgt: "FedRAMP High", strength: 3 },
    MutualRecognition { src: "CNSA 2.0", tgt: "BSI TR-02102 KRITIS hybrid-by-2030", strength: 2 },
    MutualRecognition { src: "CNSA 2.0", tgt: "NIST IR 8547", strength: 3 },
    MutualRecognition { src: "NIST IR 8547", tgt: "BSI TR-02102 KRITIS hybrid-by-2030", strength: 2 },
    MutualRecognition { src: "NIST IR 8547", tgt: "TLS 47-day certificate lifetime", strength: 2 },
    MutualRecognition { src: "EU CRA (Reg 2024/2847)", tgt: "EU NIS Coop crypto-inventory", strength: 2 },
    MutualRecognition { src: "EU CRA (Reg 2024/2847)", tgt: "CERT-In v2.0", strength: 1 },
    MutualRecognition { src: "EU CRA (Reg 2024/2847)", tgt: "PCI DSS v4.0.1", strength: 1 },
    MutualRecognition { src: "US EO 14028", tgt: "EU CRA (Reg 2024/2847)", strength: 1 },
    MutualRecognition { src: "US EO 14028", tgt: "CERT-In v2.0", strength: 1 },
    MutualRecognition { src: "EU DORA (Reg 2022/2554)", tgt: "FINMA Circular 2023/1", strength: 3 },
    MutualRecognition { src: "EU DORA (Reg 2022/2554)", tgt: "FINMA Guidance 05/2025", strength: 2 },
    MutualRecognition { src: "GDPR (Reg 2016/679)", tgt: "Swiss revFADP", strength: 3 },
];

// ===========================================================================
// Conflict markers.
// ===========================================================================

#[derive(Debug, Clone, Copy)]
pub struct ConflictMarker {
    pub label: &'static str,
    pub year: i32,
    pub text: &'static str,
}

/// Each of the 5 conflicts is represented TWICE — once on each side —
/// so both regulations carry the ⚡X marker at the same conflict year,
/// making the pairing visible in Panel C.
pub const CONFLICTS: &[ConflictMarker] = &[
    // 1. EU CRA transparency ↔ BSI VS-NfD classified secrecy (in force 2027)
    ConflictMarker { label: "EU CRA (Reg 2024/2847)", year: 2027, text: "transparency ↔ classified secrecy (BSI VS-NfD)" },
    ConflictMarker { label: "BSI VS-NfD", year: 2027, text: "classified secrecy ↔ CRA transparency" },
    // 2. GDPR data sovereignty ↔ US CLOUD Act reach (in force 2018)
    ConflictMarker { label: "GDPR (Reg 2016/679)", year: 2018, text: "data sovereignty ↔ US CLOUD Act reach" },
    ConflictMarker { label: "US CLOUD Act", year: 2018, text: "US extraterritorial reach ↔ GDPR sovereignty" },
    // 3. EU AI Act provenance ↔ EU Trade Secrets Directive (in force 2026)
    ConflictMarker { label: "EU AI Act", year: 2026, text: "provenance ↔ trade-secret protection" },
    ConflictMarker { label: "EU Trade Secrets Directive (2016/943)", year: 2026, text: "trade-secret protection ↔ AI Act provenance" },
    // 4. CNSA 2.0 PQC-only ↔ BSI KRITIS hybrid (in force 2033)
    ConflictMarker { label: "CNSA 2.0", year: 2033, text: "PQC-only ↔ DE hybrid-only (KRITIS)" },
    ConflictMarker { label: "BSI TR-02102 KRITIS hybrid-by-2030", year: 2033, text: "DE hybrid-only ↔ US CNSA PQC-only" },
    // 5. PCI DSS retention ↔ GDPR right-to-erasure (in force 2025)
    ConflictMarker { label: "PCI DSS v4.0.1", year: 2025, text: "retention ↔ GDPR right-to-erasure" },
    ConflictMarker { label: "GDPR (Reg 2016/679)", year: 2025, text: "right-to-erasure ↔ PCI DSS retention" },
];

// ===========================================================================
// Per-jurisdiction colour swatches.
// ===========================================================================

/// (jurisdiction key, #RRGGBB).
///
/// Country palette deliberately stays OUT of the red/orange/pink/gray/black
/// band reserved for the six signalisation cues (pre-effective gray,
/// in-force gray, hard-deadline black, hot-spot `#ff4500`, conflict `#ff0066`,
/// mutual-recognition arc `#444444`). All swatches are blues, purple, gold,
/// teal, green, slate, or warm brown so the legend doesn't collide with cue
/// meaning.
pub const COLOURS: &[(&str, &str)] = &[
    ("EU",     "#1f4e79"), // deep blue
    ("US",     "#6a1b9a"), // purple
    ("DE",     "#b8860b"), // dark gold
    ("FR",     "#3b5998"), // mid blue
    ("CH",     "#00838f"), // teal
    ("IN",     "#388e3c"), // forest green
    ("Intl",   "#37474f"), // dark blue-slate
    ("Global", "#6d4c41"), // warm brown
];

/// Six fixed signalisation cues — kept distinct from the jurisdiction palette.
pub const CUE_PRE_EFFECTIVE: &str = "#9aa4ad";
pub const CUE_IN_FORCE: &str = "#666666";
pub const CUE_HARD_DEADLINE: &str = "#111111";
pub const CUE_HOT_SPOT: &str = "#ff4500";
pub const CUE_CONFLICT: &str = "#ff0066";
pub const CUE_MUTUAL_ARC: &str = "#444444";

// ===========================================================================
// Per-language jurisdiction prefixes + abbreviations + sector translations.
// ===========================================================================

/// Per-language jurisdiction label prefix (e.g. EN "Germany BSI VS-NfD",
/// DE "Deutschland BSI VS-NfD"). Used to expand short jurisdiction codes
/// into reader-facing names in Panel C row titles.
///
/// Layout: `JUR_PREFIX[i] = (lang, &[(jur, label)])`.
pub const JUR_PREFIX: &[(&str, &[(&str, &str)])] = &[
    ("en", &[("EU","EU"), ("US","US"), ("DE","Germany"), ("FR","France"), ("CH","Swiss"), ("IN","India"), ("Intl","International"), ("Global","Global")]),
    ("de", &[("EU","EU"), ("US","USA"), ("DE","Deutschland"), ("FR","Frankreich"), ("CH","Schweiz"), ("IN","Indien"), ("Intl","International"), ("Global","Global")]),
    ("fr", &[("EU","UE"), ("US","USA"), ("DE","Allemagne"), ("FR","France"), ("CH","Suisse"), ("IN","Inde"), ("Intl","International"), ("Global","Global")]),
    ("it", &[("EU","UE"), ("US","USA"), ("DE","Germania"), ("FR","Francia"), ("CH","Svizzera"), ("IN","India"), ("Intl","Internazionale"), ("Global","Globale")]),
    ("rm", &[("EU","UE"), ("US","USA"), ("DE","Germania"), ("FR","Frantscha"), ("CH","Svizra"), ("IN","India"), ("Intl","Internaziunal"), ("Global","Global")]),
    ("hi", &[("EU","यूरोपीय"), ("US","अमेरिका"), ("DE","जर्मनी"), ("FR","फ्रांस"), ("CH","स्विट्जरलैंड"), ("IN","भारत"), ("Intl","अंतरराष्ट्रीय"), ("Global","वैश्विक")]),
];

/// Two-character jurisdiction abbreviations for Panel-B annotation
/// (`2018-2023 · 2 jur (CH, EU)`). Latin-script languages share the
/// ISO-3166-1 alpha-2 codes (UE in fr/it/rm for European Union); Hindi
/// uses Devanagari short forms (2-3 Devanagari syllables each).
pub const JUR_ABBREV: &[(&str, &[(&str, &str)])] = &[
    ("en", &[("EU","EU"), ("US","US"), ("DE","DE"), ("FR","FR"), ("CH","CH"), ("IN","IN"), ("Intl","IS"), ("Global","GL")]),
    ("de", &[("EU","EU"), ("US","US"), ("DE","DE"), ("FR","FR"), ("CH","CH"), ("IN","IN"), ("Intl","IS"), ("Global","GL")]),
    ("fr", &[("EU","UE"), ("US","US"), ("DE","DE"), ("FR","FR"), ("CH","CH"), ("IN","IN"), ("Intl","IS"), ("Global","GL")]),
    ("it", &[("EU","UE"), ("US","US"), ("DE","DE"), ("FR","FR"), ("CH","CH"), ("IN","IN"), ("Intl","IS"), ("Global","GL")]),
    ("rm", &[("EU","UE"), ("US","US"), ("DE","DE"), ("FR","FR"), ("CH","CH"), ("IN","IN"), ("Intl","IS"), ("Global","GL")]),
    ("hi", &[("EU","ईयू"), ("US","यूएस"), ("DE","डीई"), ("FR","फ्रा"), ("CH","स्वि"), ("IN","भा"), ("Intl","अं"), ("Global","वै")]),
];

/// Three-letter sector codes stay as-is in Latin-script languages
/// (legend expands them); Hindi switches to Devanagari short forms.
pub const SECTOR_TRANSLATIONS: &[(&str, &[(&str, &str)])] = &[
    ("HOR", &[("en","HOR"), ("de","HOR"), ("fr","HOR"), ("it","HOR"), ("rm","HOR"), ("hi","क्षै")]),
    ("FIN", &[("en","FIN"), ("de","FIN"), ("fr","FIN"), ("it","FIN"), ("rm","FIN"), ("hi","वित्त")]),
    ("IND", &[("en","IND"), ("de","IND"), ("fr","IND"), ("it","IND"), ("rm","IND"), ("hi","उद्य")]),
    ("EMB", &[("en","EMB"), ("de","EMB"), ("fr","EMB"), ("it","EMB"), ("rm","EMB"), ("hi","एम्ब")]),
    ("DEF", &[("en","DEF"), ("de","DEF"), ("fr","DEF"), ("it","DIF"), ("rm","DEF"), ("hi","रक्षा")]),
    ("GOV", &[("en","GOV"), ("de","GOV"), ("fr","GOV"), ("it","GOV"), ("rm","GOV"), ("hi","सर")]),
    ("RET", &[("en","RET"), ("de","RET"), ("fr","RET"), ("it","RET"), ("rm","RET"), ("hi","खुद")]),
    ("RES", &[("en","RES"), ("de","RES"), ("fr","RES"), ("it","RES"), ("rm","RES"), ("hi","अनुस")]),
];

// ===========================================================================
// Font fallbacks per language. The renderer scans `available_fonts` in
// order and picks the first family the host system has. Hindi NEEDS a
// Devanagari-capable family; the renderer must WARN loudly when none
// is found (matplotlib's default falls back to box glyphs).
// ===========================================================================

pub const FONT_FALLBACKS: &[(&str, &[&str])] = &[
    ("en", &["DejaVu Sans"]),
    ("de", &["DejaVu Sans"]),
    ("fr", &["DejaVu Sans"]),
    ("it", &["DejaVu Sans"]),
    ("rm", &["DejaVu Sans"]),
    ("hi", &["Nirmala UI", "Mangal", "Noto Sans Devanagari",
             "Lohit Devanagari", "Sanskrit Text", "DejaVu Sans"]),
];

// ===========================================================================
// Lookup helpers.
// ===========================================================================

/// Look up a per-jurisdiction colour. Falls back to a neutral grey
/// when the jurisdiction is unknown (defensive — should never happen
/// for well-formed `REGS`).
#[must_use]
pub fn colour_for(jur: &str) -> &'static str {
    COLOURS
        .iter()
        .find(|(k, _)| *k == jur)
        .map_or("#888888", |(_, v)| *v)
}

/// Look up the jurisdiction prefix for a (lang, jur) pair. Falls back
/// to English, then to the raw jurisdiction key. Return-type lifetime
/// is tied to the `jur` parameter so the fallback can borrow from it.
#[must_use]
pub fn jur_prefix<'a>(lang: &str, jur: &'a str) -> &'a str {
    let table = JUR_PREFIX
        .iter()
        .find(|(k, _)| *k == lang)
        .map_or(JUR_PREFIX[0].1, |(_, v)| *v);
    table
        .iter()
        .find(|(k, _)| *k == jur)
        .map_or(jur, |(_, v)| *v)
}

/// Look up the jurisdiction abbreviation. Falls back to English, then
/// to the raw jurisdiction key.
#[must_use]
pub fn jur_abbrev<'a>(lang: &str, jur: &'a str) -> &'a str {
    let table = JUR_ABBREV
        .iter()
        .find(|(k, _)| *k == lang)
        .map_or(JUR_ABBREV[0].1, |(_, v)| *v);
    table
        .iter()
        .find(|(k, _)| *k == jur)
        .map_or(jur, |(_, v)| *v)
}

/// Look up the sector-code label for a (lang, sector) pair. Falls back
/// to English, then to the raw sector code.
#[must_use]
pub fn sector_label<'a>(lang: &str, sector: &'a str) -> &'a str {
    let row = SECTOR_TRANSLATIONS
        .iter()
        .find(|(k, _)| *k == sector);
    let Some((_, translations)) = row else {
        return sector;
    };
    translations
        .iter()
        .find(|(k, _)| *k == lang)
        .or_else(|| translations.iter().find(|(k, _)| *k == "en"))
        .map_or(sector, |(_, v)| *v)
}

// ===========================================================================
// Public render API (stub).
// ===========================================================================

/// Render mode for [`render`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// All three panels stacked into one figure.
    Single,
    /// Two figures: `<stem>_AB` (panels A+B) and `<stem>_C` (panel C),
    /// sharing the same time axis.
    Split,
}

/// Top-level entry: render the regulation-timeline figure for `lang`
/// into `out_png` (a sibling `.pdf` is written alongside by the
/// final implementation).
///
/// **Status:** stub. The data layer is complete; the rendering layer
/// (panels A / B / C, mutual-recognition arcs, conflict markers,
/// meta-methodology columns) is shipped in follow-up commits. Calling
/// this today returns `Err("not yet implemented")`.
pub fn render(_out_png: &Path, _lang: &str, _mode: RenderMode) -> Result<()> {
    bail!(
        "regulation_timeline::render is a stub — data port complete, panel \
         renderers (A density / B hot-spots / C Gantt) pending. Use the \
         kit's python renderer (`inbox/regulation_timeline_v3_kit/scripts/\
         _render_regulation_timeline_v3.py`) until this lands."
    )
}

// ===========================================================================
// Tests.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn x_bounds_match_python_reference() {
        assert_eq!(X_LO, 2007);
        assert_eq!(X_HI, 2036);
        assert!(X_LO < X_HI);
    }

    #[test]
    fn goal_keys_count_matches_python_reference() {
        assert_eq!(GOAL_KEYS.len(), 12);
        // Order matters — drives Panel B Y-axis order and Panel C grouping.
        assert_eq!(GOAL_KEYS[0], "data_protection");
        assert_eq!(GOAL_KEYS[11], "methodology");
    }

    #[test]
    fn goal_keys_are_unique() {
        let set: HashSet<&&str> = GOAL_KEYS.iter().collect();
        assert_eq!(set.len(), GOAL_KEYS.len(), "GOAL_KEYS contains a duplicate");
    }

    #[test]
    fn regs_count_matches_python_reference() {
        // Counted from kit v_130732 _render_regulation_timeline_v3.py
        // REGS list (lines 105-199): 31 active regulations + 2 counterparty
        // references (US CLOUD Act, EU Trade Secrets Directive) = 33.
        assert_eq!(REGS.len(), 33, "REGS count drifted from python (was 33)");
    }

    #[test]
    fn regs_labels_are_unique() {
        let set: HashSet<&&str> = REGS.iter().map(|r| &r.label).collect();
        assert_eq!(set.len(), REGS.len(), "REGS contains a duplicate label");
    }

    #[test]
    fn regs_goal_keys_are_in_taxonomy() {
        let goals: HashSet<&&str> = GOAL_KEYS.iter().collect();
        for r in REGS {
            assert!(
                goals.contains(&r.goal_key),
                "regulation '{}' has unknown goal_key '{}' (not in GOAL_KEYS)",
                r.label,
                r.goal_key,
            );
        }
    }

    #[test]
    fn regs_jurisdictions_have_colours() {
        let jurs: HashSet<&&str> = COLOURS.iter().map(|(k, _)| k).collect();
        for r in REGS {
            assert!(
                jurs.contains(&r.jur),
                "regulation '{}' has jurisdiction '{}' with no entry in COLOURS",
                r.label,
                r.jur,
            );
        }
    }

    #[test]
    fn regs_year_ordering_is_consistent() {
        for r in REGS {
            assert!(
                r.pub_year <= r.applies_year,
                "{}: pub_year ({}) must be <= applies_year ({})",
                r.label,
                r.pub_year,
                r.applies_year,
            );
            if let Some(sunset) = r.sunset_year {
                assert!(
                    r.applies_year <= sunset,
                    "{}: applies_year ({}) must be <= sunset_year ({})",
                    r.label,
                    r.applies_year,
                    sunset,
                );
            }
        }
    }

    #[test]
    fn regs_teeth_in_band() {
        for r in REGS {
            assert!(
                (1..=3).contains(&r.teeth),
                "{}: teeth ({}) must be 1..=3",
                r.label,
                r.teeth,
            );
        }
    }

    #[test]
    fn regs_cadence_in_vocabulary() {
        for r in REGS {
            assert!(
                matches!(r.cadence, "stat" | "rev" | "ann"),
                "{}: cadence '{}' must be stat | rev | ann",
                r.label,
                r.cadence,
            );
        }
    }

    #[test]
    fn mutual_count_matches_python_reference() {
        assert_eq!(MUTUAL.len(), 20, "MUTUAL count drifted from python (was 20)");
    }

    #[test]
    fn mutual_endpoints_resolve_to_regs() {
        let labels: HashSet<&&str> = REGS.iter().map(|r| &r.label).collect();
        for m in MUTUAL {
            assert!(labels.contains(&m.src), "MUTUAL src '{}' not in REGS", m.src);
            assert!(labels.contains(&m.tgt), "MUTUAL tgt '{}' not in REGS", m.tgt);
            assert!(
                (1..=3).contains(&m.strength),
                "MUTUAL ({}, {}): strength {} must be 1..=3",
                m.src, m.tgt, m.strength,
            );
        }
    }

    #[test]
    fn conflicts_count_matches_python_reference() {
        // 5 conflict pairs × 2 sides each = 10 markers.
        assert_eq!(CONFLICTS.len(), 10, "CONFLICTS count drifted from python (was 10)");
    }

    #[test]
    fn conflicts_endpoints_resolve_to_regs() {
        let labels: HashSet<&&str> = REGS.iter().map(|r| &r.label).collect();
        for c in CONFLICTS {
            assert!(labels.contains(&c.label), "CONFLICTS label '{}' not in REGS", c.label);
            assert!(
                (X_LO..=X_HI).contains(&c.year),
                "CONFLICTS ({}): year {} outside [{}..={}]",
                c.label, c.year, X_LO, X_HI,
            );
        }
    }

    #[test]
    fn conflicts_are_symmetric() {
        // Every conflict year must appear on EXACTLY TWO regulations
        // (the two sides of the conflict).
        use std::collections::HashMap;
        let mut by_year: HashMap<i32, Vec<&&str>> = HashMap::new();
        for c in CONFLICTS {
            by_year.entry(c.year).or_default().push(&c.label);
        }
        for (year, labels) in by_year {
            assert!(
                labels.len() % 2 == 0,
                "CONFLICTS year {}: odd label count {} (each conflict needs both sides)",
                year, labels.len(),
            );
        }
    }

    #[test]
    fn colours_cover_all_known_jurisdictions() {
        // The 8 jurisdictions that appear in REGS — locked.
        let expected: &[&str] = &["EU", "US", "DE", "FR", "CH", "IN", "Intl", "Global"];
        assert_eq!(COLOURS.len(), expected.len());
        let keys: HashSet<&&str> = COLOURS.iter().map(|(k, _)| k).collect();
        for e in expected {
            assert!(keys.contains(&e), "COLOURS missing jurisdiction '{e}'");
        }
    }

    #[test]
    fn jur_prefix_covers_six_languages() {
        let langs: HashSet<&&str> = JUR_PREFIX.iter().map(|(k, _)| k).collect();
        for expected in ["en", "de", "fr", "it", "rm", "hi"] {
            assert!(langs.contains(&expected), "JUR_PREFIX missing lang '{expected}'");
        }
    }

    #[test]
    fn jur_abbrev_covers_six_languages() {
        let langs: HashSet<&&str> = JUR_ABBREV.iter().map(|(k, _)| k).collect();
        for expected in ["en", "de", "fr", "it", "rm", "hi"] {
            assert!(langs.contains(&expected), "JUR_ABBREV missing lang '{expected}'");
        }
    }

    #[test]
    fn font_fallbacks_cover_six_languages_and_hindi_is_devanagari_capable() {
        let langs: HashSet<&&str> = FONT_FALLBACKS.iter().map(|(k, _)| k).collect();
        for expected in ["en", "de", "fr", "it", "rm", "hi"] {
            assert!(langs.contains(&expected), "FONT_FALLBACKS missing lang '{expected}'");
        }
        // Hindi specifically must list a Devanagari-capable family
        // before the DejaVu fallback (otherwise the renderer silently
        // produces box glyphs).
        let (_, hi_chain) = FONT_FALLBACKS.iter().find(|(k, _)| *k == "hi").unwrap();
        let devanagari = ["Nirmala UI", "Mangal", "Noto Sans Devanagari", "Lohit Devanagari", "Sanskrit Text"];
        assert!(
            hi_chain.iter().any(|f| devanagari.contains(f)),
            "FONT_FALLBACKS['hi'] must include at least one Devanagari-capable family, got {hi_chain:?}"
        );
    }

    #[test]
    fn colour_for_returns_known_jurisdictions() {
        assert_eq!(colour_for("EU"), "#1f4e79");
        assert_eq!(colour_for("CH"), "#00838f");
        assert_eq!(colour_for("Global"), "#6d4c41");
    }

    #[test]
    fn colour_for_falls_back_for_unknown() {
        assert_eq!(colour_for("ZZ"), "#888888");
    }

    #[test]
    fn jur_prefix_lookup_handles_known_and_fallback() {
        assert_eq!(jur_prefix("en", "DE"), "Germany");
        assert_eq!(jur_prefix("de", "DE"), "Deutschland");
        assert_eq!(jur_prefix("hi", "DE"), "जर्मनी");
        // Unknown lang → falls back to English row.
        assert_eq!(jur_prefix("ja", "DE"), "Germany");
        // Unknown jur → returns the raw key.
        assert_eq!(jur_prefix("en", "ZZ"), "ZZ");
    }

    #[test]
    fn sector_label_lookup_handles_lang_specialisation() {
        assert_eq!(sector_label("en", "DEF"), "DEF");
        assert_eq!(sector_label("it", "DEF"), "DIF"); // Italian: Difesa
        assert_eq!(sector_label("hi", "DEF"), "रक्षा"); // Hindi: rakṣā
    }

    #[test]
    fn render_stub_returns_explanatory_error() {
        let r = render(Path::new("/tmp/x.png"), "en", RenderMode::Single);
        assert!(r.is_err());
        let e = format!("{}", r.unwrap_err());
        assert!(
            e.contains("stub") || e.contains("python"),
            "stub error message should explain status, got: {e}"
        );
    }
}
