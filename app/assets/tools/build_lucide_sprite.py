"""Build app/assets/lucide-sprite.svg from the `lucide-static` npm package.

Usage:
    npm pack lucide-static@1.64.0 && tar -xzf lucide-static-1.64.0.tgz
    python app/assets/tools/build_lucide_sprite.py path/to/package

Every icon (aliases included) becomes `<symbol id="lucide-<name>">`. Stroke
presentation attributes are removed so icons inherit stroke, width, caps
and joins from the referencing `<svg>`; `fill="none"` is dropped but
intentional `fill="currentColor"` dots are kept.
"""

import os
import re
import sys

STRIP = re.compile(r'\s(?:stroke|stroke-width|stroke-linecap|stroke-linejoin|class)="[^"]*"')
FILL_NONE = re.compile(r'\sfill="none"')
TAG = re.compile(r"<(path|circle|rect|line|polyline|polygon|ellipse)\b([^>]*?)\s*/?>", re.S)


def symbol(name: str, svg: str) -> str:
    inner = svg.split(">", 1)[1] if svg.lstrip().startswith("<svg") else svg
    m = re.search(r"<svg\b[^>]*>(.*)</svg>", svg, re.S)
    inner = m.group(1) if m else inner
    out = []
    for tag, attrs in TAG.findall(inner):
        attrs = STRIP.sub("", attrs)
        attrs = FILL_NONE.sub("", attrs)
        attrs = re.sub(r"\s+", " ", attrs).strip()
        out.append(f"<{tag} {attrs}/>" if attrs else f"<{tag}/>")
    return f'<symbol id="lucide-{name}" viewBox="0 0 24 24">{"".join(out)}</symbol>'


def main() -> None:
    pkg = sys.argv[1] if len(sys.argv) > 1 else "lucide-static/package"
    icons_dir = os.path.join(pkg, "icons")
    here = os.path.dirname(os.path.abspath(__file__))
    out_path = os.path.join(here, "..", "lucide-sprite.svg")
    names = sorted(f[:-4] for f in os.listdir(icons_dir) if f.endswith(".svg"))
    parts = ['<svg xmlns="http://www.w3.org/2000/svg" style="display:none">']
    for n in names:
        with open(os.path.join(icons_dir, n + ".svg"), encoding="utf-8") as f:
            parts.append(symbol(n, f.read()))
    parts.append("</svg>")
    with open(out_path, "w", encoding="utf-8", newline="\n") as f:
        f.write("".join(parts))
    print(f"{len(names)} icons -> {os.path.normpath(out_path)} ({os.path.getsize(out_path)} bytes)")


if __name__ == "__main__":
    main()
