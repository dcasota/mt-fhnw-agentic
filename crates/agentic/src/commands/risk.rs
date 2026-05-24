//! `agentic risk` — RAMP estimator (ADR-0040).

use std::io::Read;

use anyhow::{Context, Result, bail};

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

pub fn run(action: RiskAction, _json: bool) -> Result<()> {
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
    }
}
