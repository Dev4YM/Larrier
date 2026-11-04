//! The Files panel's code editor: Monaco (VS Code's editor), served from
//! the program itself (see build.rs) and dressed in Larrier's look: the
//! theme's colors, the code theme chosen in Appearance for its tokens, the
//! Text Size for its type. It edits any text file; Ctrl+S saves.
//!
//! Monaco loads on first use from `larrier://` (on Windows,
//! `http://larrier.localhost/`); its workers start from blobs that load
//! their scripts from there.

use std::borrow::Cow;
use std::collections::HashSet;

use dioxus::desktop::wry::http::{header, Request, Response};
use dioxus::prelude::*;
use serde_json::{json, Value};

use crate::state::use_app;

mod files {
    include!(concat!(env!("OUT_DIR"), "/monaco_files.rs"));
}

/// Open files changed in the editor and not saved yet.
pub static DIRTY: GlobalSignal<HashSet<String>> = Signal::global(HashSet::new);

/// The `larrier` protocol: the editor's files, at `monaco/<path>`.
pub fn serve(req: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = req.uri().path().trim_start_matches('/');
    let file = path.strip_prefix("monaco/").and_then(|p| files::FILES.iter().find(|(n, _)| *n == p));
    let Some((name, bytes)) = file else {
        return Response::builder().status(404).body(Cow::Borrowed(&b""[..])).unwrap_or_default();
    };
    let mime = match name.rsplit('.').next() {
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        _ => "text/plain",
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
        .body(Cow::Borrowed(*bytes))
        .unwrap_or_default()
}

/// The language workers there are, by the label Monaco asks for them with.
fn workers() -> Value {
    let find = |prefix: &str| files::FILES.iter().map(|(n, _)| *n).find(|n| n.starts_with(&format!("vs/assets/{prefix}-")));
    let mut w = serde_json::Map::new();
    for (labels, prefix) in [(&["editorWorkerService"][..], "editor.worker"), (&["json"], "json.worker"), (&["css", "scss", "less"], "css.worker"), (&["html", "handlebars", "razor"], "html.worker")] {
        if let Some(f) = find(prefix) {
            for l in labels {
                w.insert(l.to_string(), json!(f));
            }
        }
    }
    Value::Object(w)
}

/// The editor for one open file; the same editor shows each file in turn
/// (every file keeps its own undo history and place while Larrier runs).
#[component]
pub fn CodeEditor(path: String, text: String) -> Element {
    let app = use_app();
    // Its messages: changes made or saved, saves asked for.
    use_hook(move || {
        let mut ev = document::eval(&format!("{}\nwindow.__larrierEdSend = (m) => dioxus.send(m);\nwindow.__larrierEdWorkers = {};\nawait new Promise(() => {{}});", EDITOR_JS, workers()));
        spawn(async move {
            while let Ok(v) = ev.recv::<Value>().await {
                let path = v["path"].as_str().unwrap_or("").to_string();
                match v["t"].as_str() {
                    Some("dirty") => {
                        let on = v["dirty"].as_bool().unwrap_or(false);
                        if on != DIRTY.peek().contains(&path) {
                            if on {
                                DIRTY.write().insert(path);
                            } else {
                                DIRTY.write().remove(&path);
                            }
                        }
                    }
                    Some("save") => {
                        let text = v["text"].as_str().unwrap_or("");
                        match std::fs::write(&path, text) {
                            Ok(()) => {
                                document::eval(&format!("window.__larrierEdSaved && window.__larrierEdSaved({}, {})", json!(path), v["v"]));
                            }
                            Err(e) => {
                                let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.clone());
                                app.toast_err(format!("Couldn't save {name}: {e}"));
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    });
    // Larrier's look: light or dark, the code theme's colors, wrapping and
    // the minimap as the user leaves them.
    let dark = app.ui.read().dark;
    let (light_theme, dark_theme) = {
        let c = app.config.read();
        (c.settings.code_theme_light.clone(), c.settings.code_theme_dark.clone())
    };
    let tokens = crate::highlight::monaco_rules(if dark { dark_theme.as_deref() } else { light_theme.as_deref() }, dark);
    let size = app.config.read().settings.text_size;
    let wrap = crate::keep::value::<bool>(crate::keep::WINDOW, "editor.wrap").unwrap_or(false);
    let minimap = crate::keep::value::<bool>(crate::keep::WINDOW, "editor.minimap").unwrap_or(false);
    let look = json!({ "dark": dark, "tokens": tokens, "size": format!("{size:?}"), "wrap": wrap, "minimap": minimap, "gen": *LOOK.read() });
    use_effect(use_reactive!(|look| {
        document::eval(&format!("window.__larrierEdLook && window.__larrierEdLook({look})"));
    }));
    let file = json!({ "path": path, "text": text });
    use_effect(use_reactive!(|file| {
        document::eval(&format!("window.__larrierEdOpen && window.__larrierEdOpen({file})"));
    }));
    rsx! {
        // Monaco's own right-click menu, not Larrier's.
        div { id: "fv-editor", class: "fv-editor", "data-ctx": "editor" }
    }
}

/// Bumped when the editor's own options change (wrap, minimap).
static LOOK: GlobalSignal<u64> = Signal::global(|| 0);

/// Wrap long lines in the editor, or not (kept).
pub fn set_wrap(on: bool) {
    crate::keep::set_value(crate::keep::WINDOW, "editor.wrap", &on);
    *LOOK.write() += 1;
}

/// Show the minimap beside the code, or not (kept).
pub fn set_minimap(on: bool) {
    crate::keep::set_value(crate::keep::WINDOW, "editor.minimap", &on);
    *LOOK.write() += 1;
}

pub fn wrap() -> bool {
    let _ = LOOK.read();
    crate::keep::value::<bool>(crate::keep::WINDOW, "editor.wrap").unwrap_or(false)
}

pub fn minimap() -> bool {
    let _ = LOOK.read();
    crate::keep::value::<bool>(crate::keep::WINDOW, "editor.minimap").unwrap_or(false)
}

/// Save the file shown (what Ctrl+S does).
pub fn save() {
    document::eval("window.__larrierEdSave && window.__larrierEdSave()");
}

/// Monaco's loader, Larrier's theme for it, and the editor's file handling.
const EDITOR_JS: &str = r#"
if (!window.__larrierEdReady) {
  window.__larrierEdReady = true;
  const base = (location.protocol === 'http:' || location.protocol === 'https:') ? location.protocol + '//larrier.localhost/monaco/' : 'larrier://localhost/monaco/';
  const st = { models: {}, views: {}, disk: {}, look: null };
  let loading = null;
  // Monaco, once: its styles, its loader, then the editor itself.
  const load = () => loading || (loading = new Promise((ok, fail) => {
    const css = document.createElement('link');
    css.rel = 'stylesheet';
    css.href = base + 'vs/editor/editor.main.css';
    css.setAttribute('data-name', 'vs/editor/editor.main');
    document.head.appendChild(css);
    const s = document.createElement('script');
    s.src = base + 'vs/loader.js';
    s.onerror = () => fail(new Error('loader'));
    s.onload = () => {
      // Its define() isn't announced: other scripts on the page (the
      // terminal's) stay plain scripts.
      if (window.define) window.define.amd = undefined;
      window.require.config({ paths: { vs: base + 'vs' } });
      window.MonacoEnvironment = {
        getWorker: (_, label) => {
          const f = (window.__larrierEdWorkers || {})[label];
          // No worker for it here (TypeScript's): one that never answers,
          // so those features stay quietly off.
          const src = f ? 'importScripts(' + JSON.stringify(base + f) + ');' : '';
          return new Worker(URL.createObjectURL(new Blob([src], { type: 'text/javascript' })));
        },
      };
      window.require(['vs/editor/editor.main'], () => ok(window.monaco), fail);
    };
    document.head.appendChild(s);
  }));
  // A CSS color (variables, color-mix) as Monaco's #rrggbb(aa).
  const hex = (css) => {
    let p = document.getElementById('ed-probe');
    if (!p) {
      p = document.createElement('span');
      p.id = 'ed-probe';
      p.style.display = 'none';
      (document.querySelector('.app') || document.body).appendChild(p);
    }
    p.style.color = '';
    p.style.color = css;
    const c = getComputedStyle(p).color;
    let r = 0, g = 0, b = 0, a = 1, m;
    if ((m = c.match(/rgba?\(([^)]+)\)/))) {
      const v = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
      [r, g, b] = v;
      if (v.length > 3) a = v[3];
    } else if ((m = c.match(/color\(srgb ([^)]+)\)/))) {
      const v = m[1].split(/[\s/]+/).filter(Boolean).map(Number);
      r = v[0] * 255; g = v[1] * 255; b = v[2] * 255;
      if (v.length > 3) a = v[3];
    } else return '#00000000';
    const h = (x) => Math.round(Math.max(0, Math.min(255, x))).toString(16).padStart(2, '0');
    return '#' + h(r) + h(g) + h(b) + (a < 1 ? h(a * 255) : '');
  };
  const v = (name) => hex('var(' + name + ')');
  const mix = (name, pct) => hex('color-mix(in srgb, var(' + name + ') ' + pct + '%, transparent)');
  const theme = (m, look) => {
    m.editor.defineTheme('larrier', {
      base: look.dark ? 'vs-dark' : 'vs',
      inherit: true,
      rules: look.tokens.rules,
      colors: {
        'editor.background': v('--pane'),
        'editor.foreground': v('--fg'),
        'editorLineNumber.foreground': v('--fg-4'),
        'editorLineNumber.activeForeground': v('--fg-2'),
        'editor.lineHighlightBackground': mix('--fg', 4),
        'editor.lineHighlightBorder': '#00000000',
        'editor.selectionBackground': hex('color-mix(in srgb, var(--accent-c, var(--blue)) 26%, transparent)'),
        'editor.inactiveSelectionBackground': mix('--fg', 9),
        'editor.selectionHighlightBackground': mix('--fg', 7),
        'editor.wordHighlightBackground': mix('--fg', 7),
        'editor.findMatchBackground': hex('color-mix(in srgb, var(--accent-c, var(--blue)) 34%, transparent)'),
        'editor.findMatchHighlightBackground': mix('--fg', 12),
        'editorCursor.foreground': v('--fg'),
        'editorIndentGuide.background1': mix('--fg', 9),
        'editorIndentGuide.activeBackground1': mix('--fg', 22),
        'editorWhitespace.foreground': mix('--fg', 22),
        'editorBracketMatch.background': mix('--fg', 8),
        'editorBracketMatch.border': mix('--fg', 25),
        'editorGutter.background': v('--pane'),
        'editorWidget.background': v('--card-2'),
        'editorWidget.foreground': v('--fg'),
        'editorWidget.border': v('--border'),
        'editorSuggestWidget.background': v('--card-2'),
        'editorSuggestWidget.border': v('--border'),
        'editorSuggestWidget.selectedBackground': v('--hover'),
        'editorSuggestWidget.highlightForeground': hex('var(--accent-c, var(--blue))'),
        'editorHoverWidget.background': v('--card-2'),
        'editorHoverWidget.border': v('--border'),
        'editorStickyScroll.background': v('--pane'),
        'editorStickyScrollHover.background': v('--hover'),
        'editorStickyScroll.shadow': '#00000000',
        'input.background': v('--card'),
        'input.border': v('--border'),
        'inputOption.activeBorder': hex('var(--accent-c, var(--blue))'),
        'focusBorder': '#00000000',
        'widget.shadow': mix('--fg', 10),
        'scrollbar.shadow': '#00000000',
        'scrollbarSlider.background': mix('--fg', 13),
        'scrollbarSlider.hoverBackground': mix('--fg', 22),
        'scrollbarSlider.activeBackground': mix('--fg', 28),
        'editorOverviewRuler.border': '#00000000',
        'minimap.background': v('--pane'),
        'menu.background': v('--card-2'),
        'menu.foreground': v('--fg'),
        'menu.selectionBackground': v('--hover'),
        'menu.selectionForeground': v('--fg'),
        'menu.separatorBackground': v('--border'),
        'menu.border': v('--border'),
      },
    });
    m.editor.setTheme('larrier');
  };
  // Type as the app's code: its font and the Text Size's code size.
  const type = () => {
    const cs = getComputedStyle(document.querySelector('.app') || document.documentElement);
    const size = parseFloat(cs.getPropertyValue('--fs-code')) || 12.5;
    const line = parseFloat(cs.getPropertyValue('--lh-code')) || Math.round(size * 1.6);
    return { fontFamily: cs.getPropertyValue('--mono').trim() || 'monospace', fontSize: size + 1, lineHeight: line + 1 };
  };
  const still = () => !!document.querySelector('.no-motion');
  const editor = async () => {
    const host = document.getElementById('fv-editor');
    if (!host) return null;
    const m = await load();
    if (st.look) theme(m, st.look);
    // The panel was closed and opened again: the old editor goes (the
    // files' models stay, with their undo history).
    if (st.ed && st.ed !== host.__ed && !st.ed.getContainerDomNode().isConnected) {
      st.views[st.ed.getModel()?.__path || ''] = st.ed.saveViewState();
      st.ed.dispose();
      st.ed = null;
    }
    if (!host.__ed) {
      const ed = m.editor.create(host, {
        model: null,
        theme: 'larrier',
        automaticLayout: true,
        fixedOverflowWidgets: true,
        scrollBeyondLastLine: false,
        roundedSelection: true,
        renderLineHighlight: 'all',
        cursorBlinking: 'smooth',
        padding: { top: 10, bottom: 10 },
        scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10, useShadows: false },
        overviewRulerBorder: false,
        hideCursorInOverviewRuler: true,
        lineNumbersMinChars: 3,
        lineDecorationsWidth: 10,
        glyphMargin: false,
        stickyScroll: { enabled: true },
        guides: { indentation: true, bracketPairs: false },
        renderWhitespace: 'selection',
        unicodeHighlight: { ambiguousCharacters: false },
        ...type(),
      });
      host.__ed = ed;
      st.ed = ed;
      ed.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => window.__larrierEdSave());
      ed.onDidChangeModelContent(() => changed(ed.getModel()));
    }
    const look = st.look || {};
    host.__ed.updateOptions({
      ...type(),
      smoothScrolling: !still(),
      cursorSmoothCaretAnimation: still() ? 'off' : 'on',
      wordWrap: look.wrap ? 'on' : 'off',
      minimap: { enabled: !!look.minimap, renderCharacters: false, showSlider: 'mouseover' },
    });
    return host.__ed;
  };
  // Unsaved changes, reported as they come and go.
  const changed = (model) => {
    if (!model || !model.__path) return;
    const dirty = model.getAlternativeVersionId() !== model.__saved;
    if (dirty !== !!model.__dirty) {
      model.__dirty = dirty;
      window.__larrierEdSend({ t: 'dirty', path: model.__path, dirty });
    }
  };
  const language = (path) => {
    const name = path.split(/[\\/]/).pop().toLowerCase();
    if (name.startsWith('.env') || name.endsWith('.env')) return 'ini';
    if (name === 'cargo.lock' || name.endsWith('.toml')) return 'ini';
    return undefined;
  };
  window.__larrierEdLook = async (look) => {
    st.look = look;
    try { await editor(); } catch (e) {}
  };
  window.__larrierEdOpen = async (file) => {
    let ed;
    try { ed = await editor(); } catch (e) {
      const host = document.getElementById('fv-editor');
      if (host) host.textContent = 'The editor couldn\'t load.';
      return;
    }
    if (!ed) return;
    const m = window.monaco;
    let model = st.models[file.path];
    if (!model || model.isDisposed()) {
      const uri = m.Uri.file(file.path);
      model = m.editor.getModel(uri) || m.editor.createModel(file.text, language(file.path), uri);
      model.__path = file.path;
      model.__saved = model.getAlternativeVersionId();
      st.models[file.path] = model;
      st.disk[file.path] = file.text;
    } else if (file.text !== st.disk[file.path]) {
      // Changed on disk (an agent edited it): taken in, as an undoable
      // edit, unless there are unsaved changes here.
      st.disk[file.path] = file.text;
      if (!model.__dirty) {
        model.pushEditOperations([], [{ range: model.getFullModelRange(), text: file.text }], () => null);
        model.__saved = model.getAlternativeVersionId();
        changed(model);
      }
    }
    const cur = ed.getModel();
    if (cur !== model) {
      if (cur && cur.__path) st.views[cur.__path] = ed.saveViewState();
      ed.setModel(model);
      const view = st.views[file.path];
      if (view) ed.restoreViewState(view);
    }
  };
  window.__larrierEdSave = () => {
    const host = document.getElementById('fv-editor');
    const model = host && host.__ed && host.__ed.getModel();
    if (!model || !model.__path) return;
    window.__larrierEdSend({ t: 'save', path: model.__path, text: model.getValue(), v: model.getAlternativeVersionId() });
  };
  window.__larrierEdSaved = (path, v) => {
    const model = st.models[path];
    if (!model) return;
    model.__saved = v;
    st.disk[path] = model.getValue();
    changed(model);
  };
}
"#;
