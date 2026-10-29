"""Build app/assets/logos/*.svg from LobeHub Icons and Simple Icons.

Usage:
    npm pack @lobehub/icons-static-svg@1.101.0 simple-icons@16.36.0
    (extract both tarballs)
    python app/assets/tools/build_logos.py <lobe-package-dir> <simple-icons-package-dir>

Each logo is normalized for inline use: prolog/comments/<title> removed,
root width/height/style replaced by width="100%" height="100%", ids made
unique per file, whitespace collapsed. Simple Icons get their brand color.
Brands neither library ships are hand-drawn marks in CUSTOM below (CC0).
"""

import json
import os
import re
import sys

# id -> (source, file).  source: "lobe" | "si"
LOGOS = {
    # agents / harnesses
    "claude-code": ("lobe", "claudecode-color.svg"),
    "claude-code-mono": ("lobe", "claudecode.svg"),
    "codex": ("lobe", "codex-color.svg"),
    "codex-mono": ("lobe", "codex.svg"),
    "cursor": ("lobe", "cursor.svg"),
    "opencode": ("lobe", "opencode.svg"),
    "gemini-cli": ("lobe", "geminicli-color.svg"),
    "gemini-cli-mono": ("lobe", "geminicli.svg"),
    "pi": ("lobe", "pi.svg"),
    "hermes": ("lobe", "hermesagent.svg"),
    "goose": ("lobe", "goose.svg"),
    "amp": ("lobe", "amp-color.svg"),
    "amp-mono": ("lobe", "amp.svg"),
    "githubcopilot": ("lobe", "githubcopilot.svg"),
    "copilot": ("lobe", "copilot-color.svg"),
    "kiro": ("lobe", "kiro-color.svg"),
    "cline": ("lobe", "cline.svg"),
    "roocode": ("lobe", "roocode.svg"),
    "kilocode": ("lobe", "kilocode.svg"),
    "windsurf": ("lobe", "windsurf.svg"),
    "trae": ("lobe", "trae-color.svg"),
    "junie": ("lobe", "junie-color.svg"),
    # model vendors / providers
    "claude": ("lobe", "claude-color.svg"),
    "claude-mono": ("lobe", "claude.svg"),
    "anthropic": ("lobe", "anthropic.svg"),
    "openai": ("lobe", "openai.svg"),
    "google": ("lobe", "google-color.svg"),
    "google-mono": ("lobe", "google.svg"),
    "gemini": ("lobe", "gemini-color.svg"),
    "gemini-mono": ("lobe", "gemini.svg"),
    "xai": ("lobe", "xai.svg"),
    "grok": ("lobe", "grok.svg"),
    "deepseek": ("lobe", "deepseek-color.svg"),
    "deepseek-mono": ("lobe", "deepseek.svg"),
    "mistral": ("lobe", "mistral-color.svg"),
    "mistral-mono": ("lobe", "mistral.svg"),
    "meta": ("lobe", "meta-color.svg"),
    "meta-mono": ("lobe", "meta.svg"),
    "openrouter": ("lobe", "openrouter.svg"),
    "ollama": ("lobe", "ollama.svg"),
    "groq": ("lobe", "groq.svg"),
    "together": ("lobe", "together-color.svg"),
    "perplexity": ("lobe", "perplexity-color.svg"),
    "cohere": ("lobe", "cohere-color.svg"),
    "minimax": ("lobe", "minimax-color.svg"),
    "zhipu": ("lobe", "zhipu-color.svg"),
    "chatglm": ("lobe", "chatglm-color.svg"),
    "moonshot": ("lobe", "moonshot.svg"),
    "kimi": ("lobe", "kimi-color.svg"),
    "kimi-mono": ("lobe", "kimi.svg"),
    "qwen": ("lobe", "qwen-color.svg"),
    "qwen-mono": ("lobe", "qwen.svg"),
    "nous": ("lobe", "nousresearch.svg"),
    "huggingface": ("lobe", "huggingface-color.svg"),
    "lmstudio": ("lobe", "lmstudio.svg"),
    "azure": ("lobe", "azureai-color.svg"),
    "microsoft": ("lobe", "microsoft-color.svg"),
    "bedrock": ("lobe", "bedrock-color.svg"),
    "aws": ("lobe", "aws-color.svg"),
    "vertex": ("lobe", "vertexai-color.svg"),
    "inflection": ("lobe", "inflection.svg"),
    "vercel": ("lobe", "vercel.svg"),
    # dev tools / integrations
    "github": ("lobe", "github.svg"),
    "figma": ("lobe", "figma-color.svg"),
    "notion": ("lobe", "notion.svg"),
    "git": ("si", "git"),
    "npm": ("si", "npm"),
    "node": ("si", "nodedotjs"),
    "python": ("si", "python"),
    "rust": ("si", "rust"),
    "docker": ("si", "docker"),
    "linear": ("si", "linear"),
    "bash": ("si", "gnubash"),
    "vscodium": ("si", "vscodium"),
}

