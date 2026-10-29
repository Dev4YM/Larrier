//! Model registry: every model Larrier knows, with a proper display name,
//! vendor, context window, output cap and per-million-token prices.
//!
//! Two layers:
//! - a curated table: Anthropic's official catalog (prices as published on
//!   2026-10-06) and the aliases agents accept ("opus", "sonnet", …);
//! - a live layer parsed from the community LiteLLM price registry, which the
//!   desktop app refreshes daily and caches on disk ([`parse_litellm`],
//!   [`set_live`]).
//!
//! Unknown ids still get a readable name from [`pretty_name`]
//! ("gpt-6.1-sol" → "GPT-6.1 Sol", "hermes-4-405b" → "Hermes 4 405B").

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Anthropic,
    OpenAI,
    Google,
    XAi,
    Moonshot,
    DeepSeek,
    Alibaba,
    Mistral,
    Meta,
    Zhipu,
    MiniMax,
    Cursor,
    Nous,
    Amazon,
    Microsoft,
    Cohere,
    Other,
}

impl Vendor {
    pub const ALL: [Vendor; 17] = [
        Vendor::Anthropic,
        Vendor::OpenAI,
        Vendor::Google,
        Vendor::XAi,
        Vendor::Moonshot,
        Vendor::DeepSeek,
        Vendor::Alibaba,
        Vendor::Mistral,
        Vendor::Meta,
        Vendor::Zhipu,
        Vendor::MiniMax,
        Vendor::Cursor,
        Vendor::Nous,
        Vendor::Amazon,
        Vendor::Microsoft,
        Vendor::Cohere,
        Vendor::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Vendor::Anthropic => "Anthropic",
            Vendor::OpenAI => "OpenAI",
            Vendor::Google => "Google",
            Vendor::XAi => "xAI",
            Vendor::Moonshot => "Moonshot AI",
            Vendor::DeepSeek => "DeepSeek",
            Vendor::Alibaba => "Alibaba (Qwen)",
            Vendor::Mistral => "Mistral",
            Vendor::Meta => "Meta",
            Vendor::Zhipu => "Z.ai (GLM)",
            Vendor::MiniMax => "MiniMax",
            Vendor::Cursor => "Cursor",
            Vendor::Nous => "Nous Research",
            Vendor::Amazon => "Amazon",
            Vendor::Microsoft => "Microsoft",
            Vendor::Cohere => "Cohere",
            Vendor::Other => "Other",
        }
    }

    /// Best guess from a model id (and optionally the provider it came from).
    pub fn infer(id: &str, provider: &str) -> Vendor {
        let n = normalize(id);
        let has = |s: &str| n.contains(s);
        let starts = |s: &str| n.starts_with(s);
        if has("claude") || starts("opus") || starts("sonnet") || starts("haiku") || starts("fable") || starts("mythos") {
            Vendor::Anthropic
        } else if starts("gpt") || starts("chatgpt") || starts("o1") || starts("o3") || starts("o4") || has("codex") || starts("davinci") {
            Vendor::OpenAI
        } else if has("gemini") || has("gemma") {
            Vendor::Google
        } else if has("grok") {
            Vendor::XAi
        } else if has("kimi") || has("moonshot") {
            Vendor::Moonshot
        } else if has("deepseek") {
            Vendor::DeepSeek
        } else if has("qwen") || has("qwq") {
            Vendor::Alibaba
        } else if has("mistral") || has("codestral") || has("devstral") || has("magistral") || has("pixtral") || has("ministral") {
            Vendor::Mistral
        } else if has("llama") {
            Vendor::Meta
        } else if starts("glm") || has("zhipu") {
            Vendor::Zhipu
        } else if has("minimax") {
            Vendor::MiniMax
        } else if starts("composer") || starts("cursor") {
            Vendor::Cursor
        } else if has("hermes") {
            Vendor::Nous
        } else if starts("nova") || starts("titan") {
            Vendor::Amazon
        } else if starts("phi") {
            Vendor::Microsoft
        } else if starts("command") {
            Vendor::Cohere
        } else {
            match provider {
                "anthropic" => Vendor::Anthropic,
                "openai" | "text-completion-openai" => Vendor::OpenAI,
                "gemini" | "vertex_ai-language-models" => Vendor::Google,
                "xai" => Vendor::XAi,
                "moonshot" => Vendor::Moonshot,
                "deepseek" => Vendor::DeepSeek,
                "dashscope" => Vendor::Alibaba,
                "mistral" | "codestral" => Vendor::Mistral,
                "cohere" | "cohere_chat" => Vendor::Cohere,
                _ => Vendor::Other,
            }
        }
    }
}

