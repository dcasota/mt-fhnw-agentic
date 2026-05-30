//! Provider routing decision logic.
//!
//! Priority chain (highest first):
//!
//! 1. **CLI-context auto-detect** — env vars that the invoking AI CLI sets
//!    (e.g. `CLAUDECODE` → Anthropic, `GEMINI_CLI` → Google).
//! 2. **Per-task config** — `AGENTIC_<TASK>_PROVIDER` env var (e.g.
//!    `AGENTIC_JUDGE_PROVIDER=openai`). Persisted form via
//!    `agentic config set llm.judge openai` lives in the project DB.
//! 3. **User default** — `AGENTIC_DEFAULT_PROVIDER` env var.
//! 4. **Hard-coded fallback** — Anthropic.
//!
//! The final selection is materialised as a [`Route`] containing the kind,
//! a model name (per-task defaults), and a human-readable reason.

use std::env;

use crate::{ProviderKind, Route, Task, registry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliContext {
    ClaudeCode,
    Cursor,
    GeminiCli,
    OpenAiCodex,
    FactoryAi,
    GrokBuild,
    Unknown,
}

#[must_use]
pub fn detect_cli_context() -> CliContext {
    if env::var("CLAUDECODE").is_ok() {
        return CliContext::ClaudeCode;
    }
    if env::var("CURSOR_TRACE_ID").is_ok() {
        return CliContext::Cursor;
    }
    if env::var("GEMINI_CLI").is_ok() {
        return CliContext::GeminiCli;
    }
    if env::var("CODEX_SESSION").is_ok() {
        return CliContext::OpenAiCodex;
    }
    if env::var("FACTORYAI").is_ok() {
        return CliContext::FactoryAi;
    }
    if env::var("GROK_BUILD").is_ok() || env::var("XAI_BUILD").is_ok() {
        return CliContext::GrokBuild;
    }
    CliContext::Unknown
}

#[must_use]
pub fn provider_for_context(ctx: CliContext) -> ProviderKind {
    match ctx {
        CliContext::ClaudeCode | CliContext::Cursor | CliContext::FactoryAi => {
            ProviderKind::Anthropic
        }
        CliContext::GeminiCli => ProviderKind::Google,
        CliContext::OpenAiCodex => ProviderKind::OpenAi,
        CliContext::GrokBuild => ProviderKind::Grok,
        CliContext::Unknown => ProviderKind::Anthropic,
    }
}

/// Whether a provider can natively serve a given task.
///
/// Anthropic has no embeddings API; Voyage has no chat API. Everything else
/// is currently considered universal.
#[must_use]
pub fn supports_task(kind: ProviderKind, task: Task) -> bool {
    match (kind, task) {
        (ProviderKind::Anthropic | ProviderKind::Grok, Task::Embed) => false,
        (ProviderKind::Voyage, Task::Embed) => true,
        (ProviderKind::Voyage, _) => false,
        _ => true,
    }
}

/// Default model per (provider, task) — chosen for accuracy on thesis-scale
/// tasks. Override via env var `AGENTIC_MODEL_<PROVIDER>_<TASK>`.
#[must_use]
pub fn default_model(kind: ProviderKind, task: Task) -> &'static str {
    match (kind, task) {
        (ProviderKind::Anthropic, _) => "claude-opus-4-7",
        (ProviderKind::OpenAi, Task::Embed) => "text-embedding-3-large",
        (ProviderKind::OpenAi, _) => "gpt-5",
        (ProviderKind::Google, Task::Embed) => "text-embedding-005",
        (ProviderKind::Google, _) => "gemini-2.5-pro",
        (ProviderKind::Mistral, Task::Embed) => "mistral-embed",
        (ProviderKind::Mistral, _) => "mistral-large-latest",
        (ProviderKind::Cohere, Task::Embed) => "embed-multilingual-v3.0",
        (ProviderKind::Cohere, _) => "command-r-plus",
        (ProviderKind::Voyage, _) => "voyage-3",
        (ProviderKind::Ollama, Task::Embed) => "bge-m3",
        (ProviderKind::Ollama, _) => "llama3:latest",
        (ProviderKind::Grok, _) => "grok-4",
    }
}

