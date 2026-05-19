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

use crate::{ProviderKind, Route, Task};

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

    // Per-task hard fallback: embeddings → Voyage, anything else → Anthropic.
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
