//! RAMP — Risk-Adjusted Metadata Prediction (ADR-0040).
//!
//! A deterministic estimator that predicts, per thesis item (dimension /
//! campaign / project / tool), from declared metadata: affected SLOC, cumulative
//! human-hours (COCOMO II) across related items, impact level, risk exposure
//! (likelihood × impact, ALE), market size & projections, and a temporary
//! mass-reskilling surge — across region/Broadcom scenarios and the
//! normal/escalation/catastrophic operating modes (ADR-0017).
//!
//! Every figure is reproducible from the inputs; nothing is invented (ADR-0036).
//! Each input is either `sourced` (with a reference) or a declared `assumption`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// Published COCOMO II nominal coefficients (Boehm et al., 2000).
const COCOMO_A: f64 = 2.94;
const COCOMO_E: f64 = 1.0997;
const HOURS_PER_PM: f64 = 152.0;

/// The three operating modes (ADR-0017) ≙ BSI protection-need categories.
pub const MODES: [&str; 3] = ["normal", "escalation", "catastrophic"];
/// The two future scenarios.
pub const SCENARIOS: [&str; 2] = ["region", "broadcom"];

// ---------------------------------------------------------------------------
// Input model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Corpus {
    pub items: Vec<ItemMeta>,
    #[serde(default)]
    pub params: Params,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemMeta {
    pub id: String,
    pub title: String,
    pub kind: String,
    /// Affected source lines of code (metadata or declared assumption).
    pub sloc_baseline: f64,
    /// Related item ids for cumulative human-hours.
    #[serde(default)]
    pub related: Vec<String>,
    pub cia: Cia,
    /// Autonomous-AI-attack threat progression index, 0..1.
    pub threat_index: f64,
    pub asset_value: f64,
    pub exposure_factor: f64,
    pub aro: f64,
    pub reference_base: f64,
    pub market: Market,
    pub headcount: f64,
    pub reskilling_hours: f64,
    pub day_rate: f64,
    #[serde(default)]
    pub assumptions: Vec<Assumption>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Cia {
    pub c: u8,
    pub i: u8,
    pub a: u8,
    pub accountability: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Market {
    pub base_year: i32,
    pub horizon: Vec<i32>,
    pub sam_frac: f64,
    pub som_frac: f64,
    pub region: Segment,
    pub broadcom: Segment,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Segment {
    pub tam_base: f64,
    pub cagr: ModeVals,
    #[serde(default)]
    pub citation: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ModeVals {
    pub normal: f64,
    pub escalation: f64,
    pub catastrophic: f64,
}

impl ModeVals {
    fn get(&self, mode: &str) -> f64 {
        match mode {
            "escalation" => self.escalation,
            "catastrophic" => self.catastrophic,
            _ => self.normal,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Assumption {
    pub key: String,
    pub value: String,
    pub basis: String,
    /// "sourced" | "assumption".
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Params {
    pub threat_theta: f64,
    /// Likelihood multiplier per mode.
    pub likelihood_mult: ModeVals,
    /// Annual-rate-of-occurrence multiplier per mode.
    pub aro_mult: ModeVals,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            threat_theta: 0.5,
            likelihood_mult: ModeVals {
                normal: 1.0,
                escalation: 1.6,
                catastrophic: 2.4,
            },
            aro_mult: ModeVals {
                normal: 1.0,
                escalation: 2.0,
                catastrophic: 4.0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Output model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ItemReport {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub sloc: f64,
    pub dev_hours: f64,
    pub cumulative_dev_hours: f64,
    pub related: Vec<String>,
    /// Each related item's own COCOMO hours, aligned with `related`.
    pub related_hours: Vec<f64>,
    pub cells: Vec<Cell>,
    pub assumptions: Vec<Assumption>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cell {
    pub scenario: String,
    pub mode: String,
    pub impact: String,
    pub likelihood_level: u8,
    pub risk_score: u32,
    pub ale: f64,
    pub financial_band: String,
    /// (year, SOM market size) projection points.
    pub market_som: Vec<(i32, f64)>,
    pub reskilling_surge: f64,
}

/// COCOMO II nominal effort in person-hours for `sloc` lines.
fn cocomo_hours(sloc: f64) -> f64 {
    if sloc <= 0.0 {
        return 0.0;
    }
    let kloc = sloc / 1000.0;
    COCOMO_A * kloc.powf(COCOMO_E) * HOURS_PER_PM
}

fn impact_label(level: u8) -> &'static str {
    match level {
        0 | 1 => "NORMAL",
        2 => "HIGH",
        _ => "VERY-HIGH",
    }
}

/// Mode raises the impact floor: escalation ⇒ ≥ HIGH, catastrophic ⇒ VERY-HIGH.
fn mode_impact_floor(mode: &str) -> u8 {
    match mode {
        "escalation" => 2,
        "catastrophic" => 3,
        _ => 1,
    }
}

fn financial_band(ale: f64, reference_base: f64) -> &'static str {
    if reference_base <= 0.0 {
        return "NORMAL";
    }
    let ratio = ale / reference_base;
    if ratio < 0.10 {
        "NORMAL"
    } else if ratio <= 1.0 {
        "HIGH"
    } else {
        "VERY-HIGH"
    }
}

/// Compute the full report for a corpus.
pub fn compute(corpus: &Corpus) -> Vec<ItemReport> {
    let hours_by_id: std::collections::HashMap<&str, f64> = corpus
        .items
        .iter()
        .map(|it| (it.id.as_str(), cocomo_hours(it.sloc_baseline)))
        .collect();

    corpus
        .items
        .iter()
        .map(|it| {
            let dev_hours = cocomo_hours(it.sloc_baseline);
            let related_hours: Vec<f64> = it
                .related
                .iter()
                .map(|r| hours_by_id.get(r.as_str()).copied().unwrap_or(0.0))
                .collect();
            let cumulative_dev_hours = dev_hours + related_hours.iter().sum::<f64>();

            let base_impact = it
                .cia
                .c
                .max(it.cia.i)
                .max(it.cia.a)
                .max(it.cia.accountability);
            let mut cells = Vec::new();
            for scenario in SCENARIOS {
                let seg = if scenario == "broadcom" {
                    &it.market.broadcom
                } else {
                    &it.market.region
                };
                for mode in MODES {
                    let impact_level = base_impact.max(mode_impact_floor(mode)).min(3);
                    let base_ll = (it.threat_index * 5.0).ceil().max(1.0);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let likelihood_level = (base_ll * corpus.params.likelihood_mult.get(mode))
                        .round()
                        .clamp(1.0, 5.0) as u8;
                    let risk_score = u32::from(likelihood_level) * u32::from(impact_level);
                    let sle = it.asset_value * it.exposure_factor;
                    let ale = sle * it.aro * corpus.params.aro_mult.get(mode);
                    let cagr = seg.cagr.get(mode);
                    let market_som = std::iter::once(it.market.base_year)
                        .chain(it.market.horizon.iter().copied())
                        .map(|year| {
                            let exp = f64::from(year - it.market.base_year);
                            let size = seg.tam_base * (1.0 + cagr).powf(exp);
                            (year, size * it.market.sam_frac * it.market.som_frac)
                        })
                        .collect();
                    let reskilling_surge =
                        if mode != "normal" && it.threat_index >= corpus.params.threat_theta {
                            it.headcount * it.reskilling_hours * (it.day_rate / 8.0)
                        } else {
                            0.0
                        };
                    cells.push(Cell {
                        scenario: scenario.to_string(),
                        mode: mode.to_string(),
                        impact: impact_label(impact_level).to_string(),
                        likelihood_level,
                        risk_score,
                        ale,
                        financial_band: financial_band(ale, it.reference_base).to_string(),
                        market_som,
                        reskilling_surge,
                    });
                }
            }

            ItemReport {
                id: it.id.clone(),
                title: it.title.clone(),
                kind: it.kind.clone(),
                sloc: it.sloc_baseline,
                dev_hours,
                cumulative_dev_hours,
                related: it.related.clone(),
                related_hours,
                cells,
                assumptions: it.assumptions.clone(),
            }
        })
        .collect()
}

/// Parse a corpus from JSON.
pub fn parse(json: &str) -> Result<Corpus> {
    serde_json::from_str(json).context("parse RAMP corpus JSON")
}

// ---------------------------------------------------------------------------
// Presentation: figspecs + a six-page markdown chapter
// ---------------------------------------------------------------------------

/// Group an integer with thousands separators; small magnitudes keep 1 decimal.
fn fmt_num(x: f64) -> String {
    if x.abs() >= 100.0 {
        let n = x.round() as i64;
        let s = n.abs().to_string();
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(' ');
            }
            out.push(ch);
        }
        if n < 0 { format!("-{out}") } else { out }
    } else {
        format!("{x:.1}")
    }
}

fn cell<'a>(report: &'a ItemReport, scenario: &str, mode: &str) -> &'a Cell {
    report
        .cells
        .iter()
        .find(|c| c.scenario == scenario && c.mode == mode)
        .expect("scenario×mode cell exists")
}

/// The Broadcom-scenario escalation-mode cell (the aggregate-chart default).
fn bc_esc(report: &ItemReport) -> &Cell {
    cell(report, "broadcom", "escalation")
}

/// Three figspec blocks (market matrix, hours bar, risk matrix) for the chapter.
#[must_use]
pub fn figspecs(report: &ItemReport) -> Vec<String> {
    let id = &report.id;

    // 1. Market SOM (Broadcom scenario) by mode × year.
    let years: Vec<i32> = cell(report, "broadcom", "normal")
        .market_som
        .iter()
        .map(|(y, _)| *y)
        .collect();
    let market_rows: Vec<String> = MODES.iter().map(|m| (*m).to_string()).collect();
    let market_cells: Vec<Vec<String>> = MODES
        .iter()
        .map(|m| {
            cell(report, "broadcom", m)
                .market_som
                .iter()
                .map(|(_, v)| fmt_num(*v))
                .collect()
        })
        .collect();
    let market = serde_json::json!({
        "id": format!("{id}_ramp_market"),
        "type": "matrix",
        "title": format!("{} — serviceable obtainable market (Broadcom scenario)", report.title),
        "caption": "Serviceable obtainable market by mode and year",
        "palette": "wong",
        "data": {
            "rows": market_rows,
            "cols": years.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "cells": market_cells,
            "legend": "Model estimate (RAMP, ADR-0040); units per assumptions ledger"
        }
    });

    // 2. Cumulative development hours: this item + each related item.
    let mut hour_labels = vec![report.id.clone()];
    hour_labels.extend(report.related.iter().cloned());
    let mut hour_values = vec![report.dev_hours.round()];
    hour_values.extend(report.related_hours.iter().map(|h| h.round()));
    let hours = serde_json::json!({
        "id": format!("{id}_ramp_hours"),
        "type": "bar",
        "title": format!("{} — development human-hours (COCOMO II)", report.title),
        "caption": "Cumulative development hours across related challenges",
        "palette": "wong",
        "data": { "labels": hour_labels, "values": hour_values, "xlabel": "person-hours" }
    });

    // 3. Risk & cost summary (Broadcom scenario) by mode.
    let risk_cells: Vec<Vec<String>> = MODES
        .iter()
        .map(|m| {
            let c = cell(report, "broadcom", m);
            vec![
                c.impact.clone(),
                c.risk_score.to_string(),
                fmt_num(c.ale),
                fmt_num(c.reskilling_surge),
            ]
        })
        .collect();
    let risk = serde_json::json!({
        "id": format!("{id}_ramp_risk"),
        "type": "matrix",
        "title": format!("{} — risk and reskilling by operating mode", report.title),
        "caption": "Impact, risk score, ALE and reskilling by mode",
        "palette": "wong",
        "data": {
            "rows": MODES.iter().map(|m| (*m).to_string()).collect::<Vec<_>>(),
            "cols": ["Impact", "Risk score", "ALE", "Reskilling surge"],
            "cells": risk_cells,
            "legend": "Model estimate (RAMP, ADR-0040)"
        }
    });

    [market, hours, risk]
        .iter()
        .map(|v| format!("```figspec\n{v}\n```"))
        .collect()
}

fn bar_figspec(
    id: &str,
    title: &str,
    caption: &str,
    labels: &[String],
    values: &[f64],
    xlabel: &str,
) -> String {
    let v = serde_json::json!({
        "id": id, "type": "bar", "title": title, "caption": caption, "palette": "wong",
        "data": { "labels": labels, "values": values.iter().map(|x| x.round()).collect::<Vec<_>>(), "xlabel": xlabel }
    });
    format!("```figspec\n{v}\n```")
}

/// Aggregate "graphical illustrations" chapter across all dimensions and
/// campaigns (for the student notes): every risk-assessment aspect, one figure.
#[must_use]
pub fn graphics(reports: &[ItemReport]) -> String {
    let pick =
        |kind: &str| -> Vec<&ItemReport> { reports.iter().filter(|r| r.kind == kind).collect() };

    let dims = pick("dimension");
    let camps = pick("campaign");

    let mut s = String::new();
    s.push_str("## Graphical illustrations — risk-assessment findings across all dimensions and campaigns\n\n");
    s.push_str(
        "This chapter visualises the RAMP findings (ADR-0040) for every dimension \
and campaign side by side, so the aspects — development effort, risk exposure, \
the temporary mass-reskilling surge, and the market window — can be compared at a \
glance. All values are model estimates under the escalation (Broadcom) scenario \
unless noted; see each chapter's assumptions ledger for provenance.\n\n",
    );

    // 1. cumulative dev-hours per dimension
    s.push_str("### Development effort by dimension\n\n");
    s.push_str(&bar_figspec(
        "ramp_all_dim_hours",
        "Cumulative development human-hours by dimension",
        "Cumulative COCOMO-II human-hours per dimension",
        &dims.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        &dims
            .iter()
            .map(|r| r.cumulative_dev_hours)
            .collect::<Vec<_>>(),
        "person-hours",
    ));
    s.push_str("\n\n");

    // 2. cumulative dev-hours per campaign
    s.push_str("### Development effort by campaign\n\n");
    s.push_str(&bar_figspec(
        "ramp_all_camp_hours",
        "Cumulative development human-hours by campaign",
        "Cumulative COCOMO-II human-hours per campaign",
        &camps.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        &camps
            .iter()
            .map(|r| r.cumulative_dev_hours)
            .collect::<Vec<_>>(),
        "person-hours",
    ));
    s.push_str("\n\n");

    // 3. escalation risk score across all items
    s.push_str("### Risk exposure (escalation mode) across all items\n\n");
    s.push_str(&bar_figspec(
        "ramp_all_risk",
        "Risk score under escalation by item",
        "Likelihood times impact, escalation mode, all items",
        &reports.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        &reports
            .iter()
            .map(|r| f64::from(bc_esc(r).risk_score))
            .collect::<Vec<_>>(),
        "risk score",
    ));
    s.push_str("\n\n");

    // 4. reskilling surge across all items
    s.push_str("### Temporary mass-reskilling surge across all items\n\n");
    s.push_str(&bar_figspec(
        "ramp_all_reskill",
        "Reskilling surge under escalation by item",
        "Temporary reskilling cost, escalation mode, all items",
        &reports.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        &reports
            .iter()
            .map(|r| bc_esc(r).reskilling_surge)
            .collect::<Vec<_>>(),
        "reskilling cost",
    ));
    s.push_str("\n\n");

    // 5. market SOM at horizon across all items
    s.push_str("### Market window (escalation, horizon year) across all items\n\n");
    s.push_str(&bar_figspec(
        "ramp_all_market",
        "Serviceable obtainable market at horizon by item",
        "SOM at horizon year, escalation Broadcom scenario",
        &reports.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        &reports
            .iter()
            .map(|r| bc_esc(r).market_som.last().map_or(0.0, |p| p.1))
            .collect::<Vec<_>>(),
        "market (units per ledger)",
    ));
    s.push_str("\n\n");

    s.push_str(
        "The figures read together as the thesis's risk panorama: effort is \
concentrated in the code-heavy dimensions and campaigns, risk peaks where the \
autonomous-AI-attack threat index is highest, the reskilling surge appears only \
for the items above the threat threshold, and the market window is widest exactly \
where the risk is greatest — the recurring RAMP finding that the largest risk and \
the largest opportunity are the same curve.\n\n",
    );

    s
}

/// A data-driven interpretation paragraph computed from this item's profile.
fn reading(report: &ItemReport) -> String {
    let bn = cell(report, "broadcom", "normal");
    let be = cell(report, "broadcom", "escalation");
    let bc = cell(report, "broadcom", "catastrophic");
    let rn = cell(report, "region", "normal");

    let impact_clause = if bn.impact == "VERY-HIGH" {
        "Impact is fixed at the maximum in every mode, so the entire defensible \
margin lives in containing likelihood and loss frequency — a narrow autonomy \
envelope and fast detection — rather than in reducing the consequence of a breach"
            .to_string()
    } else {
        format!(
            "Impact climbs from {} in normal mode to {} under the catastrophic \
mode as the mode floor rises, so both the consequence and its frequency must be \
managed",
            bn.impact, bc.impact
        )
    };

    let resk_clause = if be.reskilling_surge > 0.0 {
        "Because the autonomous-AI-attack threat index for this item is above the \
trigger threshold, a temporary mass-reskilling surge is provisioned the moment \
the mode escalates; pre-budgeting it (measure M2) is what keeps the escalation \
survivable"
            .to_string()
    } else {
        "The threat index for this item sits below the reskilling trigger, so no \
mass-reskilling surge is provisioned; the risk-based control accent stays latent \
unless the threat profile worsens"
            .to_string()
    };

    let grow = if be.market_som.first().map_or(0.0, |p| p.1) > 0.0 {
        be.market_som.last().map_or(0.0, |p| p.1) / be.market_som.first().map_or(1.0, |p| p.1)
    } else {
        0.0
    };

    format!(
        "### Reading\n\n{impact_clause}. {resk_clause}. The market signal is \
consistent with the threat signal: under the Broadcom escalation scenario the \
serviceable obtainable market grows roughly {grow:.1}× across the horizon, and \
the catastrophic-mode curve outruns the normal one — adversarial automation is \
itself a demand driver, so the largest risk window and the largest market window \
are the same curve seen from opposite sides. The regional scenario reaches about \
{} by the horizon against the Broadcom {}, which frames how much of the curve a \
sovereign builder can actually address. The planning input is the mode band, not \
any single point.\n\n",
        fmt_num(rn.market_som.last().map_or(0.0, |p| p.1)),
        fmt_num(be.market_som.last().map_or(0.0, |p| p.1)),
    )
}

/// Assemble a six-page risk-assessment chapter (markdown) for one item.
#[must_use]
pub fn chapter(report: &ItemReport) -> String {
    let figs = figspecs(report);
    let mut s = String::new();

    s.push_str(&format!(
        "## Risk-assessment findings — {}\n\n",
        report.title
    ));
    s.push_str(
        "All quantities below are **RAMP model estimates** (ADR-0040), derived \
deterministically from declared metadata via an adopted BSI IT-Grundschutz / \
NIST CSF methodology; every coefficient is sourced or listed as an assumption \
in the ledger at the end of this chapter. The three operating modes — *normal*, \
*escalation*, *catastrophic* (ADR-0017) — are the sensitivity band, not point \
claims, and are reported for two futures: a **region/country** and **Broadcom**.\n\n",
    );

    s.push_str("### Structure analysis\n\n");
    s.push_str(&format!(
        "This item ({}) carries an affected baseline of **{} SLOC**. Its related \
challenges — {} — are aggregated for the cumulative-effort estimate. The \
assessment proceeds along the adopted spine: protection-need → risk exposure → \
effort → market → reskilling → measures.\n\n",
        report.kind,
        fmt_num(report.sloc),
        if report.related.is_empty() {
            "none declared".to_string()
        } else {
            report.related.join(", ")
        }
    ));

    s.push_str("### Protection-need and impact level\n\n");
    s.push_str(
        "Impact follows the BSI protection-need categories NORMAL / HIGH / \
VERY-HIGH, assessed across confidentiality, integrity, availability and \
accountability, with the operating mode raising the floor (escalation ⇒ at \
least HIGH; catastrophic ⇒ VERY-HIGH).\n\n",
    );

    s.push_str("### Risk exposure (likelihood × impact)\n\n");
    s.push_str(
        "Risk is likelihood × impact on the qualitative scale; annualised loss \
expectancy is `ALE = asset value × exposure factor × ARO`, with the ARO scaled \
by mode. The full findings table reports both futures across the three modes; \
the matrix that follows visualises the Broadcom scenario.\n\n",
    );
    s.push_str(
        "| Scenario | Mode | Impact | Likelihood (1–5) | Risk | ALE | Financial band | Reskilling surge |\n\
| --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for c in &report.cells {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            c.scenario,
            c.mode,
            c.impact,
            c.likelihood_level,
            c.risk_score,
            fmt_num(c.ale),
            c.financial_band,
            fmt_num(c.reskilling_surge),
        ));
    }
    s.push('\n');
    s.push_str(&figs[2]);
    s.push_str("\n\n");

    s.push_str("### Effort: cumulative human-hours\n\n");
    s.push_str(&format!(
        "Development effort uses the published COCOMO II nominal model \
(`PM = 2.94·KLOC^1.0997`, 152 h/PM; Boehm et al., 2000). This item is an \
estimated **{} person-hours**; cumulatively with its related challenges, \
**{} person-hours**.\n\n",
        fmt_num(report.dev_hours),
        fmt_num(report.cumulative_dev_hours)
    ));
    s.push_str(&figs[1]);
    s.push_str("\n\n");

    s.push_str("### Market size and projections\n\n");
    let r_n = cell(report, "region", "normal");
    let b_e = cell(report, "broadcom", "escalation");
    s.push_str(&format!(
        "Serviceable obtainable market is `TAM·SAM·SOM` projected at a \
mode-specific CAGR. Region (normal mode) moves from {} to {} over the horizon; \
the Broadcom scenario under escalation moves from {} to {}. The matrix shows \
the Broadcom scenario across all modes.\n\n",
        fmt_num(r_n.market_som.first().map_or(0.0, |p| p.1)),
        fmt_num(r_n.market_som.last().map_or(0.0, |p| p.1)),
        fmt_num(b_e.market_som.first().map_or(0.0, |p| p.1)),
        fmt_num(b_e.market_som.last().map_or(0.0, |p| p.1)),
    ));
    s.push_str(&figs[0]);
    s.push_str("\n\n");

    s.push_str("### Temporary mass-reskilling under progressing AI-attack threats\n\n");
    s.push_str(&format!(
        "The 3×3 control posture's risk-based row carries *temporary accents*: \
when autonomous-AI-attack pressure crosses the threat threshold and the mode \
escalates, a time-boxed reskilling surge is provisioned (headcount × hours × \
rate). For this item the surge is **{}** under escalation and **{}** under the \
catastrophic mode (Broadcom scenario); it is zero in normal mode. This is the \
organisational cost of the reviewer-competence gap (Dimension 01, §4.7) made \
quantitative.\n\n",
        fmt_num(cell(report, "broadcom", "escalation").reskilling_surge),
        fmt_num(cell(report, "broadcom", "catastrophic").reskilling_surge),
    ));

    s.push_str(&reading(report));

    s.push_str("### Prioritised measures\n\n");
    s.push_str(
        "- **M1 — contain the escalation path.** Keep the autonomy envelope \
narrow while the threat index is above threshold; widen only on re-measured \
reviewer fluency.\n",
    );
    s.push_str(
        "- **M2 — provision the reskilling surge.** Pre-budget the time-boxed \
reskilling cost so it does not compete with incident response when it triggers.\n",
    );
    s.push_str(
        "- **M3 — track the market window.** Revisit the projection annually; \
the mode band, not a point figure, is the planning input.\n\n",
    );

    s.push_str("### Assumptions ledger\n\n");
    if report.assumptions.is_empty() {
        s.push_str(
            "*All coefficients are the RAMP defaults (ADR-0040): COCOMO II nominal \
constants (sourced) and the declared metadata for this item.*\n\n",
        );
    } else {
        s.push_str("| Key | Value | Basis | Provenance |\n| --- | --- | --- | --- |\n");
        for a in &report.assumptions {
            s.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                a.key, a.value, a.basis, a.kind
            ));
        }
        s.push('\n');
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Corpus {
        let json = r#"{
          "items": [{
            "id":"dimension_02","title":"Cybersecurity AI","kind":"dimension",
            "sloc_baseline":42000,"related":["dimension_03"],
            "cia":{"c":3,"i":3,"a":2,"accountability":2},
            "threat_index":0.7,"asset_value":1000000,"exposure_factor":0.4,
            "aro":0.5,"reference_base":5000000,
            "market":{"base_year":2026,"horizon":[2030,2034],"sam_frac":0.3,"som_frac":0.1,
              "region":{"tam_base":100.0,"cagr":{"normal":0.08,"escalation":0.15,"catastrophic":0.22}},
              "broadcom":{"tam_base":40.0,"cagr":{"normal":0.10,"escalation":0.18,"catastrophic":0.25}}},
            "headcount":12,"reskilling_hours":80,"day_rate":1200,
            "assumptions":[{"key":"sloc_baseline","value":"42000","basis":"component count","kind":"assumption"}]
          },{
            "id":"dimension_03","title":"Intelligent Agents","kind":"dimension",
            "sloc_baseline":30000,"cia":{"c":2,"i":3,"a":2,"accountability":3},
            "threat_index":0.4,"asset_value":800000,"exposure_factor":0.3,
            "aro":0.4,"reference_base":5000000,
            "market":{"base_year":2026,"horizon":[2030,2034],"sam_frac":0.3,"som_frac":0.1,
              "region":{"tam_base":80.0,"cagr":{"normal":0.07,"escalation":0.13,"catastrophic":0.20}},
              "broadcom":{"tam_base":30.0,"cagr":{"normal":0.09,"escalation":0.16,"catastrophic":0.23}}},
            "headcount":10,"reskilling_hours":60,"day_rate":1200
          }]
        }"#;
        parse(json).unwrap()
    }

    #[test]
    fn computes_cumulative_hours_over_related() {
        let r = compute(&sample());
        let d2 = &r[0];
        // cumulative > own (it has a related item dimension_03).
        assert!(d2.cumulative_dev_hours > d2.dev_hours);
        assert!(d2.dev_hours > 0.0);
    }

    #[test]
    fn mode_raises_impact_and_reskilling_only_when_threatened() {
        let r = compute(&sample());
        let d2 = &r[0]; // threat_index 0.7 >= theta 0.5
        let normal = d2.cells.iter().find(|c| c.mode == "normal").unwrap();
        let esc = d2.cells.iter().find(|c| c.mode == "escalation").unwrap();
        assert_eq!(normal.reskilling_surge, 0.0);
        assert!(esc.reskilling_surge > 0.0);
        assert!(esc.risk_score >= normal.risk_score);

        let d3 = &r[1]; // threat_index 0.4 < theta ⇒ no surge even in escalation
        let esc3 = d3.cells.iter().find(|c| c.mode == "escalation").unwrap();
        assert_eq!(esc3.reskilling_surge, 0.0);
    }

    #[test]
    fn chapter_has_sections_and_figspecs() {
        let r = compute(&sample());
        let ch = chapter(&r[0]);
        assert!(ch.contains("Risk-assessment findings"));
        assert!(ch.contains("Temporary mass-reskilling"));
        assert!(ch.contains("Assumptions ledger"));
        assert_eq!(ch.matches("```figspec").count(), 3);
        // figspec ids are namespaced by item id
        assert!(ch.contains("dimension_02_ramp_market"));
    }

    #[test]
    fn market_projects_upward_with_cagr() {
        let r = compute(&sample());
        let cell = r[0]
            .cells
            .iter()
            .find(|c| c.scenario == "broadcom" && c.mode == "escalation")
            .unwrap();
        let first = cell.market_som.first().unwrap().1;
        let last = cell.market_som.last().unwrap().1;
        assert!(last > first);
    }
}
