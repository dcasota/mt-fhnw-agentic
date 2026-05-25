//! AI-vs-human investment / replacement estimator.
//!
//! Replaces fragile point-estimates of the "what fraction of an engineering
//! challenge can AI agents absorb, and at what cost?" question with an
//! auditable, parameterised model plus a sensitivity band.
//!
//! The legacy model (thesis doc) estimated each challenge in human-hours,
//! classified it by risk (Low/Med/High/Crit), and derived an *irreducible*
//! human-percentage from the operating-mode mix (Normal 90 % AI / 10 % human,
//! Escalation 40/60, Disaster 0/100). AI-replaceable hours ÷ 160 = AI
//! agent-months; × \$1 000 / agent-month (≈ 600 M tokens at ≈ \$1.67 / M tok).
//!
//! Those point estimates are fragile because they assume (a) 1:1 AI:human
//! parity, (b) a fixed token price, (c) a region-fixed human cost, and
//! (d) mode-uniform rates. This module parameterises all four and reports the
//! result as a low/base/high band, so the headline finding — that the AI cost
//! *share* is robust in direction (small) but varies materially in level — is
//! auditable rather than asserted.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Hours of AI work that one "agent-month" of compute is taken to displace
/// (≈ one human FTE-month at the legacy 160 h baseline).
const HOURS_PER_AGENT_MONTH: f64 = 160.0;

/// Risk class of an engineering challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskClass {
    Low,
    Med,
    High,
    Crit,
}

impl RiskClass {
    /// Operating-mode mix `(normal, escalation, disaster)` shares for this risk
    /// class. Higher risk shifts weight toward escalation/disaster, where the
    /// irreducible human share is larger.
    #[must_use]
    pub const fn mode_mix(self) -> (f64, f64, f64) {
        match self {
            Self::Low => (0.80, 0.15, 0.05),
            Self::Med => (0.60, 0.30, 0.10),
            Self::High => (0.40, 0.45, 0.15),
            Self::Crit => (0.30, 0.50, 0.20),
        }
    }

    /// Irreducible human percentage, recomputed from the mode mix so it is
    /// auditable: `normal·0.10 + escalation·0.60 + disaster·1.00`.
    ///
    /// This reproduces the legacy per-risk table (Low ≈ 0.22, Med ≈ 0.34,
    /// High ≈ 0.46, Crit ≈ 0.53).
    #[must_use]
    pub fn human_pct(self) -> f64 {
        let (normal, esc, dis) = self.mode_mix();
        dis.mul_add(1.00, esc.mul_add(0.60, normal * 0.10))
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Med => "Med",
            Self::High => "High",
            Self::Crit => "Crit",
        }
    }
}

/// One engineering challenge to estimate.
#[derive(Debug, Clone, Deserialize)]
pub struct Challenge {
    pub name: String,
    pub human_hours: f64,
    pub risk: RiskClass,
}

/// A loaded challenge file: `{"challenges": [...]}` or a bare array.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ChallengeFile {
    Wrapped { challenges: Vec<Challenge> },
    Bare(Vec<Challenge>),
}

/// Tunable parameters that drive the sensitivity band.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Fully-loaded human cost, USD per hour.
    pub human_rate: f64,
    /// Token price, USD per million tokens.
    pub token_price_per_mtok: f64,
    /// Compute per agent-month, in millions of tokens.
    pub tokens_per_month: f64,
    /// AI:human productivity factor, `0 < parity <= 1`. Below 1, the AI needs
    /// proportionally more agent-months to absorb the same work.
    pub parity: f64,
    /// Premium on the human rate for the escalation-mode share of the work.
    pub escalation_premium: f64,
    /// Premium on the human rate for the disaster-mode share of the work.
    pub disaster_premium: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            human_rate: region_rate_known("CH"),
            token_price_per_mtok: 1.67,
            tokens_per_month: 600.0,
            parity: 1.0,
            escalation_premium: 1.5,
            disaster_premium: 2.0,
        }
    }
}

/// Per-challenge result.
#[derive(Debug, Clone, Serialize)]
pub struct ChallengeResult {
    pub name: String,
    pub risk: RiskClass,
    pub human_hours: f64,
    pub human_pct: f64,
    pub irreducible_human_hours: f64,
    pub ai_replaceable_hours: f64,
    pub ai_agent_months: f64,
    pub ai_cost: f64,
    pub human_cost: f64,
    pub ai_share: f64,
}

