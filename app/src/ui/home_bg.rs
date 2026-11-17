//! The picture behind the new chat screen: drawn with an effect (dither,
//! grain, vignette), its brightness, blur and opacity, fading into the
//! page; and the settings for it, with wallpapers saved on this computer
//! to switch between.

use dioxus::prelude::*;
use lary_core::config::{BgEffect, HomeBackground, Wallpaper};

use super::Select;
use crate::icons::{Icon, I};
use crate::state::{store, use_app};

/// Draws a picture as ordered dots: each 3 px cell is a 2 px square in the
/// picture's own color where it's dark enough for the 4×4 Bayer threshold,
/// and empty where it's light, so the page shows through. The picture
/// spans the width at its own shape, from the top. Where the dots go comes
/// from the picture as it is, so its detail stays; brightness scales their
/// color (0 is black dots, 200% a pale print). Redrawn when the box changes
/// size.
const DITHER_JS: &str = r#"
if (!window.__lDither) {
  window.__lDither = (id, bright, blur) => {
    const cv = document.getElementById(id); if (!cv) return;
    const img = cv.parentElement && cv.parentElement.querySelector('img.hb-src'); if (!img) return;
    cv.__p = [bright, blur];
    const draw = () => {
      const [b, bl] = cv.__p;
      const W = Math.max(1, Math.round(cv.clientWidth)), H = Math.max(1, Math.round(cv.clientHeight));
      if (!img.naturalWidth) return;
      const cell = 3, gw = Math.ceil(W / cell), gh = Math.ceil(H / cell);
      const sm = cv.__sm || (cv.__sm = document.createElement('canvas'));
      sm.width = gw; sm.height = gh;
      const s = sm.getContext('2d', { willReadFrequently: true });
      s.clearRect(0, 0, gw, gh);
      const lift = b / 100;
      s.filter = 'blur(' + (bl / cell) + 'px)';
      const k = gw / img.naturalWidth;
      s.drawImage(img, 0, 0, gw, img.naturalHeight * k);
      const src = s.getImageData(0, 0, gw, gh).data;
      cv.width = W; cv.height = H;
      const c = cv.getContext('2d');
      const out = c.createImageData(W, H), o = out.data;
      const B = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
      for (let y = 0; y < gh; y++) for (let x = 0; x < gw; x++) {
        const i = (y * gw + x) * 4;
        const L = (0.299 * src[i] + 0.587 * src[i + 1] + 0.114 * src[i + 2]) / 255;
        if (src[i + 3] < 8 || L > (B[(y % 4) * 4 + (x % 4)] + 0.5) / 16) continue;
        const r = Math.min(255, src[i] * lift), g = Math.min(255, src[i + 1] * lift), bl2 = Math.min(255, src[i + 2] * lift);
        for (let dy = 0; dy < cell - 1; dy++) {
          const py = y * cell + dy; if (py >= H) break;
          for (let dx = 0; dx < cell - 1; dx++) {
            const px = x * cell + dx; if (px >= W) break;
            const j = (py * W + px) * 4;
            o[j] = r; o[j + 1] = g; o[j + 2] = bl2; o[j + 3] = 255;
          }
        }
      }
      c.putImageData(out, 0, 0);
    };
    if (img.complete) draw(); else img.addEventListener('load', draw, { once: true });
    if (!cv.__ro) { cv.__ro = new ResizeObserver(() => cv.__draw && cv.__draw()); cv.__ro.observe(cv); }
    cv.__draw = draw;
  };
}
"#;

/// The picture with its effect, for Home and for the preview in Settings.
/// `id` tells the dither canvases apart.
#[component]
pub fn HomeBackdrop(bg: HomeBackground, id: String) -> Element {
    let canvas_id = format!("hb-dither-{id}");
    let dither = bg.effect == BgEffect::Dither;
    let (bright, blur) = (bg.brightness, bg.blur);
    let cid = canvas_id.clone();
    let image = bg.image.clone();
    use_effect(use_reactive!(|dither, bright, blur, image| {
        if dither && !image.is_empty() {
            let js = format!("{DITHER_JS}\nrequestAnimationFrame(() => window.__lDither('{cid}', {bright}, {blur}));");
            document::eval(&js);
        }
    }));
    if bg.image.is_empty() {
        return rsx! {};
    }
    let url = super::projects::image_url(&bg.image);
    // The picture spans the width at its own shape from the top; its lower
    // part fades into the page, from 45% of its height at 100% (a hard
    // edge at 0).
    let solid = 100.0 - bg.fade.min(100) as f64 * 0.55;
    let mask = format!("-webkit-mask-image: linear-gradient(to bottom, #000 {solid:.2}%, transparent 100%); mask-image: linear-gradient(to bottom, #000 {solid:.2}%, transparent 100%);");
    let filter = format!("filter: brightness({}%) blur({}px);", bg.brightness, bg.blur);
    let opacity = bg.image_opacity.min(100) as f64 / 100.0;
    let overlay = bg.overlay.min(100) as f64 / 100.0;
    rsx! {
        div { class: "hb-layer", style: "{mask}",
            div { class: "hb-pic", style: "opacity: {opacity};",
                img { class: if dither { "hb-src hb-hidden" } else { "hb-src" }, src: "{url}", style: if dither { String::new() } else { filter.clone() }, alt: "" }
                if dither {
                    canvas { id: "{canvas_id}", class: "hb-dither" }
                }
            }
            if bg.effect == BgEffect::Grain { div { class: "hb-grain" } }
            if bg.effect == BgEffect::Vignette { div { class: "hb-vignette" } }
            if overlay > 0.0 { div { class: "hb-overlay", style: "opacity: {overlay};" } }
        }
    }
}

