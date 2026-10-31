// Annotate mode for Larrier's browser (injected into every page, dormant
// until Larrier turns it on). The user points at elements, a highlight
// glides to each, a click picks one (Shift+click adds more), they write a
// note, and Enter sends the elements and the note to the agent's box.
(() => {
  if (window.top !== window || window.__larrierAnnotate) return;
  const TOKEN = '__LARRIER_TOKEN__';
  const ACCENT = '#3b82f6';
  let on = false, host = null, root = null, hover = null, hoverLabel = null, bubble = null, input = null, frame = null;
  let picked = [];
  let last = null;

  // Larrier's Chrome hears pages through a DevTools binding (it may come
  // after this script), its WebView2 through the view's own channel.
  const send = (msg) => {
    const body = JSON.stringify(Object.assign({ larrier: TOKEN }, msg));
    try { if (typeof window.__larrierPost === 'function') window.__larrierPost(body); else window.ipc.postMessage(body); } catch (e) {}
  };
  const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

  const css = `
    :host { all: initial; }
    .frame { position: fixed; inset: 0; pointer-events: none; z-index: 2147483646; }
    .frame svg { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; }
    .frame rect { fill: none; stroke: ${ACCENT}; stroke-width: 3; stroke-dasharray: 100; stroke-dashoffset: 100; transition: stroke-dashoffset .7s cubic-bezier(.65, 0, .35, 1); }
    .frame.on rect { stroke-dashoffset: 0; }
    .frame .glow { position: absolute; inset: 0; box-shadow: inset 0 0 0 1px rgba(59,130,246,.35), inset 0 0 28px rgba(59,130,246,.18); opacity: 0; transition: opacity .5s ease .2s; }
    .frame.on .glow { opacity: 1; }
    .box { position: fixed; left: 0; top: 0; pointer-events: none; z-index: 2147483646; border-radius: 4px; box-sizing: border-box;
      transition: transform .16s cubic-bezier(.2, .8, .2, 1), width .16s cubic-bezier(.2, .8, .2, 1), height .16s cubic-bezier(.2, .8, .2, 1), opacity .12s; }
    .hover { border: 1.5px solid ${ACCENT}; background: rgba(59,130,246,.10); opacity: 0; }
    .hover.on { opacity: 1; }
    .picked { border: 2px solid ${ACCENT}; background: rgba(59,130,246,.06); box-shadow: 0 0 0 3px rgba(59,130,246,.18); }
    .badge { position: absolute; top: -10px; left: -10px; min-width: 18px; height: 18px; padding: 0 4px; border-radius: 9px; background: ${ACCENT}; color: #fff;
      font: 600 11px/18px system-ui, -apple-system, 'Segoe UI', sans-serif; text-align: center; box-sizing: border-box; }
    .label { position: fixed; left: 0; top: 0; pointer-events: none; z-index: 2147483647; padding: 2px 7px; border-radius: 5px; background: ${ACCENT}; color: #fff;
      font: 500 11px/16px system-ui, -apple-system, 'Segoe UI', sans-serif; white-space: nowrap; max-width: 320px; overflow: hidden; text-overflow: ellipsis;
      transition: transform .16s cubic-bezier(.2, .8, .2, 1), opacity .12s; opacity: 0; }
    .label.on { opacity: 1; }
    .bubble { position: fixed; z-index: 2147483647; width: 320px; padding: 10px; border-radius: 12px; background: #fff; color: #1f2328;
      box-shadow: 0 10px 30px rgba(0,0,0,.18), 0 0 0 1px rgba(0,0,0,.08); font: 13px/18px system-ui, -apple-system, 'Segoe UI', sans-serif;
      opacity: 0; transform: translateY(4px) scale(.98); transition: opacity .14s ease, transform .16s cubic-bezier(.2, .8, .2, 1); pointer-events: auto; }
    .bubble.on { opacity: 1; transform: none; }
    .bubble textarea { width: 100%; min-height: 54px; max-height: 160px; resize: none; border: 1px solid rgba(0,0,0,.14); border-radius: 8px; padding: 7px 9px;
      box-sizing: border-box; font: inherit; color: inherit; background: #fff; outline: none; }
    .bubble textarea:focus { border-color: ${ACCENT}; box-shadow: 0 0 0 3px rgba(59,130,246,.15); }
    .bubble .hint { margin-top: 6px; color: #6b7280; font-size: 11.5px; display: flex; justify-content: space-between; gap: 8px; }
    @media (prefers-color-scheme: dark) {
      .bubble { background: #1f1f1f; color: #e8e8e8; box-shadow: 0 10px 30px rgba(0,0,0,.5), 0 0 0 1px rgba(255,255,255,.1); }
      .bubble textarea { background: #161616; border-color: rgba(255,255,255,.14); }
      .bubble .hint { color: #9ca3af; }
    }
  `;

  const el = (tag, cls, parent) => { const e = document.createElement(tag); if (cls) e.className = cls; (parent || root).appendChild(e); return e; };
  const put = (box, r) => { box.style.width = r.width + 'px'; box.style.height = r.height + 'px'; box.style.transform = `translate(${r.left}px, ${r.top}px)`; };
  const ours = (t) => t === host || (host && host.contains(t));

  // What an element is, for the label and for the agent.
  const describe = (e) => {
    const t = e.tagName.toLowerCase();
    const name = (e.getAttribute('aria-label') || e.getAttribute('alt') || e.getAttribute('title') || e.getAttribute('placeholder') || (e.innerText || '').trim() || e.value || '').replace(/\s+/g, ' ').trim().slice(0, 60);
    const id = e.id ? '#' + e.id : '';
    const cls = typeof e.className === 'string' && e.className.trim() ? '.' + e.className.trim().split(/\s+/).slice(0, 2).join('.') : '';
    return { short: t + (id || cls) + (name ? ' · ' + name : ''), name };
  };
  const selector = (e) => {
    const parts = [];
    for (let n = e; n && n.nodeType === 1 && n !== document.documentElement && parts.length < 6; n = n.parentElement) {
      if (n.id && document.querySelectorAll('#' + CSS.escape(n.id)).length === 1) { parts.unshift('#' + CSS.escape(n.id)); break; }
      let s = n.tagName.toLowerCase();
      const sib = n.parentElement ? [...n.parentElement.children].filter(c => c.tagName === n.tagName) : [];
      if (sib.length > 1) s += ':nth-of-type(' + (sib.indexOf(n) + 1) + ')';
      parts.unshift(s);
    }
    return parts.join(' > ');
  };

  const target = (x, y) => {
    for (const e of document.elementsFromPoint(x, y)) {
      if (!ours(e) && e !== document.documentElement && e !== document.body) return e;
    }
    return null;
  };

  const showHover = (e) => {
    if (!e) { hover.classList.remove('on'); hoverLabel.classList.remove('on'); last = null; return; }
    if (e === last) return;
    last = e;
    const r = e.getBoundingClientRect();
    put(hover, r);
    hover.classList.add('on');
    hoverLabel.textContent = describe(e).short;
    const ly = r.top > 24 ? r.top - 22 : r.bottom + 4;
    hoverLabel.style.transform = `translate(${Math.max(4, Math.min(r.left, innerWidth - 330))}px, ${ly}px)`;
    hoverLabel.classList.add('on');
  };

  const layoutPicked = () => {
    for (const p of picked) put(p.box, p.el.getBoundingClientRect());
    placeBubble();
  };

  const placeBubble = () => {
    if (!bubble || !picked.length) return;
    const r = picked[picked.length - 1].el.getBoundingClientRect();
    const h = bubble.offsetHeight || 110;
    const below = r.bottom + 10 + h < innerHeight;
    const top = below ? r.bottom + 10 : Math.max(8, r.top - h - 10);
    const left = Math.max(8, Math.min(r.left, innerWidth - 336));
    bubble.style.left = left + 'px';
    bubble.style.top = top + 'px';
  };

  const openBubble = () => {
    if (!bubble) {
      bubble = el('div', 'bubble');
      input = el('textarea', '', bubble);
      input.placeholder = 'Tell the agent about this…';
      const hint = el('div', 'hint', bubble);
      el('span', '', hint).textContent = 'Enter attaches it to the chat';
      el('span', '', hint).textContent = 'Shift+click adds more';
      input.addEventListener('keydown', (e) => {
        e.stopPropagation();
        if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); attach(); }
        if (e.key === 'Escape') { e.preventDefault(); clearPicked(); }
      }, true);
      for (const k of ['keyup', 'keypress', 'input']) input.addEventListener(k, (e) => e.stopPropagation(), true);
    }
    placeBubble();
    requestAnimationFrame(() => { bubble.classList.add('on'); input.focus(); });
  };

  const clearPicked = () => {
    for (const p of picked) p.box.remove();
    picked = [];
    if (bubble) { bubble.classList.remove('on'); input.value = ''; }
  };

  const pick = (e, add) => {
    const i = picked.findIndex(p => p.el === e);
    if (i >= 0 && add) { picked[i].box.remove(); picked.splice(i, 1); }
    else {
      if (!add) clearPicked();
      const box = el('div', 'box picked');
      put(box, e.getBoundingClientRect());
      picked.push({ el: e, box });
    }
    picked.forEach((p, n) => { let b = p.box.querySelector('.badge'); if (!b) b = el('div', 'badge', p.box); b.textContent = String(n + 1); b.style.display = picked.length > 1 ? '' : 'none'; });
    if (picked.length) openBubble(); else if (bubble) bubble.classList.remove('on');
  };

  const attach = () => {
    if (!picked.length) return;
    const items = picked.map(p => {
      const r = p.el.getBoundingClientRect();
      const d = describe(p.el);
      return { tag: p.el.tagName.toLowerCase(), name: d.name, short: d.short, selector: selector(p.el), text: (p.el.innerText || p.el.value || '').trim().slice(0, 600),
        html: p.el.outerHTML.slice(0, 1500), rect: { x: r.left, y: r.top, w: r.width, h: r.height } };
    });
    const note = input.value.trim();
    // Out of the pictures Larrier takes of the elements.
    host.style.visibility = 'hidden';
    document.documentElement.style.cursor = '';
    requestAnimationFrame(() => requestAnimationFrame(() => {
      send({ kind: 'attach', note, url: location.href, title: document.title, items });
      clearPicked();
      setOn(false);
    }));
  };

  const onMove = (e) => { if (on && !ours(e.target) && !(bubble && bubble.contains(e.target))) showHover(target(e.clientX, e.clientY)); };
  const block = (e) => {
    if (!on || ours(e.target)) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.type === 'click') { const t = target(e.clientX, e.clientY); if (t) pick(t, e.shiftKey); }
  };
  const onKey = (e) => { if (on && e.key === 'Escape' && !ours(e.target)) { e.preventDefault(); if (picked.length) clearPicked(); else setOn(false); } };
  const onScroll = () => { if (on) { last = null; layoutPicked(); } };

  const setOn = (v) => {
    if (v === on) return;
    on = v;
    if (on) {
      host = document.createElement('larrier-annotate');
      root = host.attachShadow({ mode: 'closed' });
      el('style').textContent = css;
      frame = el('div', 'frame');
      frame.innerHTML = '<svg><rect x="1.5" y="1.5" width="calc(100% - 3px)" height="calc(100% - 3px)" rx="6" pathLength="100"></rect></svg><div class="glow"></div>';
      const rect = frame.querySelector('rect');
      rect.setAttribute('width', String(innerWidth - 3));
      rect.setAttribute('height', String(innerHeight - 3));
      hover = el('div', 'box hover');
      hoverLabel = el('div', 'label');
      document.documentElement.appendChild(host);
      if (reduced()) frame.classList.add('on'); else requestAnimationFrame(() => requestAnimationFrame(() => frame.classList.add('on')));
      addEventListener('mousemove', onMove, true);
      for (const t of ['click', 'mousedown', 'mouseup', 'pointerdown', 'pointerup', 'dblclick', 'contextmenu', 'auxclick']) addEventListener(t, block, true);
      addEventListener('keydown', onKey, true);
      addEventListener('scroll', onScroll, true);
      addEventListener('resize', onScroll);
      document.documentElement.style.cursor = 'crosshair';
    } else {
      removeEventListener('mousemove', onMove, true);
      for (const t of ['click', 'mousedown', 'mouseup', 'pointerdown', 'pointerup', 'dblclick', 'contextmenu', 'auxclick']) removeEventListener(t, block, true);
      removeEventListener('keydown', onKey, true);
      removeEventListener('scroll', onScroll, true);
      removeEventListener('resize', onScroll);
      document.documentElement.style.cursor = '';
      const h = host;
      if (frame) frame.classList.remove('on');
      picked = [];
      bubble = null; input = null; last = null;
      setTimeout(() => h && h.remove(), reduced() ? 0 : 450);
      host = null;
      send({ kind: 'off' });
    }
  };

  Object.defineProperty(window, '__larrierAnnotate', { value: (v) => setOn(!!v) });
})();