/// Totals across all challenges.
#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    pub human_hours: f64,
    pub irreducible_human_hours: f64,
    pub ai_agent_months: f64,
    pub ai_cost: f64,
    pub human_cost: f64,
    pub ai_share: f64,
}

/// Full estimate: per-challenge rows plus totals.
#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    pub challenges: Vec<ChallengeResult>,
    pub totals: Totals,
}

/// Region keys with a defined fully-loaded human rate, ordered as in the table.
/// CH is the default region.
pub const REGION_KEYS: &[&str] = &[
    "CH", "US", "IL", "UK", "AU", "EU", "CA", "SG", "JP", "AE", "KR", "SA", "CN", "BR", "IN",
];

/// Fully-loaded senior software-engineer cost (USD/hour) by region.
///
/// These are *indicative* fully-loaded figures aligned to the AI-Norms-book
/// jurisdiction set, NOT audited market data. Region key is matched
/// case-insensitively; an unknown key is an error listing the valid keys.
pub fn region_rate(region: &str) -> Result<f64> {
    let rate = match region.to_ascii_uppercase().as_str() {
        "CH" => 180.0, // Switzerland (default)
        "US" => 130.0, // United States
        "IL" => 95.0,  // Israel
        "UK" => 95.0,  // United Kingdom
        "AU" => 90.0,  // Australia
        "EU" => 85.0,  // European Union (avg)
        "CA" => 85.0,  // Canada
        "SG" => 80.0,  // Singapore
        "JP" => 70.0,  // Japan
        "AE" => 70.0,  // United Arab Emirates
        "KR" => 65.0,  // South Korea
        "SA" => 60.0,  // Saudi Arabia
        "CN" => 45.0,  // China
        "BR" => 35.0,  // Brazil
        "IN" => 30.0,  // India
        other => {
            anyhow::bail!(
                "unknown --region '{other}'; valid keys: {}",
                REGION_KEYS.join(", ")
            );
        }
    };
    Ok(rate)
}

/// Statically-valid region lookup for internal presets (known keys only).
#[must_use]
fn region_rate_known(region: &str) -> f64 {
    region_rate(region).expect("internal region key must be valid")
}

/// Effective human rate for a risk class: the base rate weighted by the mode
/// mix, with escalation/disaster shares carrying their premium.
#[must_use]
pub fn effective_human_rate(base_rate: f64, risk: RiskClass, p: &Params) -> f64 {
    let (normal, esc, dis) = risk.mode_mix();
    base_rate
        * dis.mul_add(
            p.disaster_premium,
            esc.mul_add(p.escalation_premium, normal),
        )
}

/// Compute one challenge under the given parameters.
#[must_use]
pub fn compute_challenge(c: &Challenge, p: &Params) -> ChallengeResult {
    let human_pct = c.risk.human_pct();
    let irreducible_human_hours = c.human_hours * human_pct;
    let ai_replaceable_hours = c.human_hours * (1.0 - human_pct);
    let ai_agent_months = ai_replaceable_hours / HOURS_PER_AGENT_MONTH / p.parity;
    let ai_cost = ai_agent_months * p.tokens_per_month * p.token_price_per_mtok;
    let human_cost = irreducible_human_hours * effective_human_rate(p.human_rate, c.risk, p);
    let total = ai_cost + human_cost;
    let ai_share = if total > 0.0 { ai_cost / total } else { 0.0 };
    ChallengeResult {
        name: c.name.clone(),
        risk: c.risk,
        human_hours: c.human_hours,
        human_pct,
        irreducible_human_hours,
        ai_replaceable_hours,
        ai_agent_months,
        ai_cost,
        human_cost,
        ai_share,
    }
}

