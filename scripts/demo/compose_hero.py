#!/usr/bin/env python3
"""Sets a screenshot on the README's backdrop: ink black lit by warm
bone-white light (the logo's colors), with bloom, vignette and film grain,
the window framed with rounded corners and a soft shadow.

    python scripts/demo/compose_hero.py shot.png docs/assets/hero.jpg

Needs Pillow and NumPy (`pip install pillow numpy`).
"""

import argparse

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

INK = "#060607"
# Light sources: center (x, y as fractions), color, strength, radii
# (fractions of the height) and tilt.
LIGHTS = [
    ((0.92, -0.05), "#f4f0e6", 1.25, (0.75, 0.55), 0.5),   # bone-white, from the top right
    ((0.60, 0.20), "#d8cdb8", 0.25, (0.90, 0.40), 0.3),    # its warm spill
    ((0.05, 1.05), "#2a2c33", 0.60, (0.70, 0.70), 0.0),    # a cool lift in the far corner
]


def linear(hex_color):
    c = np.array([int(hex_color[i:i + 2], 16) for i in (1, 3, 5)], np.float32) / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def to_srgb(c):
    c = np.clip(c, 0, 1)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * np.power(c, 1 / 2.4) - 0.055)


def blur(a, sigma):
    """Gaussian blur of a float array, as three box blurs."""
    r = int(round((np.sqrt(4 * sigma * sigma + 1) - 1) / 2))
    out = a.astype(np.float32)
    for _ in range(3):
        for axis in (0, 1):
            pad = [(0, 0)] * out.ndim
            pad[axis] = (r + 1, r)
            c = np.cumsum(np.pad(out, pad, mode="edge"), axis=axis, dtype=np.float64)
            n = out.shape[axis]
            hi = np.take(c, np.arange(2 * r + 1, 2 * r + 1 + n), axis=axis)
            lo = np.take(c, np.arange(n), axis=axis)
            out = ((hi - lo) / (2 * r + 1)).astype(np.float32)
    return out


def backdrop(w, h, seed=1, grain=0.075):
    y, x = np.mgrid[0:h, 0:w].astype(np.float32)
    x /= w
    y /= h
    aspect = w / h
    rgb = np.ones((h, w, 3), np.float32) * linear(INK)
    for (cx, cy), color, strength, (rx, ry), tilt in LIGHTS:
        dx, dy = (x - cx) * aspect, y - cy
        u = (dx * np.cos(tilt) + dy * np.sin(tilt)) / rx
        v = (-dx * np.sin(tilt) + dy * np.cos(tilt)) / ry
        rgb += np.exp(-(u * u + v * v))[..., None] * linear(color) * strength

    # Bloom, then a filmic curve so the light rolls off instead of clipping.
    bright = np.clip(rgb - 0.25, 0, None)
    rgb += np.stack([blur(bright[..., i], h / 18) for i in range(3)], -1) * 0.35
    rgb = rgb / (1 + rgb * 0.55) * 1.3
    vx, vy = (x - 0.5) * 2, (y - 0.5) * 2
    rgb *= np.clip(1 - 0.35 * np.clip(np.hypot(vx * 0.85, vy) - 0.25, 0, None) ** 1.8, 0, 1)[..., None]
    out = to_srgb(rgb)

    # Film grain: fine and clumped luminance noise with a little color,
    # strongest in the midtones.
    rng = np.random.default_rng(seed)
    fine = rng.normal(0, 1, (h, w)).astype(np.float32)
    clump = blur(rng.normal(0, 1, (h, w)).astype(np.float32), 0.85) * 2.1
    chroma = rng.normal(0, 1, (h, w, 3)).astype(np.float32) * 0.22
    lum = out.mean(-1, keepdims=True)
    out += (fine[..., None] * 0.6 + clump[..., None] * 0.55 + chroma) * grain * (0.6 + 1.6 * lum * (1 - lum))
    return Image.fromarray((np.clip(out, 0, 1) * 255 + 0.5).astype(np.uint8), "RGB")


def rounded(size, radius):
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size[0] - 1, size[1] - 1), radius, fill=255)
    return mask


def compose(shot_path, out_path, width, seed):
    shot = Image.open(shot_path).convert("RGB")
    # The window fills most of the frame; a thin band of backdrop frames it.
    win_w = int(width * 0.92)
    win_h = round(shot.height * win_w / shot.width)
    shot = shot.resize((win_w, win_h), Image.LANCZOS)
    pad_x = (width - win_w) // 2
    pad_top = int(win_h * 0.065)
    height = pad_top * 2 + win_h
    canvas = backdrop(width, height, seed).convert("RGBA")

    radius = max(12, win_w // 110)
    mask = rounded(shot.size, radius)
    # Two shadows: a wide soft one and a close contact one.
    for blur_px, offset, alpha in ((win_w // 28, win_w // 60, 160), (win_w // 160, win_w // 400, 130)):
        shadow = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
        shadow.paste(Image.new("RGBA", shot.size, (4, 4, 6, alpha)), (pad_x, pad_top + offset), mask)
        canvas = Image.alpha_composite(canvas, shadow.filter(ImageFilter.GaussianBlur(blur_px)))
    canvas.paste(shot, (pad_x, pad_top), mask)
    # A hairline edge so the dark window reads against the dark corner.
    edge = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    ImageDraw.Draw(edge).rounded_rectangle((pad_x, pad_top, pad_x + win_w - 1, pad_top + win_h - 1), radius,
                                           outline=(255, 255, 255, 40), width=max(1, win_w // 1800))
    canvas = Image.alpha_composite(canvas, edge).convert("RGB")
    if out_path.lower().endswith((".jpg", ".jpeg")):
        canvas.save(out_path, quality=90, subsampling=0, optimize=True, progressive=True)
    else:
        canvas.save(out_path, optimize=True)
    return canvas.size


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("shot")
    ap.add_argument("out")
    ap.add_argument("--width", type=int, default=3200)
    ap.add_argument("--seed", type=int, default=1)
    args = ap.parse_args()
    print(compose(args.shot, args.out, args.width, args.seed))


if __name__ == "__main__":
    main()
