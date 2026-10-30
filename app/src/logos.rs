//! Brand logos for agents, model vendors, providers and dev tools.
//!
//! SVGs live in `app/assets/logos/` (see `LICENSES.md` there) and are
//! compiled in; every file is normalized to fill its container.

use dioxus::prelude::*;
use larrier_core::AgentKind;

macro_rules! logos {
    ($($id:literal),* $(,)?) => {
        /// Every bundled logo as `(id, svg)`.
        pub static ALL: &[(&str, &str)] = &[
            $(($id, include_str!(concat!("../assets/logos/", $id, ".svg"))),)*
        ];
    };
}

logos!(
    "claude-code", "claude-code-mono", "cursor", "opencode", "gemini-cli",
    "gemini-cli-mono", "pi", "hermes", "goose", "amp", "amp-mono", "githubcopilot", "copilot", "kiro",
    "cline", "roocode", "kilocode", "windsurf", "trae", "junie", "claude", "claude-mono", "anthropic",
    "openai", "google", "google-mono", "gemini", "gemini-mono", "xai", "grok", "deepseek",
    "deepseek-mono", "mistral", "mistral-mono", "meta", "meta-mono", "openrouter", "ollama", "groq",
    "together", "perplexity", "cohere", "minimax", "zhipu", "chatglm", "moonshot", "kimi", "kimi-mono",
    "qwen", "qwen-mono", "nous", "huggingface", "lmstudio", "azure", "microsoft", "bedrock", "aws",
    "vertex", "inflection", "vercel", "github", "figma", "notion", "git", "npm", "node", "python", "rust",
    "docker", "linear", "bash", "vscodium", "droid", "aider", "vscode", "powershell", "terminal", "slack",
    "larrier",
);

/// Look up a bundled logo by its file id.
pub fn logo(id: &str) -> Option<&'static str> {
    ALL.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
}

fn agent_logo_id(kind: &AgentKind) -> Option<&'static str> {
    Some(match kind {
        AgentKind::ClaudeCode => "claude-code",
        // Codex is part of the ChatGPT app now: the OpenAI mark, drawn in the
        // text color (no badge behind it).
        AgentKind::Codex => "openai",
        AgentKind::Cursor => "cursor",
        AgentKind::OpenCode => "opencode",
        AgentKind::Gemini => "gemini-cli",
        AgentKind::Pi => "pi",
        AgentKind::Hermes => "hermes",
        AgentKind::Goose => "goose",
        AgentKind::Amp => "amp",
        AgentKind::Copilot => "githubcopilot",
        AgentKind::Qwen => "qwen",
        AgentKind::Kimi => "kimi",
        AgentKind::Droid => "droid",
        AgentKind::Aider => "aider",
        AgentKind::Chat => "larrier",
        AgentKind::ChatGpt => "openai",
        AgentKind::ClaudeAi => "claude",
        AgentKind::Custom(_) => return None,
    })
}

/// Color logo for an agent / harness (`None` for custom agents).
pub fn agent_logo(kind: &AgentKind) -> Option<&'static str> {
    agent_logo_id(kind).and_then(logo)
}

/// Monochrome (`currentColor`) variant when one exists, else the color logo.
pub fn agent_logo_mono(kind: &AgentKind) -> Option<&'static str> {
    let id = agent_logo_id(kind)?;
    logo(&format!("{id}-mono")).or_else(|| logo(id))
}

/// Logo for a provider / vendor / tool id (e.g. `openrouter`, `github`).
pub fn provider_logo(id: &str) -> Option<&'static str> {
    let id = id.trim().to_ascii_lowercase();
    let key = match id.as_str() {
        "x-ai" | "x.ai" => "xai",
        "google-gemini" | "googlegemini" => "gemini",
        "llama" | "meta-llama" => "meta",
        "zai" | "z-ai" | "glm" | "zhipuai" => "zhipu",
        "moonshotai" => "moonshot",
        "alibaba" | "qwen-code" => "qwen",
        "nousresearch" => "nous",
        "hf" | "hugging-face" => "huggingface",
        "lm-studio" => "lmstudio",
        "azure-openai" | "azureopenai" => "azure",
        "aws-bedrock" | "amazon-bedrock" => "bedrock",
        "vertexai" | "vertex-ai" | "google-vertex" => "vertex",
        "visualstudiocode" | "vs-code" | "code" => "vscode",
        "windows-terminal" | "windowsterminal" | "pwsh" => "powershell",
        "nodejs" | "node.js" | "nodedotjs" => "node",
        "togetherai" | "together-ai" => "together",
        "openai-compatible" | "openai_compatible" => "openai",
        "mistralai" => "mistral",
        other => other,
    };
    logo(key)
}

