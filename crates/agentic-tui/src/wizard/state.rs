//! Wizard state: the answers a user gives, plus draft persistence.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Languages the wizard offers as the working language.
pub const LANGS: &[&str] = &["en", "de", "fr", "it", "rm", "hi"];

/// The eight providers the wizard knows how to capture keys for.
/// Mirrors [`agentic_providers::ProviderKind`] without depending on it.
pub const PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "mistral",
    "cohere",
    "voyage",
    "ollama",
    "grok",
];

/// Project kinds the wizard can pick.
pub const PROJECT_KINDS: &[&str] = &["thesis", "sub_paper", "standalone", "portfolio_root"];

/// Single-row slot we use for the in-progress draft.
const DRAFT_SLOT: &str = "default";

/// Linear step order. The wizard advances `current_step` through these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    Welcome,
    Name,
    Kind,
    Lang,
    InstitutionTrack,
    ProviderKeys,
    Review,
    Done,
}

impl Step {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Name => "Project name",
            Self::Kind => "Project kind",
            Self::Lang => "Working language",
            Self::InstitutionTrack => "Institution / track",
            Self::ProviderKeys => "Provider keys",
            Self::Review => "Review",
            Self::Done => "Done",
        }
    }

    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Welcome,
            Self::Name,
            Self::Kind,
            Self::Lang,
            Self::InstitutionTrack,
            Self::ProviderKeys,
            Self::Review,
            Self::Done,
        ]
    }

    #[must_use]
    pub fn next(self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|s| *s == self).unwrap_or(0);
        all.get(idx + 1).copied().unwrap_or(Self::Done)
    }

    #[must_use]
    pub fn prev(self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|s| *s == self).unwrap_or(0);
        if idx == 0 {
            Self::Welcome
        } else {
            all[idx - 1]
        }
    }
}

/// Everything the wizard captures. Provider keys live here only in memory;
/// they are written to the OS keychain by the caller and never persisted in
/// the draft (the draft is in plaintext SQLite — keys must not land there).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardState {
    pub current_step: Step,
    pub project_name: String,
    pub kind: String,
    pub working_lang: String,
    pub institution: String,
    pub track: String,
    /// Indices into [`PROVIDERS`] that the user has supplied a key for. Stored
    /// in the draft so the user can see which were already done after `--resume`.
    pub providers_keyed: Vec<usize>,
    /// Provider name → entered key. Always omitted from serialized drafts.
    #[serde(skip)]
    pub provider_keys: Vec<(String, String)>,
}

impl Default for WizardState {
    fn default() -> Self {
        Self {
            current_step: Step::Welcome,
            project_name: String::new(),
            kind: "thesis".into(),
            working_lang: "en".into(),
            institution: "fhnw-mas".into(),
            track: String::new(),
            providers_keyed: Vec::new(),
            provider_keys: Vec::new(),
        }
    }
}

impl WizardState {
    #[must_use]
    pub fn new_with_defaults(working_lang: &str, institution: Option<&str>) -> Self {
        let mut s = Self::default();
        if LANGS.contains(&working_lang) {
            s.working_lang = working_lang.to_owned();
        }
        if let Some(inst) = institution {
            s.institution = inst.to_owned();
        }
        s
    }

    /// True when the user has filled in everything required to create a project.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.project_name.trim().is_empty()
            && PROJECT_KINDS.contains(&self.kind.as_str())
            && LANGS.contains(&self.working_lang.as_str())
    }

    /// Add or overwrite a key for `provider`. Tracks the index in `providers_keyed`.
    pub fn set_key(&mut self, provider: &str, key: String) {
        let Some(idx) = PROVIDERS.iter().position(|p| *p == provider) else {
            return;
        };
        // Replace existing entry if present, else push.
        if let Some(slot) = self.provider_keys.iter_mut().find(|(p, _)| p == provider) {
            slot.1 = key;
        } else {
            self.provider_keys.push((provider.to_owned(), key));
        }
        if !self.providers_keyed.contains(&idx) {
            self.providers_keyed.push(idx);
        }
    }

    pub fn forget_key(&mut self, provider: &str) {
        let Some(idx) = PROVIDERS.iter().position(|p| *p == provider) else {
            return;
        };
        self.provider_keys.retain(|(p, _)| p != provider);
        self.providers_keyed.retain(|i| *i != idx);
    }
}