/// Resolve which provider + model to use for a given [`Task`].
#[must_use]
pub fn route(task: Task) -> Route {
    let ctx = detect_cli_context();
    if ctx != CliContext::Unknown {
        let kind = provider_for_context(ctx);
        if supports_task(kind, task) {
            return Route {
                kind,
                model: model_or_default(kind, task),
                reason: format!("cli-context:{}", ctx_label(ctx)),
            };
        }
        // CLI context can't serve this task (e.g. embed under Anthropic);
        // fall through to env-var / default chain.
    }

    let task_env = format!("AGENTIC_{}_PROVIDER", task.as_str().to_uppercase());
    if let Ok(name) = env::var(&task_env) {
        if let Ok(kind) = name.parse::<ProviderKind>() {
            if supports_task(kind, task) {
                return Route {
                    kind,
                    model: model_or_default(kind, task),
                    reason: format!("env:{task_env}"),
                };
            }
        }
    }

    if let Ok(name) = env::var("AGENTIC_DEFAULT_PROVIDER") {
        if let Ok(kind) = name.parse::<ProviderKind>() {
            if supports_task(kind, task) {
                return Route {
                    kind,
                    model: model_or_default(kind, task),
                    reason: "env:AGENTIC_DEFAULT_PROVIDER".into(),
                };
            }
        }
    }

    // ADR-0051 §3.2 (2026-05-30) — available-key scan BEFORE the hard
    // fallback. The historical behaviour (commit `9b347a4`+) was to
    // always default Embed → Voyage and everything else → Anthropic
    // even if the user had ZERO key for that vendor but ONE key for
    // another vendor that COULD serve the task. Result: cascade gates
    // `embed inbox` / `classify inbox` reported FAIL when the user
    // (e.g.) had only `GEMINI_API_KEY` set — Google CAN do embeddings,
    // but the router never asked. This block fixes that by walking the
    // preferred-provider order and picking the first that both supports
    // the task AND has a key in env/keychain.
    if let Some(kind) = available_provider_for(task) {
        return Route {
            kind,
            model: model_or_default(kind, task),
            reason: format!("available-key-scan:{}", kind.as_str()),
        };
    }

    // Per-task hard fallback: embeddings → Voyage, anything else → Anthropic.
    // Reached ONLY when no available-key match exists. The cascade gate
    // layer (commands/embed.rs / commands/classify.rs) intercepts this
    // case and converts the resulting build-provider failure into a
    // graceful SKIP per ADR-0051 §3.3.
    let fallback = match task {
        Task::Embed => ProviderKind::Voyage,
        _ => ProviderKind::Anthropic,
    };
    Route {
        kind: fallback,
        model: model_or_default(fallback, task),
        reason: "fallback".into(),
    }
}

/// ADR-0051 §3.2 — preferred-provider order for the available-key scan.
///
/// Ordering rationale (most-capable / cheapest-per-token first within
/// each task class):
/// - **Embed:** Voyage (purpose-built embeddings, best quality) →
///   Google (strong + cheap) → OpenAI → Mistral → Cohere → Ollama
///   (local, last resort). Anthropic and Grok excluded — neither has
///   an embeddings API (`supports_task`).
/// - **Chat / other:** Anthropic → OpenAI → Google → Grok → Mistral
///   → Cohere → Ollama (local, last resort). Voyage excluded —
///   embeddings-only.
///
/// **Ollama policy (user directive, 2026-05-30):** Ollama stays
/// optional at the END of both orders. Its inclusion is gated by a
/// reachability probe in `available_provider_for` so that an
/// unconfigured / not-running Ollama does NOT cause the gate to FAIL —
/// it stays cleanly invisible to the scan, and the SKIP semantics of
/// §3.3 trigger. Per persistent memory `db-source-of-truth-and-pqc-
/// audit` Ollama is never an automatic substitute for a missing cloud
/// provider — but when it IS reachable, it's a valid last-resort
/// fallback.
fn preferred_provider_order(task: Task) -> &'static [ProviderKind] {
    match task {
        Task::Embed => &[
            ProviderKind::Voyage,
            ProviderKind::Google,
            ProviderKind::OpenAi,
            ProviderKind::Mistral,
            ProviderKind::Cohere,
            ProviderKind::Ollama,
        ],
        _ => &[
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::Google,
            ProviderKind::Grok,
            ProviderKind::Mistral,
            ProviderKind::Cohere,
            ProviderKind::Ollama,
        ],
    }
}