/// CSS for the composer over the picture: solid, or see-through and
/// frosted by its blur.
pub fn input_style(bg: &HomeBackground) -> String {
    if bg.image.is_empty() {
        return String::new();
    }
    let blur = bg.composer_blur.min(40);
    format!("--hb-blur: {blur}px; --hb-solid: {}%;", if blur == 0 { 100 } else { 60 })
}

/// Copy a picked picture into Lary's folder; returns its path.
async fn pick_picture() -> Option<String> {
    let f = rfd::AsyncFileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"]).pick_file().await?;
    let ext = f.path().extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
    let dir = store().root().join("wallpapers");
    std::fs::create_dir_all(&dir).ok()?;
    let dest = dir.join(format!("{}.{ext}", chrono::Utc::now().timestamp_millis()));
    std::fs::write(&dest, f.read().await).ok()?;
    Some(dest.to_string_lossy().to_string())
}

/// Delete a picture Lary copied once nothing uses it any more.
fn forget_picture(path: &str, s: &lary_core::Settings) {
    let used = s.home_bg.image == path || s.wallpapers.iter().any(|w| w.look.image == path);
    let ours = std::path::Path::new(path).starts_with(store().root().join("wallpapers"));
    if !used && ours {
        let _ = std::fs::remove_file(path);
    }
}

/// One slider row: the label, the range and its value.
#[component]
fn BgSlider(title: &'static str, value: u32, max: u32, unit: &'static str, onchange: EventHandler<u32>) -> Element {
    rsx! {
        div { class: "set-row hb-slider",
            div { class: "lbl", div { class: "t", "{title}" } }
            div { class: "ap-range",
                input { r#type: "range", min: "0", max: "{max}", step: "1", value: "{value}",
                    oninput: move |e| { if let Ok(v) = e.value().parse::<u32>() { onchange.call(v) } } }
                span { class: "ap-range-val", "{value}{unit}" }
            }
        }
    }
}