# Hand-drawn marks for brands neither library ships (CC0).
CUSTOM = {
    "droid": '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="1" y="1" width="22" height="22" rx="6" fill="#EE6018"/><path d="M8 6.5v-2M16 6.5v-2" stroke="#fff" stroke-width="1.6" stroke-linecap="round"/><rect x="5" y="7" width="14" height="11" rx="3.5" fill="#fff"/><circle cx="9.5" cy="12.5" r="1.6" fill="#EE6018"/><circle cx="14.5" cy="12.5" r="1.6" fill="#EE6018"/></svg>',
    "aider": '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="1" y="1" width="22" height="22" rx="6" fill="#14B014"/><path d="M8.5 8 5 12l3.5 4M15.5 8 19 12l-3.5 4M13.2 6.5l-2.4 11" fill="none" stroke="#fff" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    "vscode": '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="1" y="1" width="22" height="22" rx="6" fill="#0078D4"/><path d="M15.5 5.5v13L8 12.6l-2.6 2.2L4 13.9V10l1.4-.9L8 11.4zM8 11.4 15.5 5.5M8 12.6l7.5 5.9" fill="none" stroke="#fff" stroke-width="1.5" stroke-linejoin="round"/><path d="m15.5 5.5 3 1.5v10l-3 1.5z" fill="#fff"/></svg>',
    "powershell": '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="1" y="1" width="22" height="22" rx="6" fill="#012456"/><path d="m6.5 8 4.5 4-4.5 4M12.5 16.5h5" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    "terminal": '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="1" y="1" width="22" height="22" rx="6" fill="#2b2b2b"/><path d="m6.5 8 4.5 4-4.5 4M12.5 16.5h5" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    "slack": '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="8.2" y="2" width="3.6" height="11" rx="1.8" fill="#36C5F0"/><rect x="2" y="12.2" width="11" height="3.6" rx="1.8" fill="#E01E5A"/><rect x="12.2" y="11" width="3.6" height="11" rx="1.8" fill="#ECB22E"/><rect x="11" y="8.2" width="11" height="3.6" rx="1.8" fill="#2EB67D"/></svg>',
    "larrier": '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="1" y="1" width="22" height="22" rx="6" fill="#26251E"/><path d="M8.5 5.5v12h8" fill="none" stroke="#F7F7F4" stroke-width="3" stroke-linecap="square"/></svg>',
}


def normalize(logo_id: str, svg: str) -> str:
    svg = re.sub(r"<\?xml[^>]*\?>", "", svg)
    svg = re.sub(r"<!--.*?-->", "", svg, flags=re.S)
    svg = re.sub(r"<title>.*?</title>", "", svg, flags=re.S)

    def root(m: re.Match) -> str:
        attrs = m.group(1)
        attrs = re.sub(r'\s(?:width|height|style|role|class)="[^"]*"', "", attrs)
        return f'<svg{attrs} width="100%" height="100%">'

    svg = re.sub(r"<svg\b([^>]*)>", root, svg, count=1)
    ids = re.findall(r'\sid="([^"]+)"', svg)
    for i in sorted(set(ids), key=len, reverse=True):
        new = f"lg-{logo_id}-{i}"
        svg = svg.replace(f'id="{i}"', f'id="{new}"')
        svg = svg.replace(f"url(#{i})", f"url(#{new})")
        svg = svg.replace(f'href="#{i}"', f'href="#{new}"')
    svg = re.sub(r">\s+<", "><", svg)
    return re.sub(r"\s+", " ", svg).strip()


def main() -> None:
    lobe = sys.argv[1]
    si = sys.argv[2]
    here = os.path.dirname(os.path.abspath(__file__))
    out = os.path.join(here, "..", "logos")
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(si, "data", "simple-icons.json"), encoding="utf-8") as f:
        si_data = {d["slug"]: d for d in json.load(f)}
    total = 0
    for logo_id, (src, name) in LOGOS.items():
        if src == "lobe":
            with open(os.path.join(lobe, "icons", name), encoding="utf-8") as f:
                svg = f.read()
        else:
            with open(os.path.join(si, "icons", name + ".svg"), encoding="utf-8") as f:
                svg = f.read()
            hexc = si_data.get(name, {}).get("hex", "888888")
            svg = svg.replace("<path ", f'<path fill="#{hexc}" ', 1)
        text = normalize(logo_id, svg)
        with open(os.path.join(out, logo_id + ".svg"), "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
        total += len(text)
    for logo_id, svg in CUSTOM.items():
        text = normalize(logo_id, svg)
        with open(os.path.join(out, logo_id + ".svg"), "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
        total += len(text)
    print(f"{len(LOGOS) + len(CUSTOM)} logos, {total} bytes -> {os.path.normpath(out)}")


if __name__ == "__main__":
    main()