/// ADR-0051 §3.2 — Ollama-specific gate. `registry::has_key(Ollama)`
/// returns `true` unconditionally (Ollama needs no key); without an
/// extra check the scan would always pick Ollama as a last-resort
/// fallback even when the Ollama server isn't running, defeating the
/// SKIP semantics. This helper makes the inclusion **opt-out by
/// environment**: Ollama is included only when
/// `AGENTIC_OLLAMA_ENABLE` is explicitly set (default: excluded).
/// Tests stay deterministic (env unset → Ollama not picked → SKIP).
/// Users who run Ollama locally set the var once and Ollama joins the
/// last-resort fallback chain.
fn ollama_available_for_scan() -> bool {
    parse_ollama_enable_flag(env::var("AGENTIC_OLLAMA_ENABLE").ok().as_deref())
}

/// Pure parser for `AGENTIC_OLLAMA_ENABLE`. Truthy values are anything
/// non-empty other than `0` and `false` (case-insensitive). Pulled out
/// so the truth table is testable without env mutation (which the
/// agentic-providers crate forbids via `deny(unsafe_code)`).
#[must_use]
pub fn parse_ollama_enable_flag(v: Option<&str>) -> bool {
    match v {
        None => false,
        Some(s) => !s.is_empty() && s != "0" && !s.eq_ignore_ascii_case("false"),
    }
}

/// ADR-0051 §3.2 — return the first provider in
/// [`preferred_provider_order`] that both `supports_task(task)` AND has
/// a configured key (via `registry::has_key`, which checks
/// `AGENTIC_<PROVIDER>_KEY`, the vendor-native env-var aliases —
/// including `GEMINI_API_KEY` for `google` per ADR-0051 §3.1 — and
/// finally the OS keychain). Returns `None` when nothing is available;
/// the caller falls back to the hard-coded default and the gate then
/// SKIPs (per §3.3) rather than FAILing.
#[must_use]
pub fn available_provider_for(task: Task) -> Option<ProviderKind> {
    preferred_provider_order(task)
        .iter()
        .copied()
        .find(|k| {
            // Ollama is in the scan but only when explicitly enabled
            // via `AGENTIC_OLLAMA_ENABLE` — see `ollama_available_for_scan`.
            // This preserves SKIP semantics when Ollama isn't running.
            if *k == ProviderKind::Ollama {
                return ollama_available_for_scan();
            }
            supports_task(*k, task) && registry::has_key(*k)
        })
}

fn model_or_default(kind: ProviderKind, task: Task) -> String {
    let env_name = format!(
        "AGENTIC_MODEL_{}_{}",
        kind.as_str().to_uppercase(),
        task.as_str().to_uppercase()
    );
    env::var(&env_name).unwrap_or_else(|_| default_model(kind, task).into())
}

