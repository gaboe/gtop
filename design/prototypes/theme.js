// Rack palettes: presets, a random mixer, and saved mixes. Six base colors derive every other token.
(function () {
  const PRESETS = [
    { id: "amber", name: "Amber", base: { ground: "#0b0c0d", plate: "#141618", silk: "#e8e6df", led: "#ffb000", hot: "#ff3b1f", ok: "#38ff8b" } },
    { id: "ice", name: "Ice", base: { ground: "#07090c", plate: "#0f141a", silk: "#dfe8f0", led: "#43d6ff", hot: "#ff3b6b", ok: "#b4ff39" } },
    { id: "phosphor", name: "Phosphor", base: { ground: "#050805", plate: "#0c120d", silk: "#d6f5d9", led: "#39ff6a", hot: "#ff4a2e", ok: "#d8ff5a" } },
    { id: "neon", name: "Neon", base: { ground: "#0a0710", plate: "#130e1c", silk: "#f1e6ff", led: "#ff2bd6", hot: "#ffe600", ok: "#00f0ff" } },
    { id: "alu", name: "Aluminium", base: { ground: "#9fa3a8", plate: "#c8cbcf", silk: "#121315", led: "#ff5a00", hot: "#c40000", ok: "#00874a" } },
  ];
  const BASE = [["ground", "Ground"], ["plate", "Panel"], ["silk", "Labels"], ["led", "LED"], ["hot", "Hot"], ["ok", "OK and ports"]];
  const store = {
    get(k, d) { try { const v = localStorage.getItem(k); return v == null ? d : JSON.parse(v); } catch (e) { return d; } },
    set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch (e) {} },
  };
  const root = document.documentElement;
  let customs = store.get("gtop-customs", []);
  const T = { active: "amber", draft: null, saveName: "" };

  function lum(hex) { const n = parseInt(hex.slice(1), 16); return (0.2126 * (n >> 16) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255)) / 255; }
  function tokens(b) {
    const light = lum(b.ground) > 0.4;
    const mix = (a, c, p) => `color-mix(in oklab, ${a} ${p}%, ${c})`;
    return {
      "--ground": b.ground, "--plate": b.plate, "--silk": b.silk, "--led": b.led, "--hot": b.hot, "--ok": b.ok,
      "--plate-2": mix(b.plate, b.silk, light ? 88 : 93), "--rule": mix(b.plate, b.silk, light ? 78 : 86), "--rule-hi": mix(b.plate, b.silk, light ? 62 : 74),
      "--silk-dim": mix(b.silk, b.plate, 58), "--ink-row": mix(b.silk, b.plate, 78), "--seg-off": mix(b.plate, b.silk, light ? 82 : 90),
      "--hot-off": mix(b.hot, b.plate, 20), "--led-off": mix(b.led, b.plate, 16), "--well": mix(b.ground, light ? "#fff" : "#000", light ? 80 : 60),
      "--row-rule": mix(b.ground, b.silk, light ? 88 : 94), "--cmd": mix(b.silk, b.ground, light ? 60 : 48), "--on-led": light ? "#fff" : "#000",
      "--glow": light ? "0" : "1",
    };
  }
  function apply(b) { Object.entries(tokens(b)).forEach(([k, v]) => root.style.setProperty(k, v)); root.style.colorScheme = lum(b.ground) > 0.4 ? "light" : "dark"; }
  const all = () => [...PRESETS, ...customs];
  const find = (id) => all().find((p) => p.id === id);
  function use(id) { const p = find(id) || PRESETS[0]; T.active = p.id; T.draft = null; apply(p.base); store.set("gtop-pal", p.id); }
  function cycle() { const l = all(), i = l.findIndex((p) => p.id === T.active); use(l[(i + 1) % l.length].id); }

  function oklch(l, c, h) {
    const a = c * Math.cos(h * Math.PI / 180), bb = c * Math.sin(h * Math.PI / 180);
    const L = (l + 0.3963377774 * a + 0.2158037573 * bb) ** 3, M = (l - 0.1055613458 * a - 0.0638541728 * bb) ** 3, S = (l - 0.0894841775 * a - 1.291485548 * bb) ** 3;
    const lin = [4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S, -1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S, -0.0041960863 * L - 0.7034186147 * M + 1.707614701 * S];
    return "#" + lin.map((x) => { x = Math.min(1, Math.max(0, x)); x = x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055; return Math.round(x * 255).toString(16).padStart(2, "0"); }).join("");
  }
  // LED and hot sit far apart on the hue wheel so load and alarm never read as one color.
  function random() {
    const h = Math.random() * 360, light = Math.random() < 0.2, tint = 0.008 + Math.random() * 0.03;
    const hot = (h + 130 + Math.random() * 100) % 360, ok = (h + 60 + Math.random() * 60) % 360;
    T.draft = light
      ? { ground: oklch(0.7, tint, h), plate: oklch(0.85, tint, h), silk: oklch(0.17, tint, h), led: oklch(0.64, 0.19, h), hot: oklch(0.5, 0.2, hot), ok: oklch(0.52, 0.14, ok) }
      : { ground: oklch(0.12, tint, h), plate: oklch(0.17, tint + 0.004, h), silk: oklch(0.93, 0.02, h), led: oklch(0.8, 0.17, h), hot: oklch(0.67, 0.22, hot), ok: oklch(0.86, 0.17, ok) };
    apply(T.draft);
  }
  function edit(k, v) { T.draft = { ...(T.draft || find(T.active).base), [k]: v }; apply(T.draft); }
  function save() {
    if (!T.draft) return;
    const id = "mix-" + Date.now().toString(36);
    customs.push({ id, name: T.saveName.trim() || "Mix " + (customs.length + 1), base: T.draft });
    store.set("gtop-customs", customs); T.saveName = ""; use(id);
  }
  function remove(id) { customs = customs.filter((c) => c.id !== id); store.set("gtop-customs", customs); if (T.active === id) use("amber"); }

  function panel() {
    const cur = T.draft || find(T.active).base;
    const list = all().map((p) => `<div class="pal-row ${!T.draft && T.active === p.id ? "on" : ""}">
      <button class="pal-pick" data-act="pal:${p.id}"><span class="chips">${["plate", "led", "hot", "ok"].map((k) => `<i style="background:${p.base[k]}"></i>`).join("")}</span>${p.name}</button>
      ${p.id.startsWith("mix-") ? `<button class="pal-del" data-act="paldel:${p.id}" aria-label="Delete ${p.name}">Del</button>` : ""}</div>`).join("");
    const toks = BASE.map(([k, l]) => `<label class="tok"><input type="color" data-tok="${k}" value="${cur[k]}"><span>${l}</span><code>${cur[k]}</code></label>`).join("");
    return `<aside class="side settings" aria-label="Settings" data-scroll="settings">
      <header class="side-head"><div class="silk">Settings · S</div><div class="side-name">Palette</div></header>
      <section class="side-sec"><h3 class="silk"><span>Presets and saved mixes</span><span>T cycles</span></h3><div class="pal-list">${list}</div></section>
      <section class="side-sec"><h3 class="silk"><span>Mix your own</span><button class="btn silk" data-act="palrand">Random · R</button></h3><div class="toks">${toks}</div></section>
      <section class="side-sec"><h3 class="silk"><span>${T.draft ? "Unsaved mix, already live" : "Change a color or roll Random"}</span></h3>
        <div class="save"><input data-savename placeholder="Name this mix" value="${T.saveName.replace(/"/g, "&quot;")}" ${T.draft ? "" : "disabled"} aria-label="Mix name">
        <button class="btn silk" data-act="palsave" ${T.draft ? "" : "disabled"}>Save</button><button class="btn silk" data-act="paldrop" ${T.draft ? "" : "disabled"}>Discard</button></div></section>
      <div class="side-actions silk"><button class="btn" data-act="settings">Close · Esc</button></div></aside>`;
  }

  const hooks = {
    key(k) { if (k === "t") { cycle(); return true; } if (k === "r" && UI.ui.settingsOpen) { random(); return true; } return false; },
    click(act, val) {
      if (act === "pal") use(val);
      else if (act === "palrand") random();
      else if (act === "palsave") save();
      else if (act === "paldrop") use(T.active);
      else if (act === "paldel") remove(val);
      else return false;
      return true;
    },
    input(e) {
      if (e.target.matches("[data-tok]")) { edit(e.target.dataset.tok, e.target.value); return false; }
      if (e.target.matches("[data-savename]")) { T.saveName = e.target.value; return false; }
      return false;
    },
    change(e) { return e.target.matches("[data-tok]"); },
  };

  let start = "amber";
  try { start = new URLSearchParams(location.search).get("p") || store.get("gtop-pal", "amber"); } catch (e) {}
  use(start);
  window.THEME = { panel, hooks, state: T, presets: PRESETS };
})();
