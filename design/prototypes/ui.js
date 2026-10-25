// Shared prototype behavior: view, sort, search, selection, detail, settings, keyboard.
(function () {
  const ui = {
    view: "live",
    hours: 1,
    sort: "cpu",
    query: "",
    selected: 0,
    selPid: null,
    paused: false,
    confirmKill: null,
    detailPid: null,
    envOpen: false,
    settingsOpen: false,
    moment: null,
  };
  const fmt = {
    pct: (v) => v.toFixed(1),
    mem: (mb) => (mb >= 1024 ? (mb / 1024).toFixed(2) + " GB" : mb.toFixed(0) + " MB"),
    mins: (m) => (m >= 60 ? Math.floor(m / 60) + " h " + String(Math.round(m % 60)).padStart(2, "0") + " min" : m.toFixed(1) + " min"),
    up: (s) => { const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60); return `${d}d ${h}h ${m}m`; },
    clock: (t) => t.toTimeString().slice(0, 5),
  };
  const TRACE = new Map();
  let lastTick = -1;
  function rows(keepIndex) {
    const q = ui.query.toLowerCase();
    let list = GTOP.state.procs.filter((p) => !q || p.name.toLowerCase().includes(q) || String(p.pid).includes(q) || p.cmd.toLowerCase().includes(q) || p.ports.some((x) => String(x).includes(q)));
    const key = { cpu: (p) => -p.cpu, mem: (p) => -p.rss, gpu: (p) => -p.gpu, pid: (p) => p.pid, name: (p) => p.name.toLowerCase() }[ui.sort];
    list = list.slice().sort((a, b) => (key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0));
    if (!keepIndex && ui.selPid != null) { const i = list.findIndex((p) => p.pid === ui.selPid); if (i >= 0) ui.selected = i; }
    ui.selected = Math.max(0, Math.min(ui.selected, list.length - 1));
    ui.selPid = list[ui.selected] ? list[ui.selected].pid : null;
    return list;
  }
  let render = () => {}, hooks = {};
  function draw(keepIndex) {
    const s = GTOP.state;
    if (s.tick !== lastTick) {
      lastTick = s.tick;
      s.procs.forEach((p) => { const t = TRACE.get(p.pid) || []; t.push(p.cpu); if (t.length > 60) t.shift(); TRACE.set(p.pid, t); });
    }
    if (ui.detailPid && !s.procs.some((p) => p.pid === ui.detailPid)) ui.detailPid = null;
    const list = rows(keepIndex);
    render({ ui, fmt, rows: list, s, hist: GTOP.window(ui.hours), detail: ui.detailPid && s.procs.find((p) => p.pid === ui.detailPid), trace: TRACE });
  }
  const kill = () => { const p = ui.confirmKill; GTOP.state.procs = GTOP.state.procs.filter((x) => x.pid !== p.pid); ui.confirmKill = null; };
  function start(fn, h = {}) {
    render = fn; hooks = h;
    draw();
    setInterval(() => { if (!ui.paused) { GTOP.tick(); draw(); } }, 1500);
    document.addEventListener("keydown", (e) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.target.matches("input, textarea")) {
        if (e.target.matches("[data-search]") && (e.key === "Escape" || e.key === "Enter")) { e.target.blur(); e.preventDefault(); }
        return;
      }
      const k = e.key;
      if (ui.confirmKill) {
        if (k === "y" || k === "Enter") kill();
        else if (k === "n" || k === "Escape") ui.confirmKill = null;
        else return;
        e.preventDefault(); draw(); return;
      }
      let moved = false;
      if (k === "/") { const el = document.querySelector("[data-search]"); el && el.focus(); e.preventDefault(); return; }
      else if (k === "j" || k === "ArrowDown") { ui.selected++; moved = true; }
      else if (k === "k" || k === "ArrowUp") { ui.selected = Math.max(0, ui.selected - 1); moved = true; }
      else if (k === " ") ui.paused = !ui.paused;
      else if (k === "c") ui.sort = "cpu";
      else if (k === "m") ui.sort = "mem";
      else if (k === "g") ui.sort = "gpu";
      else if (k === "p") ui.sort = "pid";
      else if (k === "Tab") { ui.view = ui.view === "live" ? "history" : "live"; ui.selected = 0; moved = true; }
      else if (k === "1") { ui.view = "history"; ui.hours = 1; }
      else if (k === "8") { ui.view = "history"; ui.hours = 8; }
      else if (k === "2") { ui.view = "history"; ui.hours = 24; }
      else if (k === "Enter" && ui.view === "live") { ui.detailPid = ui.selPid; ui.envOpen = false; ui.moment = null; }
      else if (k === "Escape") { if (ui.settingsOpen) ui.settingsOpen = false; else if (ui.moment) ui.moment = null; else if (ui.detailPid) ui.detailPid = null; else return; }
      else if (k === "e" && ui.detailPid) ui.envOpen = !ui.envOpen;
      else if (k === "s") ui.settingsOpen = !ui.settingsOpen;
      else if (k === "x" && ui.view === "live") ui.confirmKill = GTOP.state.procs.find((p) => p.pid === (ui.detailPid || ui.selPid));
      else if (!(hooks.key && hooks.key(k))) return;
      e.preventDefault();
      draw(moved);
    });
    document.addEventListener("input", (e) => {
      if (e.target.matches("[data-search]")) { ui.query = e.target.value; ui.selected = 0; draw(true); }
      else if (hooks.input && hooks.input(e)) draw(true);
    });
    document.addEventListener("change", (e) => { if (hooks.change && hooks.change(e)) draw(true); });
    document.addEventListener("click", (e) => {
      const el = e.target.closest("[data-act]");
      if (!el) return;
      const [act, val] = el.dataset.act.split(":");
      let moved = false;
      if (act === "view") { ui.view = val; ui.selected = 0; moved = true; }
      else if (act === "hours") { ui.view = "history"; ui.hours = +val; }
      else if (act === "sort") ui.sort = val;
      else if (act === "row") { ui.selected = +val; moved = true; if (ui.view === "live" && el.dataset.pid) { ui.detailPid = +el.dataset.pid; ui.envOpen = false; ui.settingsOpen = false; ui.moment = null; } }
      else if (act === "pause") ui.paused = !ui.paused;
      else if (act === "kill") ui.confirmKill = GTOP.state.procs.find((p) => p.pid === (ui.detailPid || ui.selPid));
      else if (act === "confirm") kill();
      else if (act === "cancel") ui.confirmKill = null;
      else if (act === "detail") { ui.detailPid = +val || ui.selPid; ui.envOpen = false; ui.moment = null; }
      else if (act === "close") { ui.detailPid = null; ui.moment = null; }
      else if (act === "env") ui.envOpen = !ui.envOpen;
      else if (act === "settings") ui.settingsOpen = !ui.settingsOpen;
      else if (!(hooks.click && hooks.click(act, val, el))) return;
      draw(moved);
    });
    document.addEventListener("dblclick", (e) => { const r = e.target.closest("[data-pid]"); if (r && ui.view === "live") { ui.detailPid = +r.dataset.pid; ui.envOpen = false; draw(); } });
  }
  // The app is re-rendered every tick, so keep focused inputs and scroll positions stable.
  window.keepSearch = (html, host) => {
    const a = document.activeElement;
    const old = a && host.contains(a) && a.matches("[data-search], [data-savename]") ? a : null;
    const sel = old && (old.matches("[data-search]") ? "[data-search]" : "[data-savename]");
    const pos = old && old.selectionStart;
    const scroll = [...host.querySelectorAll("[data-scroll]")].map((el) => [el.dataset.scroll, el.scrollTop]);
    host.innerHTML = html;
    const s = host.querySelector("[data-search]");
    if (s) s.value = ui.query;
    scroll.forEach(([k, t]) => { const el = host.querySelector(`[data-scroll="${k}"]`); if (el) el.scrollTop = t; });
    if (sel) { const now = host.querySelector(sel); if (now) { now.focus(); now.setSelectionRange(pos, pos); } }
  };
  window.UI = { start, ui, fmt, redraw: () => draw(true) };
})();