fn ctx_label(ctx: CliContext) -> &'static str {
    match ctx {
        CliContext::ClaudeCode => "claude-code",
        CliContext::Cursor => "cursor",
        CliContext::GeminiCli => "gemini-cli",
        CliContext::OpenAiCodex => "openai-codex",
        CliContext::FactoryAi => "factory-ai",
        CliContext::GrokBuild => "grok-build",
        CliContext::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resolve() {
        assert_eq!(
            default_model(ProviderKind::Anthropic, Task::Chat),
            "claude-opus-4-7"
        );
        assert_eq!(default_model(ProviderKind::Voyage, Task::Embed), "voyage-3");
        assert_eq!(default_model(ProviderKind::Ollama, Task::Embed), "bge-m3");
    }

    #[test]
    fn gemini_cli_routes_to_google() {
        assert_eq!(
            provider_for_context(CliContext::GeminiCli),
            ProviderKind::Google
        );
    }

    #[test]
    fn anthropic_does_not_support_embed() {
        assert!(!supports_task(ProviderKind::Anthropic, Task::Embed));
        assert!(supports_task(ProviderKind::Anthropic, Task::Chat));
    }

    #[test]
    fn voyage_supports_only_embed() {
        assert!(supports_task(ProviderKind::Voyage, Task::Embed));
        assert!(!supports_task(ProviderKind::Voyage, Task::Chat));
    }

    /// ADR-0051 §3.2 / §3.4 — the preferred-provider order locks the
    /// availability scan. This test is structural (doesn't touch env)
    /// to keep CI deterministic — it verifies the ordering invariants
    /// the ADR codifies.
    ///
    /// A separate integration test that toggles env vars and asserts
    /// `available_provider_for(...)` returns the right kind lives in
    /// `tests/router_available_key.rs` (env-tinkering inside a
    /// `#[cfg(test)]` module would race with concurrent tests).
    #[test]
    fn preferred_provider_order_invariants_per_adr_0051() {
        // Embed: Voyage MUST be first (purpose-built), Google MUST be
        // included (covers the GEMINI_API_KEY case from §3.1), Ollama
        // MUST be LAST (per user directive 2026-05-30: optional last-
        // resort), and Anthropic/Grok MUST be EXCLUDED (no embed API).
        let embed = preferred_provider_order(Task::Embed);
        assert_eq!(embed[0], ProviderKind::Voyage, "embed must start with Voyage");
        assert_eq!(
            *embed.last().unwrap(),
            ProviderKind::Ollama,
            "embed must end with Ollama (last-resort fallback per ADR-0051)"
        );
        assert!(
            embed.contains(&ProviderKind::Google),
            "embed must include Google (handles GEMINI_API_KEY)"
        );
        assert!(
            !embed.contains(&ProviderKind::Anthropic),
            "Anthropic has no embeddings API — must NOT appear in embed order"
        );
        assert!(
            !embed.contains(&ProviderKind::Grok),
            "Grok has no embeddings API — must NOT appear in embed order"
        );

        // Chat: Anthropic MUST be first, Ollama MUST be last, Voyage
        // MUST be EXCLUDED, Google/Grok MUST be included.
        let chat = preferred_provider_order(Task::Chat);
        assert_eq!(chat[0], ProviderKind::Anthropic, "chat must start with Anthropic");
        assert_eq!(
            *chat.last().unwrap(),
            ProviderKind::Ollama,
            "chat must end with Ollama (last-resort fallback per ADR-0051)"
        );
        assert!(
            !chat.contains(&ProviderKind::Voyage),
            "Voyage has no chat API — must NOT appear in chat order"
        );
        assert!(
            chat.contains(&ProviderKind::Google),
            "chat must include Google (covers GEMINI_API_KEY for chat too)"
        );
        assert!(
            chat.contains(&ProviderKind::Grok),
            "chat must include Grok (covers XAI_API_KEY)"
        );

        // Every kind in each order MUST `supports_task` for that task.
        for &k in embed {
            assert!(
                supports_task(k, Task::Embed),
                "{k:?} in embed order but doesn't support embed"
            );
        }
        for &k in chat {
            assert!(
                supports_task(k, Task::Chat),
                "{k:?} in chat order but doesn't support chat"
            );
        }
    }

    /// ADR-0051 Ollama-policy gate: parse_ollama_enable_flag — env-free
    /// truth table. Locks the matrix the user directive (2026-05-30)
    /// codifies: "Ollama optional at the end, no failure if not
    /// configured" → default-off, explicit opt-in.
    #[test]
    fn ollama_opt_in_truth_table_per_adr_0051() {
        // Truthy
        assert!(parse_ollama_enable_flag(Some("1")));
        assert!(parse_ollama_enable_flag(Some("true")));
        assert!(parse_ollama_enable_flag(Some("True")));
        assert!(parse_ollama_enable_flag(Some("TRUE")));
        assert!(parse_ollama_enable_flag(Some("yes")));
        assert!(parse_ollama_enable_flag(Some("on")));
        // Falsy
        assert!(!parse_ollama_enable_flag(None));
        assert!(!parse_ollama_enable_flag(Some("")));
        assert!(!parse_ollama_enable_flag(Some("0")));
        assert!(!parse_ollama_enable_flag(Some("false")));
        assert!(!parse_ollama_enable_flag(Some("False")));
        assert!(!parse_ollama_enable_flag(Some("FALSE")));
    }

    #[test]
    fn parses_kind_aliases() {
        assert_eq!(
            "claude".parse::<ProviderKind>().unwrap(),
            ProviderKind::Anthropic
        );
        assert_eq!(
            "gemini".parse::<ProviderKind>().unwrap(),
            ProviderKind::Google
        );
        assert_eq!(
            "local".parse::<ProviderKind>().unwrap(),
            ProviderKind::Ollama
        );
        assert_eq!("xai".parse::<ProviderKind>().unwrap(), ProviderKind::Grok);
        assert_eq!("grok".parse::<ProviderKind>().unwrap(), ProviderKind::Grok);
    }

    #[test]
    fn grok_build_routes_to_grok() {
        assert_eq!(
            provider_for_context(CliContext::GrokBuild),
            ProviderKind::Grok
        );
    }

    #[test]
    fn grok_does_not_support_embed() {
        assert!(!supports_task(ProviderKind::Grok, Task::Embed));
        assert!(supports_task(ProviderKind::Grok, Task::Chat));
    }
}