/// Vendor logo for a model id such as `claude-opus-4-5`, `gpt-5-codex`,
/// `anthropic/claude-sonnet-4.5` or `openrouter/x-ai/grok-4`.
pub fn model_logo(model_id: &str) -> Option<&'static str> {
    let m = model_id.trim().to_ascii_lowercase();
    if m.is_empty() {
        return None;
    }
    // Names first: "openrouter/anthropic/claude-…" should show Claude, not
    // OpenRouter; fall back to the provider prefix.
    let name = m.rsplit('/').next().unwrap_or(&m);
    let by_name = |n: &str| -> Option<&'static str> {
        let has = |s: &str| n.contains(s);
        let starts = |s: &str| n.starts_with(s);
        let id = if has("claude") || has("opus") || has("sonnet") || has("haiku") || has("fable") {
            "claude"
        } else if starts("gpt") || starts("o1") || starts("o3") || starts("o4") || has("codex") || starts("chatgpt") {
            "openai"
        } else if has("gemini") || has("gemma") {
            "gemini"
        } else if has("grok") {
            "grok"
        } else if has("deepseek") {
            "deepseek"
        } else if has("mistral") || has("codestral") || has("devstral") || has("magistral") || has("pixtral") {
            "mistral"
        } else if has("llama") {
            "meta"
        } else if has("qwen") || has("qwq") {
            "qwen"
        } else if has("kimi") || has("moonshot") {
            "kimi"
        } else if has("glm") || has("chatglm") {
            "zhipu"
        } else if has("minimax") {
            "minimax"
        } else if has("hermes") {
            "nous"
        } else if has("composer") || has("cursor") {
            "cursor"
        } else if has("command-r") || has("cohere") {
            "cohere"
        } else if has("sonar") {
            "perplexity"
        } else {
            return None;
        };
        logo(id)
    };
    by_name(name).or_else(|| {
        let mut parts = m.split('/');
        let first = parts.next().unwrap_or("");
        if m.contains('/') {
            // `provider/vendor/model` → try the vendor segment, then the provider.
            parts.next().and_then(provider_logo).or_else(|| provider_logo(first))
        } else {
            None
        }
    })
}

/// Inline brand logo sized to a square box.
#[component]
pub fn Logo(svg: &'static str, #[props(default = 16)] size: u32) -> Element {
    rsx! {
        span {
            class: "logo-svg",
            style: "display:inline-flex;flex-shrink:0;width:{size}px;height:{size}px;line-height:0;",
            dangerous_inner_html: svg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_agent_has_a_logo() {
        for spec in larrier_core::agent::catalog() {
            if matches!(spec.kind, AgentKind::Custom(_) | AgentKind::Chat) {
                continue;
            }
            assert!(agent_logo(&spec.kind).is_some(), "no logo for {}", spec.name);
        }
    }

    #[test]
    fn lookups() {
        assert!(model_logo("claude-opus-4-5").is_some());
        assert_eq!(model_logo("anthropic/claude-sonnet-4.5"), logo("claude"));
        assert_eq!(model_logo("gpt-5-codex"), logo("openai"));
        assert_eq!(model_logo("openrouter/x-ai/grok-4"), logo("grok"));
        assert_eq!(model_logo("ollama/some-local-thing"), logo("ollama"));
        assert!(model_logo("totally-unknown").is_none());
        assert!(provider_logo("openrouter").is_some());
        assert!(provider_logo("github").is_some());
        assert!(provider_logo("powershell").is_some());
        for id in [
            "anthropic", "openai", "google", "gemini", "xai", "deepseek", "mistral", "meta", "openrouter", "ollama",
            "groq", "together", "perplexity", "cohere", "minimax", "zhipu", "moonshot", "qwen", "nous", "huggingface",
            "lmstudio", "azure", "bedrock", "vertex", "github", "git", "vscode", "cursor", "powershell", "npm", "node",
            "python", "rust", "docker", "slack", "linear",
        ] {
            assert!(provider_logo(id).is_some(), "missing provider logo {id}");
        }
    }

    #[test]
    fn logos_fill_their_box() {
        for (id, svg) in ALL {
            assert!(svg.starts_with("<svg"), "{id}");
            assert!(svg.contains("width=\"100%\""), "{id}");
        }
    }
}
