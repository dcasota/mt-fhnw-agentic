//! First-class named profile bundles (perception P-1).
//!
//! A profile is a named, reusable JSON configuration bundle the operator
//! can attach to one or more *sections* (a section being one of the
//! `audit_profile::Section` values: dimensions, campaigns, master_thesis,
//! …). The same profile may be shared by multiple sections.
//!
//! Storage: append-only `passport::Section::Profiles`. Each entry's payload
//! is `{"name": "...", "attach_sections": [...], "settings": {...}}`. The
//! latest entry per `name` wins (latest-wins-by-id, mirroring
//! `passport::current`).
//!
//! This module does not interpret `settings` — it stores and resolves them.
//! Consumers (e.g. the bookkit) decide which keys they read.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::passport::{self, Entry, Section as PassportSection};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    /// Unique profile name (slug-like, e.g. "fhnw-mas-thesis-c").
    pub name: String,
    /// Which sections this profile attaches to (slugs from
    /// `audit_profile::Section::slug()`). Empty = unattached / orphan.
    #[serde(default)]
    pub attach_sections: Vec<String>,
    /// Free-form settings bundle. The bookkit + checks decide which keys
    /// they consume. No schema is enforced here — callers may version their
    /// own settings keys.
    #[serde(default)]
    pub settings: Value,
}

/// Persist a profile (latest-wins via passport supersede chain). Returns the
/// newly-inserted entry id.
pub fn put(conn: &Connection, project_id: &str, profile: &Profile) -> Result<i64> {
    let payload = serde_json::to_string(profile)?;
    let prior = list(conn, project_id)?
        .into_iter()
        .find(|(_, p)| p.name == profile.name)
        .map(|(id, _)| id);
    let id = passport::append(
        conn,
        project_id,
        PassportSection::Profiles,
        &payload,
        None,
        prior,
    )?;
    Ok(id)
}

/// Read the latest profile by `name`. Returns `None` if no profile by that
/// name has been persisted (or all prior versions have been superseded by
/// a row whose payload no longer carries that name).
pub fn get(conn: &Connection, project_id: &str, name: &str) -> Result<Option<Profile>> {
    Ok(list(conn, project_id)?
        .into_iter()
        .find(|(_, p)| p.name == name)
        .map(|(_, p)| p))
}

/// List all live profiles (latest entry per name, by entry id). Returns
/// `(entry_id, profile)` tuples sorted by name.
pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<(i64, Profile)>> {
    let entries = passport::current(conn, project_id, PassportSection::Profiles)?;
    // Latest entry per name wins.
    let mut latest: HashMap<String, (i64, Profile)> = HashMap::new();
    for e in entries {
        if let Some(p) = parse_payload(&e) {
            let cur = latest.get(&p.name).map(|(id, _)| *id).unwrap_or(0);
            if e.id > cur {
                latest.insert(p.name.clone(), (e.id, p));
            }
        }
    }
    let mut out: Vec<(i64, Profile)> = latest.into_values().collect();
    out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    Ok(out)
}

/// Resolve the profile attached to a given section slug. If multiple
/// profiles attach to the same section (the "shared profile" case the
/// perception calls out), the latest-id wins — but the operator can
/// always read the others via `list()`.
pub fn resolve_for_section(
    conn: &Connection,
    project_id: &str,
    section_slug: &str,
) -> Result<Option<Profile>> {
    let mut attached: Vec<(i64, Profile)> = list(conn, project_id)?
        .into_iter()
        .filter(|(_, p)| {
            p.attach_sections
                .iter()
                .any(|s| s.eq_ignore_ascii_case(section_slug))
        })
        .collect();
    attached.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(attached.into_iter().next().map(|(_, p)| p))
}

fn parse_payload(e: &Entry) -> Option<Profile> {
    serde_json::from_str::<Profile>(&e.payload_json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::project::{ProjectKind, create as create_project};
    use serde_json::json;

    fn proj() -> (Connection, String) {
        let c = open_in_memory().unwrap();
        let p = create_project(&c, "T", ProjectKind::Thesis, "en", None).unwrap();
        (c, p)
    }

    #[test]
    fn put_then_get_round_trip() {
        let (c, p) = proj();
        let pr = Profile {
            name: "fhnw-mas-thesis-c".into(),
            attach_sections: vec!["master_thesis".into()],
            settings: json!({"page_boundary": 60, "language": "en"}),
        };
        let id = put(&c, &p, &pr).unwrap();
        assert!(id > 0);
        let got = get(&c, &p, "fhnw-mas-thesis-c").unwrap().unwrap();
        assert_eq!(got.name, pr.name);
        assert_eq!(got.attach_sections, pr.attach_sections);
        assert_eq!(got.settings["page_boundary"], 60);
    }

    #[test]
    fn put_supersedes_prior_with_same_name() {
        let (c, p) = proj();
        let v1 = Profile {
            name: "x".into(),
            attach_sections: vec![],
            settings: json!({"v": 1}),
        };
        let v2 = Profile {
            name: "x".into(),
            attach_sections: vec!["dimensions".into()],
            settings: json!({"v": 2}),
        };
        let _ = put(&c, &p, &v1).unwrap();
        let _ = put(&c, &p, &v2).unwrap();
        let got = get(&c, &p, "x").unwrap().unwrap();
        assert_eq!(got.settings["v"], 2);
        assert_eq!(got.attach_sections, vec!["dimensions".to_string()]);
        let all = list(&c, &p).unwrap();
        assert_eq!(all.len(), 1, "supersede keeps the latest only");
    }

    #[test]
    fn list_sorted_and_independent_of_insertion_order() {
        let (c, p) = proj();
        put(
            &c,
            &p,
            &Profile {
                name: "z-profile".into(),
                attach_sections: vec![],
                settings: json!({}),
            },
        )
        .unwrap();
        put(
            &c,
            &p,
            &Profile {
                name: "a-profile".into(),
                attach_sections: vec![],
                settings: json!({}),
            },
        )
        .unwrap();
        let names: Vec<_> = list(&c, &p)
            .unwrap()
            .into_iter()
            .map(|(_, p)| p.name)
            .collect();
        assert_eq!(names, vec!["a-profile", "z-profile"]);
    }

    #[test]
    fn resolve_picks_latest_id_when_multiple_profiles_share_a_section() {
        let (c, p) = proj();
        put(
            &c,
            &p,
            &Profile {
                name: "shared-a".into(),
                attach_sections: vec!["campaigns".into()],
                settings: json!({"hint": "older"}),
            },
        )
        .unwrap();
        put(
            &c,
            &p,
            &Profile {
                name: "shared-b".into(),
                attach_sections: vec!["campaigns".into()],
                settings: json!({"hint": "newer"}),
            },
        )
        .unwrap();
        let r = resolve_for_section(&c, &p, "campaigns").unwrap().unwrap();
        assert_eq!(r.name, "shared-b");
        // case-insensitive section slug lookup
        let r2 = resolve_for_section(&c, &p, "Campaigns").unwrap();
        assert!(r2.is_some());
        // unattached section -> None
        assert!(resolve_for_section(&c, &p, "norms").unwrap().is_none());
    }
}
