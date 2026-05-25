//! `agentic risk` — RAMP estimator (ADR-0040).

use std::io::Read;

use agentic_risk::invest::{self, Params};
use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::cli::RiskAction;

fn read_input(input: &str) -> Result<String> {
    if input == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("read RAMP corpus from stdin")?;
        Ok(s)
    } else {
        std::fs::read_to_string(input).with_context(|| format!("read RAMP corpus {input}"))
    }
}

pub fn run(action: RiskAction, json_out: bool) -> Result<()> {
    match action {
        RiskAction::Compute { input, item } => {
            let corpus = agentic_risk::parse(&read_input(&input)?)?;
            let mut reports = agentic_risk::compute(&corpus);
            if let Some(id) = item {
                reports.retain(|r| r.id == id);
                if reports.is_empty() {
                    bail!("no item with id '{id}' in the corpus");
                }
            }
            println!("{}", serde_json::to_string_pretty(&reports)?);
            Ok(())
        }
        RiskAction::Chapter { input, item } => {
            let corpus = agentic_risk::parse(&read_input(&input)?)?;
            let reports = agentic_risk::compute(&corpus);
            let report = reports
                .iter()
                .find(|r| r.id == item)
                .with_context(|| format!("no item with id '{item}' in the corpus"))?;
            print!("{}", agentic_risk::chapter(report));
            Ok(())
        }
        RiskAction::Graphics { input } => {
            let corpus = agentic_risk::parse(&read_input(&input)?)?;
            let reports = agentic_risk::compute(&corpus);
            print!("{}", agentic_risk::graphics(&reports));
            Ok(())
        }
        RiskAction::Invest {
            challenges,
            region,
            token_price,
            scenario,
            tokens_per_month,
            parity,
            escalation_premium,
            disaster_premium,
        } => run_invest(
            challenges.as_deref(),
            &region,
            token_price,
            scenario.as_deref(),
            tokens_per_month,
            parity,
            escalation_premium,
            disaster_premium,
            json_out,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_invest(
    challenges_path: Option<&std::path::Path>,
    region: &str,
    token_price: f64,
    scenario: Option<&str>,
    tokens_per_month: f64,
    parity: f64,
    escalation_premium: f64,
    disaster_premium: f64,
    json_out: bool,
) -> Result<()> {
    if !(parity > 0.0 && parity <= 1.0) {
        bail!("--parity must satisfy 0 < parity <= 1 (got {parity})");
    }

    // Token price: an explicit --scenario preset wins over --token-price.
    let token_price_per_mtok = match scenario {
        Some(name) => invest::scenario_token_price(name)
            .with_context(|| format!("unknown --scenario '{name}' (baseline-2025 | asic-2027)"))?,
        None => token_price,
    };

    let challenges = match challenges_path {
        Some(path) => {
            let body = std::fs::read_to_string(path)
                .with_context(|| format!("read challenges {}", path.display()))?;
            invest::parse_challenges(&body)?
        }
        None => invest::demo_challenges(),
    };

    let params = Params {
        human_rate: invest::region_rate(region)?,
        token_price_per_mtok,
        tokens_per_month,
        parity,
        escalation_premium,
        disaster_premium,
    };

    let est = invest::estimate(&challenges, &params);
    let sens = invest::sensitivity(&challenges, &params);

    if json_out {
        let out = json!({
            "params": {
                "region": region,
                "human_rate": params.human_rate,
                "token_price_per_mtok": params.token_price_per_mtok,
                "tokens_per_month": params.tokens_per_month,
                "parity": params.parity,
                "escalation_premium": params.escalation_premium,
                "disaster_premium": params.disaster_premium,
            },
            "estimate": est,
            "sensitivity": sens,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    print_invest_human(region, &params, &est, &sens);
    Ok(())
}

fn print_invest_human(
    region: &str,
    params: &Params,
    est: &invest::Estimate,
    sens: &invest::Sensitivity,
) {
    println!("AI-vs-human investment estimate (ADR-0040)");
    println!(
        "  region {region} (${:.0}/h)  token ${:.2}/Mtok  tokens/mo {:.0}M  parity {:.2}  esc×{:.2}  dis×{:.2}",
        params.human_rate,
        params.token_price_per_mtok,
        params.tokens_per_month,
        params.parity,
        params.escalation_premium,
        params.disaster_premium,
    );
    println!();
    println!(
        "{:<34} {:>5} {:>11} {:>11} {:>9} {:>13} {:>13} {:>8}",
        "challenge", "risk", "human_h", "irred_h", "ag_mo", "ai_cost", "human_cost", "ai_share",
    );
    let sep = "-".repeat(34 + 5 + 11 + 11 + 9 + 13 + 13 + 8 + 7);
    println!("{sep}");
    for r in &est.challenges {
        println!(
            "{:<34} {:>5} {:>11} {:>11} {:>9.1} {:>13} {:>13} {:>7.1}%",
            truncate(&r.name, 34),
            r.risk.label(),
            commas(r.human_hours),
            commas(r.irreducible_human_hours),
            r.ai_agent_months,
            money(r.ai_cost),
            money(r.human_cost),
            r.ai_share * 100.0,
        );
    }
    let t = &est.totals;
    println!("{sep}");
    println!(
        "{:<34} {:>5} {:>11} {:>11} {:>9.1} {:>13} {:>13} {:>7.1}%",
        "TOTAL",
        "",
        commas(t.human_hours),
        commas(t.irreducible_human_hours),
        t.ai_agent_months,
        money(t.ai_cost),
        money(t.human_cost),
        t.ai_share * 100.0,
    );

    println!();
    println!("Sensitivity band (vary parity × token-scenario × region):");
    println!(
        "{:<6} {:>7} {:>15} {:>7} {:>9} {:>15} {:>9}",
        "band", "parity", "scenario", "region", "$/h", "total_cost", "ai_share",
    );
    let band_sep = "-".repeat(6 + 7 + 15 + 7 + 9 + 15 + 9 + 6);
    println!("{band_sep}");
    for r in &sens.rows {
        println!(
            "{:<6} {:>7.2} {:>15} {:>7} {:>9.0} {:>15} {:>8.1}%",
            r.label,
            r.parity,
            r.scenario,
            r.region,
            r.human_rate,
            money(r.total_cost),
            r.ai_share * 100.0,
        );
    }
    println!("{band_sep}");
    println!(
        "ai_share range: {:.1}% – {:.1}%  (direction robust: AI stays the minority share; \
level varies materially)",
        sens.ai_share_min * 100.0,
        sens.ai_share_max * 100.0,
    );
}

/// Thousands-separated integer (rounded).
fn commas(x: f64) -> String {
    #[allow(clippy::cast_possible_truncation)]
    let n = x.round() as i64;
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// USD with a `$` prefix and thousands separators (rounded to dollars).
fn money(x: f64) -> String {
    format!("${}", commas(x))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