/// Persist (or replace) the current draft. Excludes provider keys.
pub fn save_draft(conn: &Connection, state: &WizardState) -> Result<()> {
    let json = serde_json::to_string(state).context("serialize wizard state")?;
    conn.execute(
        "INSERT INTO wizard_drafts (slot, state_json) VALUES (?1, ?2)
         ON CONFLICT(slot) DO UPDATE SET state_json = excluded.state_json,
                                         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
        params![DRAFT_SLOT, json],
    )?;
    Ok(())
}

/// Load the draft if one exists.
pub fn load_draft(conn: &Connection) -> Result<Option<WizardState>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT state_json FROM wizard_drafts WHERE slot = ?1",
            params![DRAFT_SLOT],
            |row| row.get(0),
        )
        .optional()?;
    match raw {
        Some(json) => {
            let s: WizardState = serde_json::from_str(&json).context("parse wizard state")?;
            Ok(Some(s))
        }
        None => Ok(None),
    }
}

/// Drop the draft after the wizard succeeds (or the user discards).
pub fn delete_draft(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM wizard_drafts WHERE slot = ?1",
        params![DRAFT_SLOT],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::db::open_in_memory;

    #[test]
    fn step_navigation() {
        assert_eq!(Step::Welcome.next(), Step::Name);
        assert_eq!(Step::Name.prev(), Step::Welcome);
        assert_eq!(Step::Done.next(), Step::Done);
        assert_eq!(Step::Welcome.prev(), Step::Welcome);
    }

    #[test]
    fn defaults_carry_args() {
        let s = WizardState::new_with_defaults("de", Some("custom-inst"));
        assert_eq!(s.working_lang, "de");
        assert_eq!(s.institution, "custom-inst");
    }

    #[test]
    fn invalid_lang_falls_back_to_default() {
        let s = WizardState::new_with_defaults("xx", None);
        assert_eq!(s.working_lang, "en");
    }

    #[test]
    fn is_complete_requires_name_kind_lang() {
        let mut s = WizardState::default();
        assert!(!s.is_complete());
        s.project_name = "My Thesis".into();
        assert!(s.is_complete());
        s.kind = "bogus".into();
        assert!(!s.is_complete());
    }

    #[test]
    fn set_and_forget_keys_tracks_index() {
        let mut s = WizardState::default();
        s.set_key("anthropic", "sk-1".into());
        s.set_key("openai", "sk-2".into());
        assert_eq!(s.providers_keyed.len(), 2);
        assert!(s.providers_keyed.contains(&0));
        s.forget_key("anthropic");
        assert_eq!(s.providers_keyed.len(), 1);
        assert!(!s.providers_keyed.contains(&0));
    }

    #[test]
    fn draft_round_trips_without_keys() {
        let conn = open_in_memory().unwrap();
        let mut s = WizardState::default();
        s.project_name = "Round Trip".into();
        s.set_key("anthropic", "secret-do-not-persist".into());
        save_draft(&conn, &s).unwrap();
        let loaded = load_draft(&conn).unwrap().unwrap();
        assert_eq!(loaded.project_name, "Round Trip");
        // Provider keys must NEVER be persisted in the draft.
        assert!(loaded.provider_keys.is_empty());
        // But the index showing which providers were already keyed survives.
        assert_eq!(loaded.providers_keyed, vec![0]);
        delete_draft(&conn).unwrap();
        assert!(load_draft(&conn).unwrap().is_none());
    }
}
