//! `agentic check calibration` — reviewer FNR/FPR calibration (ADR-0044).
//!
//! The ARS "Reviewer Calibration Mode" measures false-negative / false-positive
//! rates against a user-supplied gold set (targets FNR < 0.15, FPR < 0.10). This
//! gate reads a gold set from `out/calibration_gold.json` and computes the rates.
//!
//! The file is either a confusion-count object
//! `{"tp":N,"fp":N,"fn":N,"tn":N}` or an array of labelled samples
//! `[{"gold":true,"pred":false}, …]`. When no gold set exists the gate is INFO
//! (not applicable); when present it FAILs (`CALIBRATION_FNR` / `CALIBRATION_FPR`)
//! if a rate breaches its threshold.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

pub const FNR_MAX: f64 = 0.15;
pub const FPR_MAX: f64 = 0.10;
const GOLD_PATH: &str = "out/calibration_gold.json";

/// `(fnr, fpr, balanced_accuracy)` from confusion counts.
#[must_use]
pub fn calibrate(tp: u64, fp: u64, fn_: u64, tn: u64) -> (f64, f64, f64) {
    let fnr = if tp + fn_ == 0 {
        0.0
    } else {
        fn_ as f64 / (tp + fn_) as f64
    };
    let fpr = if fp + tn == 0 {
        0.0
    } else {
        fp as f64 / (fp + tn) as f64
    };
    let tpr = 1.0 - fnr;
    let tnr = 1.0 - fpr;
    (fnr, fpr, (tpr + tnr) / 2.0)
}

/// Parse confusion counts `(tp, fp, fn, tn)` from the gold-set JSON.
#[must_use]
pub fn counts_from_json(v: &Value) -> Option<(u64, u64, u64, u64)> {
    if let Some(o) = v.as_object() {
        let g = |k: &str| o.get(k).and_then(Value::as_u64);
        if let (Some(tp), Some(fp), Some(fn_), Some(tn)) = (g("tp"), g("fp"), g("fn"), g("tn")) {
            return Some((tp, fp, fn_, tn));
        }
    }
    if let Some(arr) = v.as_array() {
        let (mut tp, mut fp, mut fn_, mut tn) = (0u64, 0u64, 0u64, 0u64);
        for s in arr {
            let gold = s.get("gold").and_then(Value::as_bool)?;
            let pred = s.get("pred").and_then(Value::as_bool)?;
            match (gold, pred) {
                (true, true) => tp += 1,
                (false, true) => fp += 1,
                (true, false) => fn_ += 1,
                (false, false) => tn += 1,
            }
        }
        return Some((tp, fp, fn_, tn));
    }
    None
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();

    let blob = worktree::read_at(conn, project, GOLD_PATH).ok();
    let Some(blob) = blob else {
        findings.push(Finding {
            category: "CALIBRATION_NA".into(),
            severity: Severity::Info,
            message: format!("no gold set at {GOLD_PATH} — reviewer calibration not scored"),
            location: Some("calibration".into()),
        });
        return Ok(CheckReport::new("calibration", findings));
    };

    let text = String::from_utf8_lossy(&blob.content);
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        findings.push(Finding {
            category: "CALIBRATION_PARSE".into(),
            severity: Severity::Warn,
            message: format!("{GOLD_PATH} is not valid JSON"),
            location: Some(GOLD_PATH.into()),
        });
        return Ok(CheckReport::new("calibration", findings));
    };
    let Some((tp, fp, fn_, tn)) = counts_from_json(&v) else {
        findings.push(Finding {
            category: "CALIBRATION_PARSE".into(),
            severity: Severity::Warn,
            message: "gold set has neither tp/fp/fn/tn counts nor gold/pred samples".into(),
            location: Some(GOLD_PATH.into()),
        });
        return Ok(CheckReport::new("calibration", findings));
    };

    let (fnr, fpr, bal) = calibrate(tp, fp, fn_, tn);
    if fnr >= FNR_MAX {
        findings.push(Finding {
            category: "CALIBRATION_FNR".into(),
            severity: Severity::Error,
            message: format!("FNR {fnr:.3} ≥ {FNR_MAX} — reviewer misses too many true issues"),
            location: Some(GOLD_PATH.into()),
        });
    }
    if fpr >= FPR_MAX {
        findings.push(Finding {
            category: "CALIBRATION_FPR".into(),
            severity: Severity::Error,
            message: format!("FPR {fpr:.3} ≥ {FPR_MAX} — reviewer raises too many false issues"),
            location: Some(GOLD_PATH.into()),
        });
    }
    findings.push(Finding {
        category: "CALIBRATION_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "FNR {fnr:.3} (≤{FNR_MAX}), FPR {fpr:.3} (≤{FPR_MAX}), balanced-accuracy {bal:.3} (tp={tp} fp={fp} fn={fn_} tn={tn})"
        ),
        location: Some("calibration".into()),
    });
    Ok(CheckReport::new("calibration", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_computed() {
        // tp=9 fn=1 → fnr 0.10; fp=1 tn=9 → fpr 0.10.
        let (fnr, fpr, bal) = calibrate(9, 1, 1, 9);
        assert!((fnr - 0.10).abs() < 1e-9);
        assert!((fpr - 0.10).abs() < 1e-9);
        assert!((bal - 0.90).abs() < 1e-9);
    }

    #[test]
    fn parse_both_shapes() {
        let counts = serde_json::json!({"tp":5,"fp":0,"fn":0,"tn":5});
        assert_eq!(counts_from_json(&counts), Some((5, 0, 0, 5)));
        let samples = serde_json::json!([
            {"gold":true,"pred":true},
            {"gold":true,"pred":false},
            {"gold":false,"pred":false}
        ]);
        assert_eq!(counts_from_json(&samples), Some((1, 0, 1, 1)));
    }
}
