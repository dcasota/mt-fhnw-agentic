//! Provider routing decision logic.
//!
//! Resolves which provider to use for a given task following the priority
//! described in [`crate`]'s module docs.

use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliContext {
    ClaudeCode,
    Cursor,
    GeminiCli,
    OpenAiCodex,
    FactoryAi,
    Unknown,
}

#[must_use]
pub fn detect_cli_context() -> CliContext {
    if env::var("CLAUDECODE").is_ok() { return CliContext::ClaudeCode; }
    if env::var("CURSOR_TRACE_ID").is_ok() { return CliContext::Cursor; }
    if env::var("GEMINI_CLI").is_ok() { return CliContext::GeminiCli; }
    if env::var("CODEX_SESSION").is_ok() { return CliContext::OpenAiCodex; }
    if env::var("FACTORYAI").is_ok() { return CliContext::FactoryAi; }
    CliContext::Unknown
}

/// Pick a default provider for the detected CLI context.
#[must_use]
pub fn provider_for_context(ctx: CliContext) -> &'static str {
    match ctx {
        CliContext::ClaudeCode => "anthropic",
        CliContext::Cursor => "anthropic",
        CliContext::GeminiCli => "google",
        CliContext::OpenAiCodex => "openai",
        CliContext::FactoryAi => "anthropic",
        CliContext::Unknown => "anthropic", // fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_falls_back_to_anthropic() {
        assert_eq!(provider_for_context(CliContext::Unknown), "anthropic");
    }

    #[test]
    fn gemini_cli_routes_to_google() {
        assert_eq!(provider_for_context(CliContext::GeminiCli), "google");
    }
}