/// Compute every challenge and the totals.
#[must_use]
pub fn estimate(challenges: &[Challenge], p: &Params) -> Estimate {
    let rows: Vec<ChallengeResult> = challenges.iter().map(|c| compute_challenge(c, p)).collect();
    let mut t = Totals {
        human_hours: 0.0,
        irreducible_human_hours: 0.0,
        ai_agent_months: 0.0,
        ai_cost: 0.0,
        human_cost: 0.0,
        ai_share: 0.0,
    };
    for r in &rows {
        t.human_hours += r.human_hours;
        t.irreducible_human_hours += r.irreducible_human_hours;
        t.ai_agent_months += r.ai_agent_months;
        t.ai_cost += r.ai_cost;
        t.human_cost += r.human_cost;
    }
    let total = t.ai_cost + t.human_cost;
    t.ai_share = if total > 0.0 { t.ai_cost / total } else { 0.0 };
    Estimate {
        challenges: rows,
        totals: t,
    }
}

/// One row of the sensitivity sweep.
#[derive(Debug, Clone, Serialize)]
pub struct SensitivityRow {
    pub label: String,
    pub parity: f64,
    pub scenario: String,
    pub token_price_per_mtok: f64,
    pub region: String,
    pub human_rate: f64,
    pub total_cost: f64,
    pub ai_share: f64,
}

/// The sensitivity band: a low / base / high sweep plus the derived range.
#[derive(Debug, Clone, Serialize)]
pub struct Sensitivity {
    pub rows: Vec<SensitivityRow>,
    pub ai_share_min: f64,
    pub ai_share_max: f64,
}

/// Token price (USD / M tokens) for a named scenario preset.
#[must_use]
pub fn scenario_token_price(scenario: &str) -> Option<f64> {
    match scenario {
        "baseline-2025" => Some(1.67),
        "asic-2027" => Some(0.20),
        _ => None,
    }
}