/// US dollars per million tokens.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

impl Price {
    fn new(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Price {
        Price { input, output, cache_read: Some(cache_read), cache_write: Some(cache_write) }
    }

    /// Cost in dollars for a token mix.
    pub fn cost(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
        (input as f64 * self.input
            + output as f64 * self.output
            + cache_read as f64 * self.cache_read.unwrap_or(self.input * 0.1)
            + cache_write as f64 * self.cache_write.unwrap_or(self.input * 1.25))
            / 1e6
    }

    /// "$4 / $20" (input / output per million tokens).
    pub fn short(&self) -> String {
        format!("{} / {}", money(self.input), money(self.output))
    }
}

/// "$4", "$0.10", "$2.50".
pub fn money(v: f64) -> String {
    if v >= 1.0 {
        format!("${}", trim_float(v, 2))
    } else if v >= 0.01 {
        format!("${v:.2}")
    } else {
        format!("${}", trim_float(v, 4))
    }
}

fn trim_float(v: f64, max_decimals: usize) -> String {
    let s = format!("{v:.max_decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// "1M", "200K", "128K".
pub fn tokens_short(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        format!("{}M", trim_float(m, 1))
    } else if n >= 1000 {
        format!("{}K", (n as f64 / 1000.0).round() as u64)
    } else {
        n.to_string()
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    #[default]
    Active,
    Legacy,
    Deprecated,
    Retired,
}

impl ModelStatus {
    pub fn label(self) -> &'static str {
        match self {
            ModelStatus::Active => "Active",
            ModelStatus::Legacy => "Legacy",
            ModelStatus::Deprecated => "Deprecated",
            ModelStatus::Retired => "Retired",
        }
    }
}

/// Where a model's data came from.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The vendor's own published catalog (curated in Larrier).
    Official,
    /// The live LiteLLM registry.
    #[default]
    Registry,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelInfo {
    /// Canonical id ([`normalize`]d).
    pub id: String,
    /// Display name ("Claude Opus 5.5").
    pub name: String,
    pub vendor: Vendor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<Price>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub status: ModelStatus,
    /// Short tag: "Default", "Most capable", "Fast", "Project Glasswing".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default)]
    pub source: Source,
}

impl ModelInfo {
    /// Name without the vendor's brand ("Opus 5.5" for "Claude Opus 5.5"),
    /// for places that already show the agent or vendor.
    pub fn short_name(&self) -> String {
        for brand in ["Claude "] {
            if let Some(rest) = self.name.strip_prefix(brand) {
                return rest.to_string();
            }
        }
        self.name.clone()
    }

    /// "1M context · $4 / $20".
    pub fn summary(&self) -> String {
        let mut bits = Vec::new();
        if let Some(c) = self.context {
            bits.push(format!("{} context", tokens_short(c)));
        }
        if let Some(p) = &self.price {
            bits.push(p.short());
        }
        bits.join(" · ")
    }
}

// ---------------------------------------------------------------------------
// Curated catalog
// ---------------------------------------------------------------------------

struct Curated {
    id: &'static str,
    name: &'static str,
    context: u64,
    max_output: u64,
    /// (input, output, cache read, cache write) per MTok; None = not
    /// published in the catalog Larrier ships (the live registry fills it).
    price: Option<(f64, f64, f64, f64)>,
    status: ModelStatus,
    tag: Option<&'static str>,
}

const M: u64 = 1_000_000;
const K: u64 = 1_000;

/// Anthropic's catalog (cached 2026-10-06). Cache writes are the 5-minute
/// rate (1.25× input).
const ANTHROPIC: &[Curated] = &[
    Curated { id: "claude-fable-5-1", name: "Claude Fable 5.1", context: M, max_output: 128 * K, price: Some((10.0, 50.0, 0.25, 12.5)), status: ModelStatus::Active, tag: Some("Most capable") },
    Curated { id: "claude-mythos-5-1", name: "Claude Mythos 5.1", context: M, max_output: 128 * K, price: Some((10.0, 50.0, 0.25, 12.5)), status: ModelStatus::Active, tag: Some("Project Glasswing") },
    Curated { id: "claude-fable-5", name: "Claude Fable 5", context: M, max_output: 128 * K, price: Some((10.0, 50.0, 1.0, 12.5)), status: ModelStatus::Active, tag: None },
    Curated { id: "claude-mythos-5", name: "Claude Mythos 5", context: M, max_output: 128 * K, price: Some((10.0, 50.0, 1.0, 12.5)), status: ModelStatus::Active, tag: Some("Project Glasswing") },
    Curated { id: "claude-opus-5-5", name: "Claude Opus 5.5", context: M, max_output: 128 * K, price: Some((4.0, 20.0, 0.2, 5.0)), status: ModelStatus::Active, tag: Some("Default") },
    Curated { id: "claude-opus-5", name: "Claude Opus 5", context: M, max_output: 128 * K, price: Some((5.0, 25.0, 0.5, 6.25)), status: ModelStatus::Active, tag: None },
    Curated { id: "claude-opus-4-8", name: "Claude Opus 4.8", context: M, max_output: 128 * K, price: Some((5.0, 25.0, 0.5, 6.25)), status: ModelStatus::Active, tag: None },
    Curated { id: "claude-opus-4-7", name: "Claude Opus 4.7", context: M, max_output: 128 * K, price: Some((5.0, 25.0, 0.5, 6.25)), status: ModelStatus::Active, tag: None },
    Curated { id: "claude-opus-4-6", name: "Claude Opus 4.6", context: M, max_output: 128 * K, price: Some((5.0, 25.0, 0.5, 6.25)), status: ModelStatus::Active, tag: None },
    Curated { id: "claude-sonnet-5-5", name: "Claude Sonnet 5.5", context: M, max_output: 128 * K, price: Some((2.0, 10.0, 0.2, 2.5)), status: ModelStatus::Active, tag: Some("Balanced") },
    Curated { id: "claude-sonnet-5", name: "Claude Sonnet 5", context: M, max_output: 128 * K, price: Some((2.0, 10.0, 0.2, 2.5)), status: ModelStatus::Active, tag: None },
    Curated { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", context: M, max_output: 128 * K, price: Some((3.0, 15.0, 0.3, 3.75)), status: ModelStatus::Active, tag: None },
    Curated { id: "claude-haiku-5-5", name: "Claude Haiku 5.5", context: M, max_output: 128 * K, price: Some((0.1, 0.5, 0.01, 0.125)), status: ModelStatus::Active, tag: Some("Fast") },
    Curated { id: "claude-haiku-4-5", name: "Claude Haiku 4.5", context: 200 * K, max_output: 64 * K, price: Some((1.0, 5.0, 0.1, 1.25)), status: ModelStatus::Active, tag: None },
    Curated { id: "claude-opus-4-5", name: "Claude Opus 4.5", context: 200 * K, max_output: 64 * K, price: None, status: ModelStatus::Legacy, tag: None },
    Curated { id: "claude-sonnet-4-5", name: "Claude Sonnet 4.5", context: 200 * K, max_output: 64 * K, price: None, status: ModelStatus::Legacy, tag: None },
    Curated { id: "claude-opus-4-1", name: "Claude Opus 4.1", context: 200 * K, max_output: 32 * K, price: None, status: ModelStatus::Deprecated, tag: None },
    Curated { id: "claude-opus-4-0", name: "Claude Opus 4", context: 200 * K, max_output: 32 * K, price: None, status: ModelStatus::Deprecated, tag: None },
    Curated { id: "claude-sonnet-4-0", name: "Claude Sonnet 4", context: 200 * K, max_output: 64 * K, price: None, status: ModelStatus::Deprecated, tag: None },
    Curated { id: "claude-3-haiku", name: "Claude Haiku 3", context: 200 * K, max_output: 4 * K, price: None, status: ModelStatus::Deprecated, tag: None },
];

/// Aliases agents accept (Claude Code: `--model opus`).
const ALIASES: &[(&str, &str, &str)] = &[
    ("opus", "claude-opus-5-5", "Opus"),
    ("sonnet", "claude-sonnet-5-5", "Sonnet"),
    ("haiku", "claude-haiku-5-5", "Haiku"),
    ("fable", "claude-fable-5-1", "Fable"),
    ("claude-opus-4", "claude-opus-4-0", "Claude Opus 4"),
    ("claude-sonnet-4", "claude-sonnet-4-0", "Claude Sonnet 4"),
];

/// What an alias resolves to: (target id, label).
pub fn alias(raw: &str) -> Option<(&'static str, &'static str)> {
    let n = normalize(raw);
    ALIASES.iter().find(|(a, _, _)| *a == n).map(|(_, t, l)| (*t, *l))
}

fn curated() -> &'static HashMap<String, ModelInfo> {
    static C: OnceLock<HashMap<String, ModelInfo>> = OnceLock::new();
    C.get_or_init(|| {
        ANTHROPIC
            .iter()
            .map(|c| {
                (
                    c.id.to_string(),
                    ModelInfo {
                        id: c.id.to_string(),
                        name: c.name.to_string(),
                        vendor: Vendor::Anthropic,
                        context: Some(c.context),
                        max_output: Some(c.max_output),
                        price: c.price.map(|(i, o, r, w)| Price::new(i, o, r, w)),
                        reasoning: true,
                        vision: true,
                        status: c.status,
                        tag: c.tag.map(|t| t.to_string()),
                        source: Source::Official,
                    },
                )
            })
            .collect()
    })
}

fn live() -> &'static RwLock<HashMap<String, ModelInfo>> {
    static L: OnceLock<RwLock<HashMap<String, ModelInfo>>> = OnceLock::new();
    L.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Replace the live layer (from the LiteLLM registry or its disk cache).
pub fn set_live(models: Vec<ModelInfo>) {
    let map: HashMap<String, ModelInfo> = models.into_iter().map(|m| (m.id.clone(), m)).collect();
    if let Ok(mut l) = live().write() {
        *l = map;
    }
}

pub fn live_count() -> usize {
    live().read().map(|l| l.len()).unwrap_or(0)
}

/// Everything known about a model id (curated first, the live registry
/// filling gaps). Aliases resolve to their target.
pub fn lookup(raw: &str) -> Option<ModelInfo> {
    let mut id = normalize(raw);
    if let Some((target, _)) = alias(&id) {
        id = target.to_string();
    }
    let live_hit = live().read().ok().and_then(|l| l.get(&id).cloned());
    match (curated().get(&id).cloned(), live_hit) {
        (Some(mut c), Some(l)) => {
            if c.price.is_none() {
                c.price = l.price;
            }
            Some(c)
        }
        (Some(c), None) => Some(c),
        (None, Some(l)) => Some(l),
        (None, None) => None,
    }
}

/// Display name for any model id ("Claude Opus 5.5", "GPT-6.1 Sol").
pub fn display_name(raw: &str) -> String {
    let n = normalize(raw);
    if n.is_empty() || n == "default" || n == "auto" {
        return "Auto".into();
    }
    if let Some(m) = lookup(raw) {
        return m.name;
    }
    pretty_name(raw)
}

/// Price for a model id, if known.
pub fn price(raw: &str) -> Option<Price> {
    lookup(raw).and_then(|m| m.price)
}

/// Every known model: curated plus the live registry, one entry per id,
/// sorted by vendor, status and name.
pub fn all() -> Vec<ModelInfo> {
    let mut out: HashMap<String, ModelInfo> = live().read().map(|l| l.clone()).unwrap_or_default();
    for (id, c) in curated() {
        let mut c = c.clone();
        if c.price.is_none() {
            c.price = out.get(id).and_then(|l| l.price.clone());
        }
        out.insert(id.clone(), c);
    }
    let mut v: Vec<ModelInfo> = out.into_values().collect();
    // Official models in catalog order (newest first), then the rest by name.
    let rank = |m: &ModelInfo| ANTHROPIC.iter().position(|c| c.id == m.id).unwrap_or(usize::MAX);
    v.sort_by(|a, b| {
        a.vendor
            .cmp(&b.vendor)
            .then((a.status as u8).cmp(&(b.status as u8)))
            .then(rank(a).cmp(&rank(b)))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    v
}

// ---------------------------------------------------------------------------
// Ids and names
// ---------------------------------------------------------------------------

/// Canonical form of a model id: lower case, no provider / region prefix,
/// no "[1m]" suffix, no date or Bedrock version suffix, and Claude versions
/// with dashes ("anthropic/claude-opus-4.5" → "claude-opus-4-5").
pub fn normalize(raw: &str) -> String {
    let mut s = raw.trim().to_lowercase();
    if let Some(i) = s.find('[') {
        s.truncate(i);
    }
    if let Some(last) = s.rsplit('/').next() {
        s = last.to_string();
    }
    for region in ["us.", "eu.", "apac.", "global.", "au.", "jp.", "ca."] {
        if let Some(rest) = s.strip_prefix(region) {
            s = rest.to_string();
        }
    }
    for vendor in ["anthropic.", "amazon.", "meta.", "mistral.", "cohere.", "openai.", "deepseek.", "qwen.", "moonshotai."] {
        if let Some(rest) = s.strip_prefix(vendor) {
            s = rest.to_string();
        }
    }
    // Bedrock "…-v1:0", Vertex "@20251101", dated snapshots "-20250929".
    if let Some(i) = s.find(':') {
        s.truncate(i);
    }
    if let Some(i) = s.find('@') {
        s.truncate(i);
    }
    if let Some(rest) = s.strip_suffix("-v1").or_else(|| s.strip_suffix("-v2")) {
        if rest.starts_with("claude") {
            s = rest.to_string();
        }
    }
    let bytes: Vec<&str> = s.rsplitn(2, '-').collect();
    if bytes.len() == 2 && bytes[0].len() == 8 && bytes[0].starts_with("20") && bytes[0].chars().all(|c| c.is_ascii_digit()) {
        s = bytes[1].to_string();
    }
    if s.starts_with("claude") {
        s = s.replace('.', "-");
        // "claude-3-5-sonnet" style ids keep their order; nothing else to do.
    }
    s
}

/// A readable name for an id nobody catalogued: "gpt-6.1-sol" → "GPT-6.1
/// Sol", "gemini-3-pro" → "Gemini 3 Pro", "claude-3-5-sonnet" → "Claude 3.5
/// Sonnet", "hermes-4-405b" → "Hermes 4 405B".
pub fn pretty_name(raw: &str) -> String {
    let id = normalize(raw);
    if id.is_empty() || id == "default" || id == "auto" {
        return "Auto".into();
    }
    let toks: Vec<&str> = id.split(['-', '_', ' ']).filter(|t| !t.is_empty()).collect();
    let small_num = |t: &str| !t.is_empty() && t.len() <= 2 && t.chars().all(|c| c.is_ascii_digit());
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        // Dashed versions ("4-5" → "4.5") inside a name.
        if small_num(t) && i + 1 < toks.len() && small_num(toks[i + 1]) && !words.is_empty() {
            words.push(format!("{}.{}", t, toks[i + 1]));
            i += 2;
            continue;
        }
        let word = match t {
            "gpt" | "glm" => {
                // "GPT-6.1", "GLM-4.6": the brand hugs its version.
                if let Some(next) = toks.get(i + 1).filter(|n| n.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)) {
                    i += 1;
                    format!("{}-{}", t.to_uppercase(), next)
                } else {
                    t.to_uppercase()
                }
            }
            "chatgpt" => "ChatGPT".into(),
            "deepseek" => "DeepSeek".into(),
            "minimax" => "MiniMax".into(),
            "openai" => "OpenAI".into(),
            "xai" => "xAI".into(),
            "mini" | "nano" | "preview" | "latest" | "turbo" if !words.is_empty() => t.to_string(),
            "ai" => "AI".into(),
            "it" => "IT".into(),
            "vl" | "r1" | "v3" | "v4" | "k2" | "k3" | "m1" | "m2" => t.to_uppercase(),
            _ if t.len() >= 2 && t.starts_with('o') && t[1..].chars().all(|c| c.is_ascii_digit()) => {
                // "o3", "o4-mini": OpenAI writes these lower case and hyphenated.
                if let Some(next) = toks.get(i + 1).filter(|n| matches!(**n, "mini" | "pro" | "deep")) {
                    i += 1;
                    format!("{t}-{next}")
                } else {
                    t.to_string()
                }
            }
            _ if t.ends_with('b') && t.len() > 1 && t[..t.len() - 1].chars().all(|c| c.is_ascii_digit() || c == '.') => t.to_uppercase(),
            _ if t.starts_with('v') && t.len() > 1 && t[1..].chars().all(|c| c.is_ascii_digit() || c == '.') => t.to_uppercase(),
            _ => {
                let mut c = t.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            }
        };
        words.push(word);
        i += 1;
    }
    words.join(" ")
}

// ---------------------------------------------------------------------------
// LiteLLM registry
// ---------------------------------------------------------------------------

/// Where the live registry is fetched from.
pub const LITELLM_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// How much to trust a provider's entry for a model: first-party APIs win
/// over resellers and clouds.
fn provider_rank(provider: &str) -> u8 {
    match provider {
        "anthropic" | "openai" | "gemini" | "xai" | "deepseek" | "mistral" | "moonshot" | "dashscope" | "zai" | "minimax" | "cohere_chat" => 0,
        "openrouter" => 2,
        "" => 3,
        _ => 4,
    }
}

/// Parse LiteLLM's `model_prices_and_context_window.json` into registry
/// entries (chat models only, one per canonical id, the most first-party
/// source winning).
pub fn parse_litellm(json: &str) -> anyhow::Result<Vec<ModelInfo>> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let obj = v.as_object().ok_or_else(|| anyhow::anyhow!("registry is not an object"))?;
    let mut best: HashMap<String, (u8, ModelInfo)> = HashMap::new();
    for (key, e) in obj {
        if key == "sample_spec" {
            continue;
        }
        let mode = e.get("mode").and_then(|m| m.as_str()).unwrap_or("chat");
        if mode != "chat" && mode != "responses" {
            continue;
        }
        let provider = e.get("litellm_provider").and_then(|p| p.as_str()).unwrap_or("");
        let id = normalize(key);
        if id.is_empty() || id.contains(' ') {
            continue;
        }
        let per_m = |k: &str| e.get(k).and_then(|x| x.as_f64()).map(|x| x * 1e6);
        let price = match (per_m("input_cost_per_token"), per_m("output_cost_per_token")) {
            (Some(i), Some(o)) => Some(Price { input: i, output: o, cache_read: per_m("cache_read_input_token_cost"), cache_write: per_m("cache_creation_input_token_cost") }),
            _ => None,
        };
        let int = |k: &str| e.get(k).and_then(|x| x.as_u64());
        let flag = |k: &str| e.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        let status = if e.get("deprecation_date").and_then(|d| d.as_str()).is_some() { ModelStatus::Deprecated } else { ModelStatus::Active };
        let info = ModelInfo {
            id: id.clone(),
            name: pretty_name(&id),
            vendor: Vendor::infer(&id, provider),
            context: int("max_input_tokens").or_else(|| int("max_tokens")),
            max_output: int("max_output_tokens"),
            price,
            reasoning: flag("supports_reasoning"),
            vision: flag("supports_vision"),
            status,
            tag: None,
            source: Source::Registry,
        };
        let rank = provider_rank(provider) + if info.price.is_none() { 5 } else { 0 };
        match best.get(&id) {
            Some((r, _)) if *r <= rank => {}
            _ => {
                best.insert(id, (rank, info));
            }
        }
    }
    Ok(best.into_values().map(|(_, m)| m).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_provider_and_dated_ids() {
        assert_eq!(normalize("anthropic/claude-opus-4.5"), "claude-opus-4-5");
        assert_eq!(normalize("us.anthropic.claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(normalize("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
        assert_eq!(normalize("claude-opus-4-5@20251101"), "claude-opus-4-5");
        assert_eq!(normalize("claude-opus-4-6[1m]"), "claude-opus-4-6");
        assert_eq!(normalize("openrouter/openai/gpt-6.1-sol"), "gpt-6.1-sol");
        assert_eq!(normalize("anthropic.claude-sonnet-4-5-20250929-v1:0"), "claude-sonnet-4-5");
    }

    #[test]
    fn pretty_names() {
        assert_eq!(pretty_name("gpt-6.1-sol"), "GPT-6.1 Sol");
        assert_eq!(pretty_name("gpt-5-codex"), "GPT-5 Codex");
        assert_eq!(pretty_name("gpt-5-mini"), "GPT-5 mini");
        assert_eq!(pretty_name("gemini-3-pro"), "Gemini 3 Pro");
        assert_eq!(pretty_name("gemini-2.5-flash"), "Gemini 2.5 Flash");
        assert_eq!(pretty_name("grok-4"), "Grok 4");
        assert_eq!(pretty_name("opencode/grok-code"), "Grok Code");
        assert_eq!(pretty_name("composer-2"), "Composer 2");
        assert_eq!(pretty_name("nousresearch/hermes-4-405b"), "Hermes 4 405B");
        assert_eq!(pretty_name("claude-3-5-sonnet-20241022"), "Claude 3.5 Sonnet");
        assert_eq!(pretty_name("kimi-k2"), "Kimi K2");
        assert_eq!(pretty_name("deepseek-v3.1"), "DeepSeek V3.1");
        assert_eq!(pretty_name("glm-4.6"), "GLM-4.6");
        assert_eq!(pretty_name("o4-mini"), "o4-mini");
        assert_eq!(pretty_name("qwen3-coder"), "Qwen3 Coder");
        assert_eq!(pretty_name("default"), "Auto");
    }

    #[test]
    fn curated_and_aliases() {
        assert_eq!(display_name("claude-opus-5-5"), "Claude Opus 5.5");
        assert_eq!(display_name("opus"), "Claude Opus 5.5");
        assert_eq!(alias("sonnet"), Some(("claude-sonnet-5-5", "Sonnet")));
        let p = price("claude-opus-5-5").unwrap();
        assert_eq!((p.input, p.output), (4.0, 20.0));
        assert_eq!(p.short(), "$4 / $20");
        assert_eq!(price("claude-haiku-5-5").unwrap().short(), "$0.10 / $0.50");
        assert_eq!(lookup("claude-opus-5-5").unwrap().summary(), "1M context · $4 / $20");
        assert_eq!(lookup("claude-opus-5-5").unwrap().short_name(), "Opus 5.5");
    }

    #[test]
    fn parses_litellm_and_prefers_first_party() {
        let json = r#"{
            "sample_spec": {"mode": "chat"},
            "gpt-9": {"litellm_provider": "openai", "mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 8e-06, "cache_read_input_token_cost": 1e-07, "max_input_tokens": 400000, "max_output_tokens": 128000, "supports_vision": true, "supports_reasoning": true},
            "openrouter/openai/gpt-9": {"litellm_provider": "openrouter", "mode": "chat", "input_cost_per_token": 2e-06, "output_cost_per_token": 9e-06},
            "text-embedding-9": {"litellm_provider": "openai", "mode": "embedding", "input_cost_per_token": 1e-08}
        }"#;
        let v = parse_litellm(json).unwrap();
        assert_eq!(v.len(), 1);
        let m = &v[0];
        assert_eq!(m.id, "gpt-9");
        assert_eq!(m.name, "GPT-9");
        assert_eq!(m.vendor, Vendor::OpenAI);
        let p = m.price.clone().unwrap();
        assert!((p.input - 1.0).abs() < 1e-9 && (p.output - 8.0).abs() < 1e-9);
        assert_eq!(p.cache_read.map(|c| (c * 100.0).round()), Some(10.0));
        assert_eq!(m.context, Some(400_000));
        assert!(m.vision && m.reasoning);
    }

    #[test]
    fn money_and_tokens() {
        assert_eq!(money(4.0), "$4");
        assert_eq!(money(0.1), "$0.10");
        assert_eq!(money(2.5), "$2.5");
        assert_eq!(money(12.5), "$12.5");
        assert_eq!(money(0.005), "$0.005");
        assert_eq!(tokens_short(1_000_000), "1M");
        assert_eq!(tokens_short(1_048_576), "1M");
        assert_eq!(tokens_short(200_000), "200K");
        assert_eq!(tokens_short(922_000), "922K");
    }
}
