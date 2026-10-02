// What the page looks like, for AI-assisted mode: every visible run of text
// grouped by the box it sits in, with where that box is and its colours, plus
// pictures and search boxes, in reading order.
(() => {
  const BOX = /^(block|flex|grid|list-item|table|flow-root|inline-block|inline-flex|inline-grid)/;
  const hex = c => {
    const m = (c || '').match(/[\d.]+/g);
    if (!m || (m.length > 3 && +m[3] < 0.5)) return null;
    return '#' + m.slice(0, 3).map(v => Math.round(+v).toString(16).padStart(2, '0')).join('');
  };
  const bgOf = el => {
    for (let e = el; e && e.nodeType === 1; e = e.parentElement) {
      const b = hex(getComputedStyle(e).backgroundColor);
      if (b) return b;
    }
    return '#ffffff';
  };
  const boxOf = el => {
    for (let e = el; e && e !== document.body; e = e.parentElement) {
      if (BOX.test(getComputedStyle(e).display) || /^(TD|TH|LI|BUTTON)$/.test(e.tagName)) return e;
    }
    return document.body;
  };
  const items = [], byEl = new Map();
  let chars = 0;
  const add = (el, extra) => {
    const r = el.getBoundingClientRect(), s = getComputedStyle(el);
    const it = Object.assign({
      t: el.tagName.toLowerCase(), x: Math.round(r.left + scrollX), y: Math.round(r.top + scrollY), w: Math.round(r.width), h: Math.round(r.height),
      fg: hex(s.color) || '#000000', bg: bgOf(el), fs: Math.round(parseFloat(s.fontSize) || 16), b: parseInt(s.fontWeight) >= 600, s: [],
    }, extra);
    items.push(it);
    return it;
  };
  const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
    acceptNode(n) {
      if (n.nodeType === 3) return NodeFilter.FILTER_ACCEPT;
      if (/^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE|IFRAME|SELECT|OPTION)$/.test(n.tagName)) return NodeFilter.FILTER_REJECT;
      if (n.tagName.toLowerCase() === 'svg' && n.getAttribute('role') !== 'img') return NodeFilter.FILTER_REJECT;
      if (n.checkVisibility && !n.checkVisibility({ visibilityProperty: true, opacityProperty: true })) return NodeFilter.FILTER_REJECT;
      // text only screen readers get, squeezed into a pixel
      const r = n.getBoundingClientRect();
      if ((r.width <= 2 || r.height <= 2) && (r.width || r.height)) {
        const st = getComputedStyle(n);
        if (st.overflow === 'hidden' || st.clip !== 'auto' || st.clipPath !== 'none') return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  for (let n = walk.nextNode(); n && items.length < 4000 && chars < 400000; n = walk.nextNode()) {
    if (n.nodeType === 1) {
      const t = n.tagName;
      const r = n.getBoundingClientRect();
      if ((t === 'IMG' || t.toLowerCase() === 'svg') && r.width >= 40 && r.height >= 24) {
        const src = n.getAttribute('data-src') || n.getAttribute('src') || (n.getAttribute('srcset') || '').split(',').pop().trim().split(' ')[0];
        let abs = '';
        try { abs = src ? new URL(src, document.baseURI).href : ''; } catch (e) {}
        add(n, { t: 'img', src: abs, alt: n.getAttribute('alt') || n.getAttribute('aria-label') || '' });
      } else if ((t === 'INPUT' && /^(text|search|)$/.test(n.type) || t === 'TEXTAREA') && r.width > 20) {
        add(n, { t: 'input', alt: n.placeholder || n.getAttribute('aria-label') || '' });
      }
      continue;
    }
    const text = n.data.replace(/\s+/g, ' ');
    const p = n.parentElement;
    if (!p || p.getClientRects().length === 0) continue;
    const box = boxOf(p);
    if (!text.trim()) {
      // the space between two words in different tags
      const it = byEl.get(box), last = it && it.s[it.s.length - 1];
      if (last && !last[0].endsWith(' ')) last[0] += ' ';
      continue;
    }
    let it = byEl.get(box);
    if (!it) {
      it = add(box, {});
      byEl.set(box, it);
    }
    const a = p.closest('a[href]');
    const href = a && !/^(javascript|mailto|tel):/.test(a.getAttribute('href')) ? a.href : null;
    const bold = parseInt(getComputedStyle(p).fontWeight) >= 600;
    const last = it.s[it.s.length - 1];
    if (last && last[1] === href && last[2] === bold) last[0] += text;
    else it.s.push([text, href, bold]);
    chars += text.length;
  }
  for (const it of items) {
    if (it.s.length) {
      it.s[0][0] = it.s[0][0].trimStart();
      it.s[it.s.length - 1][0] = it.s[it.s.length - 1][0].trimEnd();
    }
  }
  const body = document.body;
  return JSON.stringify({
    bg: bgOf(body) === '#ffffff' ? (hex(getComputedStyle(document.documentElement).backgroundColor) || '#ffffff') : bgOf(body),
    fg: hex(getComputedStyle(body).color) || '#000000',
    h: document.documentElement.scrollHeight,
    items: items.filter(i => i.h > 0 && (i.t === 'img' || i.t === 'input' || i.s.some(s => s[0].trim()))),
  });
})()