/// Settings → Appearance: the home background and saved wallpapers.
#[component]
pub fn HomeBackgroundSettings() -> Element {
    let app = use_app();
    let s = app.config.read().settings.clone();
    let bg = s.home_bg.clone();
    let has = !bg.image.is_empty();
    let set = move |f: Box<dyn FnOnce(&mut HomeBackground)>| app.update_config(move |c| f(&mut c.settings.home_bg));
    let choose = move |_| {
        spawn(async move {
            let Some(path) = pick_picture().await else { return };
            let old = app.config.read().settings.home_bg.image.clone();
            app.update_config(|c| c.settings.home_bg.image = path);
            if !old.is_empty() {
                forget_picture(&old, &app.config.read().settings);
            }
        });
    };
    let remove = move |_| {
        let old = app.config.read().settings.home_bg.image.clone();
        app.update_config(|c| c.settings.home_bg.image.clear());
        forget_picture(&old, &app.config.read().settings);
    };
    let saved = s.wallpapers.iter().any(|w| w.look == bg);
    let thumb_now = if has { super::projects::image_url(&bg.image) } else { String::new() };
    let save = move |_| {
        let look = app.config.read().settings.home_bg.clone();
        if look.image.is_empty() || app.config.read().settings.wallpapers.iter().any(|w| w.look == look) {
            return;
        }
        let id = chrono::Utc::now().timestamp_millis().to_string();
        app.update_config(move |c| c.settings.wallpapers.insert(0, Wallpaper { id, look }));
        app.toast("Wallpaper saved");
    };
    rsx! {
        div { class: "set-group-title", Icon { i: I::Image, size: 13 } "Home Background" }
        div { class: "set-card hb-card",
            div { class: "set-row",
                div { class: "lbl", div { class: "t", "Picture" } div { class: "d", "An image or GIF behind the new chat screen. Chats keep their own background." } }
                div { class: "row hb-pick",
                    if has {
                        img { class: "hb-thumb", src: "{thumb_now}", alt: "" }
                        button { class: "icon-btn", title: "Choose another picture", onclick: choose, Icon { i: I::Refresh, size: 14 } }
                        button { class: "icon-btn", title: "Remove the picture", onclick: remove, Icon { i: I::Trash, size: 14 } }
                    } else {
                        button { class: "ghost-btn", onclick: choose, Icon { i: I::Upload, size: 13 } "Choose picture…" }
                    }
                }
            }
            if has {
                div { class: "set-row",
                    div { class: "lbl", div { class: "t", "Effect" } }
                    Select {
                        value: bg.effect.label().to_string(),
                        options: BgEffect::ALL.iter().map(|e| (e.label().to_string(), e.label().to_string())).collect(),
                        onchange: move |v: String| {
                            if let Some(e) = BgEffect::ALL.into_iter().find(|e| e.label() == v) {
                                set(Box::new(move |b| b.effect = e));
                            }
                        },
                    }
                }
                // The new chat screen in small, as it will look.
                div { class: "hb-preview-wrap",
                    div { class: "hb-preview", style: "{input_style(&bg)}",
                        HomeBackdrop { bg: bg.clone(), id: "preview" }
                        div { class: "hb-mock",
                            div { class: "hb-mock-text", "Plan, build, / for commands, @ for context" }
                            div { class: "hb-mock-row",
                                Icon { i: I::Plus, size: 13 }
                                span { class: "grow" }
                                span { class: "hb-mock-send", Icon { i: I::ArrowUp, size: 12 } }
                            }
                        }
                    }
                }
                BgSlider { title: "Brightness", value: bg.brightness, max: 200, unit: "%", onchange: move |v| set(Box::new(move |b| b.brightness = v)) }
                BgSlider { title: "Blur", value: bg.blur, max: 40, unit: "px", onchange: move |v| set(Box::new(move |b| b.blur = v)) }
                BgSlider { title: "Picture Opacity", value: bg.image_opacity, max: 100, unit: "%", onchange: move |v| set(Box::new(move |b| b.image_opacity = v)) }
                BgSlider { title: "Bottom Fade", value: bg.fade, max: 100, unit: "%", onchange: move |v| set(Box::new(move |b| b.fade = v)) }
                BgSlider { title: "Overlay", value: bg.overlay, max: 100, unit: "%", onchange: move |v| set(Box::new(move |b| b.overlay = v)) }
                BgSlider { title: "Composer Blur", value: bg.composer_blur, max: 40, unit: "px", onchange: move |v| set(Box::new(move |b| b.composer_blur = v)) }
                div { class: "set-row hb-actions",
                    span { class: "grow" }
                    button { class: "ghost-btn", disabled: bg.reset_look() == bg, onclick: move |_| set(Box::new(|b| *b = b.reset_look())), "Reset to defaults" }
                }
            }
        }
        div { class: "set-group-title", Icon { i: I::Layers, size: 13 } "Wallpapers" }
        div { class: "set-card",
            div { class: "set-row",
                div { class: "lbl", div { class: "t", "Saved Wallpapers" } div { class: "d", "Kept on this computer. Click one to use it on the new chat screen." } }
                button { class: "ghost-btn", disabled: !has || saved, title: if saved { "This one is saved" } else { "" }, onclick: save, Icon { i: I::Download, size: 13 } "Save current" }
            }
            if s.wallpapers.is_empty() {
                div { class: "hb-empty", "No wallpapers yet. Choose a picture, set it up, then save it here to switch back to it later." }
            } else {
                div { class: "hb-walls",
                    for w in s.wallpapers.iter().cloned() {
                        {
                            let on = w.look == bg;
                            let (look, id) = (w.look.clone(), w.id.clone());
                            let thumb = super::projects::image_url(&w.look.image);
                            rsx! {
                                div { key: "{w.id}", class: if on { "hb-wall on" } else { "hb-wall" },
                                    button { class: "hb-wall-pick", title: format!("{} · {}", w.look.effect.label(), if on { "In use" } else { "Use this wallpaper" }),
                                        onclick: move |_| { let l = look.clone(); app.update_config(move |c| c.settings.home_bg = l); },
                                        img { src: "{thumb}", alt: "" }
                                        if w.look.effect != BgEffect::None { span { class: "hb-wall-fx", "{w.look.effect.label()}" } }
                                    }
                                    button { class: "hb-wall-x", title: "Delete this wallpaper",
                                        onclick: move |_| {
                                            let gone = app.config.read().settings.wallpapers.iter().find(|x| x.id == id).map(|x| x.look.image.clone());
                                            let id2 = id.clone();
                                            app.update_config(move |c| c.settings.wallpapers.retain(|x| x.id != id2));
                                            if let Some(p) = gone { forget_picture(&p, &app.config.read().settings); }
                                        },
                                        Icon { i: I::X, size: 11 }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