/// Build the low/base/high sensitivity band.
///
/// Jointly varies parity, the token scenario, and the region. The corner cases
/// bracket the headline `ai_share`: cheap-AI / cheap-labour (IN, asic-2027,
/// parity 0.5) versus expensive-AI / expensive-labour (US, baseline-2025,
/// parity 1.0).
#[must_use]
pub fn sensitivity(challenges: &[Challenge], base: &Params) -> Sensitivity {
    // (label, parity, scenario, region)
    let corners = [
        ("low", 0.5, "asic-2027", "IN"),
        ("base", 0.75, "baseline-2025", "CH"),
        ("high", 1.0, "baseline-2025", "US"),
    ];
    let mut rows = Vec::with_capacity(corners.len());
    for (label, parity, scenario, region) in corners {
        let token_price = scenario_token_price(scenario).unwrap_or(base.token_price_per_mtok);
        let p = Params {
            human_rate: region_rate_known(region),
            token_price_per_mtok: token_price,
            parity,
            ..*base
        };
        let est = estimate(challenges, &p);
        rows.push(SensitivityRow {
            label: label.to_string(),
            parity,
            scenario: scenario.to_string(),
            token_price_per_mtok: token_price,
            region: region.to_string(),
            human_rate: p.human_rate,
            total_cost: est.totals.ai_cost + est.totals.human_cost,
            ai_share: est.totals.ai_share,
        });
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for r in &rows {
        min = min.min(r.ai_share);
        max = max.max(r.ai_share);
    }
    Sensitivity {
        rows,
        ai_share_min: min,
        ai_share_max: max,
    }
}

/// A small built-in demo corpus so the CLI runs with no arguments.
#[must_use]
pub fn demo_challenges() -> Vec<Challenge> {
    vec![
        Challenge {
            name: "Routine feature delivery".to_string(),
            human_hours: 50_000.0,
            risk: RiskClass::Low,
        },
        Challenge {
            name: "Platform hardening".to_string(),
            human_hours: 120_000.0,
            risk: RiskClass::High,
        },
        Challenge {
            name: "Cryptographic core / safety-critical".to_string(),
            human_hours: 300_000.0,
            risk: RiskClass::Crit,
        },
    ]
}

/// Parse a challenge file (`{"challenges":[...]}` or a bare JSON array).
pub fn parse_challenges(json: &str) -> Result<Vec<Challenge>> {
    let file: ChallengeFile = serde_json::from_str(json).context("parse challenges JSON")?;
    Ok(match file {
        ChallengeFile::Wrapped { challenges } | ChallengeFile::Bare(challenges) => challenges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-6;

    fn crit_300k() -> Challenge {
        Challenge {
            name: "crit".to_string(),
            human_hours: 300_000.0,
            risk: RiskClass::Crit,
        }
    }

    #[test]
    fn human_pct_reproduces_legacy_table() {
        assert!((RiskClass::Low.human_pct() - 0.22).abs() < EPS);
        assert!((RiskClass::Med.human_pct() - 0.34).abs() < EPS);
        assert!((RiskClass::High.human_pct() - 0.46).abs() < EPS);
        assert!((RiskClass::Crit.human_pct() - 0.53).abs() < EPS);
    }

    #[test]
    fn reproduces_legacy_point_estimate_at_baseline() {
        // parity=1, token_price=1.67, tokens=600, region=CH baseline.
        let p = Params::default();
        let r = compute_challenge(&crit_300k(), &p);
        // human_pct ~= 0.53
        assert!((r.human_pct - 0.53).abs() < EPS);
        // ai_replaceable = 300000 * 0.47 = 141000; /160 = 881.25 agent-months.
        assert!((r.ai_agent_months - 881.25).abs() < 1e-3);
        // ai_cost = 881.25 * 600 * 1.67
        assert!((r.ai_cost - 881.25 * 600.0 * 1.67).abs() < 1e-3);
    }

    #[test]
    fn parity_half_doubles_agent_months() {
        let base = Params::default();
        let half = Params {
            parity: 0.5,
            ..base
        };
        let r1 = compute_challenge(&crit_300k(), &base);
        let r2 = compute_challenge(&crit_300k(), &half);
        assert!((r2.ai_agent_months - 2.0 * r1.ai_agent_months).abs() < 1e-6);
        assert!((r2.ai_cost - 2.0 * r1.ai_cost).abs() < 1e-3);
    }

    #[test]
    fn ai_share_rises_when_region_cost_drops() {
        // Same AI cost, cheaper humans => AI is a larger share of total spend.
        let us = Params {
            human_rate: region_rate("US").unwrap(),
            ..Params::default()
        };
        let inr = Params {
            human_rate: region_rate("IN").unwrap(),
            ..Params::default()
        };
        let r_us = compute_challenge(&crit_300k(), &us);
        let r_in = compute_challenge(&crit_300k(), &inr);
        assert!(r_in.ai_share > r_us.ai_share);
    }

    #[test]
    fn sensitivity_band_is_ordered_and_small() {
        let chs = demo_challenges();
        let s = sensitivity(&chs, &Params::default());
        assert_eq!(s.rows.len(), 3);
        assert!(s.ai_share_min <= s.ai_share_max);
        // Direction is robust: AI share stays a minority share across the band.
        assert!(s.ai_share_max < 0.5);
        assert!(s.ai_share_min > 0.0);
    }

    #[test]
    fn region_rate_table_and_case_insensitivity() {
        // A sample of the expanded AI-Norms-book jurisdiction set.
        assert!((region_rate("CH").unwrap() - 180.0).abs() < EPS);
        assert!((region_rate("IN").unwrap() - 30.0).abs() < EPS);
        assert!((region_rate("CN").unwrap() - 45.0).abs() < EPS);
        // Case-insensitive matching.
        assert!((region_rate("ch").unwrap() - 180.0).abs() < EPS);
        assert!((region_rate("Us").unwrap() - 130.0).abs() < EPS);
        // Back-compat: the original four still resolve.
        assert!(region_rate("EU").is_ok());
        // Unknown key errors and the message lists the valid keys.
        let err = region_rate("XX").unwrap_err().to_string();
        assert!(err.contains("unknown --region"));
        assert!(err.contains("CH"));
        assert!(err.contains("IN"));
    }

    #[test]
    fn parse_accepts_wrapped_and_bare() {
        let wrapped = r#"{"challenges":[{"name":"a","human_hours":100,"risk":"crit"}]}"#;
        let bare = r#"[{"name":"a","human_hours":100,"risk":"low"}]"#;
        assert_eq!(parse_challenges(wrapped).unwrap().len(), 1);
        assert_eq!(parse_challenges(bare).unwrap().len(), 1);
        assert_eq!(parse_challenges(bare).unwrap()[0].risk, RiskClass::Low);
    }
}
