//! `agentic audit` — PQC (ML-DSA-87) non-repudiation signing + complete audit
//! reports. See ADR-0039 (PQC-only) and the AUDIT guide.
//!
//! Key storage: ML-DSA-87 secret keys are ~4.9 KB, which exceeds the OS
//! keychain blob limit (Windows Credential Manager ~2.5 KB). They are therefore
//! stored as a file under the user data dir (outside the repo and git), the way
//! `ssh`/`minisign` store signing keys. Protect or passphrase-wrap that file
//! for production use. Only the public key + signatures live in the database.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::json;

use agentic_core::{audit, signing};

use crate::cli::AuditAction;

/// Absolute path to the secret-key file for `key_id` (created dir as needed).
fn key_path(key_id: &str) -> Result<PathBuf> {
    let base = dirs::data_dir().ok_or_else(|| anyhow!("cannot locate user data dir"))?;
    let dir = base.join("agentic").join("keys");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join(format!("{key_id}.mldsa87.sk")))
}

fn read_secret(sk_ref: &str) -> Result<String> {
    std::fs::read_to_string(sk_ref)
        .with_context(|| format!("reading secret key at {sk_ref}"))
        .map(|s| s.trim().to_owned())
}

pub fn run(db_path: &Path, action: AuditAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        AuditAction::Keygen { signer } => {
            let kp = signing::generate()?;
            let path = key_path(&kp.key_id)?;
            std::fs::write(&path, &kp.secret_hex)
                .with_context(|| format!("writing secret key to {}", path.display()))?;
            #[cfg(windows)]
            restrict_windows_acl(&path);
            let sk_ref = path.to_string_lossy().to_string();
            signing::register_key(&conn, &kp.key_id, &kp.public_hex, &sk_ref, Some(&signer))?;
            if json_out {
                println!(
                    "{}",
                    json!({ "alg": signing::ALG, "key_id": kp.key_id, "sk_ref": sk_ref, "signer": signer })
                );
            } else {
                println!(
                    "Generated {} key {}\n  secret  -> {}  (protect this file; it is NOT in git)\n  public  -> crypto_keys (active)\n  signer  -> {}",
                    signing::ALG,
                    kp.key_id,
                    sk_ref,
                    signer
                );
            }
        }

        AuditAction::SignCommits { project } => {
            let key = signing::active_key(&conn)?.ok_or_else(|| {
                anyhow!("no active signing key — run `agentic audit keygen` first")
            })?;
            let secret = read_secret(&key.sk_ref)?;
            let shas: Vec<String> = {
                let mut stmt = conn.prepare("SELECT sha256 FROM commits ORDER BY timestamp")?;
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<_, _>>()?
            };
            let mut signed = 0usize;
            for sha in &shas {
                let sig = signing::sign(&secret, sha.as_bytes())?;
                signing::record_signature(
                    &conn,
                    "commit",
                    sha,
                    &key.key_id,
                    &sig,
                    key.signer.as_deref(),
                )?;
                signed += 1;
            }
            if json_out {
                println!(
                    "{}",
                    json!({ "signed": signed, "key_id": key.key_id, "project": project })
                );
            } else {
                println!(
                    "Signed {signed} commits with {} key {}",
                    signing::ALG,
                    key.key_id
                );
            }
        }

        AuditAction::Verify { project } => {
            let shas: Vec<String> = {
                let mut stmt = conn.prepare("SELECT sha256 FROM commits")?;
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<_, _>>()?
            };
            let (mut ok, mut bad, mut unsigned) = (0usize, 0usize, 0usize);
            for sha in &shas {
                let sigs = signing::signatures_for(&conn, "commit", sha)?;
                if sigs.is_empty() {
                    unsigned += 1;
                    continue;
                }
                for s in sigs {
                    let Some(k) = signing::key_by_id(&conn, &s.key_id)? else {
                        bad += 1;
                        continue;
                    };
                    if signing::verify(&k.public_key, sha.as_bytes(), &s.signature)? {
                        ok += 1;
                    } else {
                        bad += 1;
                    }
                }
            }
            if json_out {
                println!(
                    "{}",
                    json!({ "valid": ok, "invalid": bad, "unsigned": unsigned, "project": project })
                );
            } else {
                println!(
                    "Signature verification: {ok} valid, {bad} invalid, {unsigned} unsigned commits"
                );
                if bad > 0 {
                    bail!("{bad} invalid signature(s) — chain integrity FAILED");
                }
            }
        }

        AuditAction::Record {
            project,
            agent,
            action,
            target,
            result,
            model,
            tokens,
            iteration,
            detail,
        } => {
            let sidecar = detail.as_ref().map(|d| json!({ "detail": d }).to_string());
            conn.execute(
                "INSERT INTO audit_rows (project_id, iteration, agent, action, target, result, sidecar_json, tokens_used, model) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![project, iteration, agent, action, target, result, sidecar, tokens, model],
            )?;
            if json_out {
                println!(
                    "{}",
                    json!({ "recorded": true, "agent": agent, "action": action })
                );
            } else {
                println!("Recorded LLM decision: {agent} / {action}");
            }
        }

        AuditAction::Report {
            project,
            item,
            format,
            to,
        } => {
            let rep = audit::compile(&conn, &project, item.as_deref())?;
            let body = if format == "json" {
                serde_json::to_string_pretty(&rep)?
            } else {
                audit::render_markdown(&rep)
            };

            // Sign the rendered body (PQC non-repudiation) if a key is active.
            let mut output = body.clone();
            if let Some(key) = signing::active_key(&conn)? {
                if let Ok(secret) = read_secret(&key.sk_ref) {
                    let digest = signing::digest_hex(body.as_bytes());
                    let sig = signing::sign(&secret, body.as_bytes())?;
                    signing::record_signature(
                        &conn,
                        "audit_report",
                        &digest,
                        &key.key_id,
                        &sig,
                        key.signer.as_deref(),
                    )?;
                    if format == "md" {
                        output.push_str(&format!(
                            "\n## 8 Cryptographic signature\n\n- Algorithm: {}\n- Key id: {}\n- Body SHA-256: `{}`\n- Signature (ML-DSA-87, hex, truncated): `{}…`\n\nVerify by recomputing the body SHA-256 and checking the signature against the public key in `crypto_keys`.\n",
                            signing::ALG, key.key_id, digest, &sig[..sig.len().min(64)]
                        ));
                    }
                }
            }

            match to {
                Some(path) => {
                    std::fs::write(&path, output.as_bytes())
                        .with_context(|| format!("writing {}", path.display()))?;
                    if !json_out {
                        println!("Wrote audit report to {}", path.display());
                    }
                }
                None => {
                    std::io::stdout().write_all(output.as_bytes())?;
                }
            }
        }

        AuditAction::Profile {
            project,
            section,
            to,
        } => {
            let all = agentic_core::audit_profile::compute(&conn, &project)?;
            // Optional --section filter; warn if the slug is unknown.
            let filtered: Vec<_> = if let Some(ref sl) = section {
                match agentic_core::audit_profile::Section::from_slug(sl) {
                    Some(want) => all.into_iter().filter(|p| p.section == want).collect(),
                    None => {
                        anyhow::bail!(
                            "unknown section '{sl}'; valid: dimensions, campaigns, projects, student_notes, master_thesis, agentic_handbook, audit, norms, frontmatter, other"
                        )
                    }
                }
            } else {
                all
            };
            let body = if json_out {
                serde_json::to_string_pretty(&filtered)?
            } else {
                agentic_core::audit_profile::render_markdown(&filtered)
            };
            match to {
                Some(path) => {
                    std::fs::write(&path, body.as_bytes())
                        .with_context(|| format!("writing {}", path.display()))?;
                    println!("Wrote audit profile to {}", path.display());
                }
                None => {
                    std::io::stdout().write_all(body.as_bytes())?;
                }
            }
        }
    }
    Ok(())
}

/// Best-effort: restrict the secret-key file to the current user (Windows).
#[cfg(windows)]
fn restrict_windows_acl(path: &Path) {
    use std::process::Command;
    let p = path.to_string_lossy().to_string();
    let _ = Command::new("icacls")
        .args([
            &p,
            "/inheritance:r",
            "/grant:r",
            &format!("{}:F", whoami_user()),
        ])
        .output();
}

#[cfg(windows)]
fn whoami_user() -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "%USERNAME%".into())
}
