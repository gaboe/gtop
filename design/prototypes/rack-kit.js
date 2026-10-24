// Rack building blocks shared by every variant. Each returns an HTML string.
(function () {
  const CORES = 14;
  const LIMIT = { hot: 400, culprit: 100 };
  const segs = (n, pct, hotAt = 0.85) => {
    const on = Math.round(Math.max(0, Math.min(100, pct)) / 100 * n);
    let o = "";
    for (let i = 0; i < n; i++) o += `<i class="${i < on ? (i >= n * hotAt ? "hot" : "on") : ""}"></i>`;
    return o;
  };
  // A process's CPU as a share of the whole machine, so every bar uses the same scale.
  const machine = (cpu) => cpu / CORES;
  const screws = `<i class="sc tl"></i><i class="sc tr"></i><i class="sc bl"></i><i class="sc br"></i>`;

  // Groups the heaviest processes by name: "rustc ×2 + cargo".
  function source(rows) {
    const heavy = rows.filter((p) => p.cpu > LIMIT.culprit);
    if (!heavy.length) return null;
    const by = new Map();
    heavy.forEach((p) => { const g = by.get(p.name) || { name: p.name, n: 0, cpu: 0 }; g.n++; g.cpu += p.cpu; by.set(p.name, g); });
    const groups = [...by.values()].sort((a, b) => b.cpu - a.cpu).slice(0, 3);
    const total = groups.reduce((a, g) => a + g.cpu, 0);
    return { label: groups.map((g) => g.name + (g.n > 1 ? " ×" + g.n : "")).join(" + "), cpu: total, share: machine(total) };
  }

  function bank(ui, extra = "") {
    const sw = (label, act, on, cls = "") => `<button class="sw silk ${cls}" data-act="${act}" aria-pressed="${on}"><span class="lamp"></span>${label}</button>`;
    return `${sw("Live", "view:live", ui.view === "live")}${sw("1 h", "hours:1", ui.view === "history" && ui.hours === 1)}${sw("8 h", "hours:8", ui.view === "history" && ui.hours === 8)}${sw("24 h", "hours:24", ui.view === "history" && ui.hours === 24)}
      <label class="search"><span class="silk">Find</span><input data-search placeholder="name, pid, command or port" aria-label="Search processes"></label>
      ${extra}${sw(ui.paused ? "Paused" : "Running", "pause", !ui.paused, ui.paused ? "alarm" : "")}`;
  }

  function liveTable({ ui, fmt, rows, detail }, opts = {}) {
    const th = (k, l, num) => `<th class="silk ${num ? "num" : ""}" ${k ? `data-act="sort:${k}"` : ""} ${ui.sort === k ? 'aria-sort="descending"' : ""}>${l}</th>`;
    const focus = rows.length && rows[0].cpu > LIMIT.culprit && !ui.query;
    const tr = rows.map((p, i) => {
      const hot = p.cpu > LIMIT.hot, culprit = p.cpu > LIMIT.culprit && i < 4;
      return `<tr data-act="row:${i}" data-pid="${p.pid}" class="${i === ui.selected ? "sel" : ""} ${culprit ? "culprit" : ""} ${detail && detail.pid === p.pid ? "open" : ""}">
        <td class="lamp"><span class="led ${hot ? "hot" : culprit ? "on" : ""}"></span></td><td>${p.name}</td><td class="num">${p.pid}</td>${opts.noUser ? "" : `<td>${p.user}</td>`}
        <td class="num"><span class="mini seg">${segs(14, machine(p.cpu), 0.72)}</span>${fmt.pct(p.cpu)}</td>
        <td class="num">${fmt.mem(p.rss)}</td><td class="num">${p.gpu >= 0.05 ? fmt.pct(p.gpu) : "·"}</td>
        <td class="port">${p.ports.join(" ")}</td>${opts.noCmd ? "" : `<td class="cmd">${p.cmd}</td>`}</tr>`;
    }).join("");
    const empty = rows.length ? "" : `<tr><td colspan="9" style="padding:24px 12px;color:var(--silk-dim)">No process matches “${ui.query}”. Esc clears the search.</td></tr>`;
    return `<div class="blades ${focus ? "focus" : ""}" data-scroll="table"><table>
      <thead><tr><th></th>${th("name", "Process")}${th("pid", "PID", 1)}${opts.noUser ? "" : th(null, "User")}${th("cpu", "CPU % · bar = share of Mac", 1)}${th("mem", "Memory", 1)}${th("gpu", "GPU %", 1)}${th(null, "Ports")}${opts.noCmd ? "" : th(null, "Command")}</tr></thead>
      <tbody>${tr}${empty}</tbody></table></div>`;
  }

  // History metrics: how a minute bucket maps to a column height, and when it counts as hot.
  const METRICS = {
    cpu: { label: "CPU", unit: "%", get: (b) => b.cpu, pct: (v) => v, hot: 70, fmt: (v) => v.toFixed(0) + " %" },
    mem: { label: "Memory", unit: "GB", get: (b) => b.mem, pct: (v) => v / 36 * 100, hot: 33, fmt: (v) => v.toFixed(1) + " GB" },
    gpu: { label: "GPU", unit: "%", get: (b) => b.gpu, pct: (v) => v, hot: 70, fmt: (v) => v.toFixed(0) + " %" },
    temp: { label: "CPU temperature", unit: "°C", get: (b) => b.temp, pct: (v) => (v - 30) / 80 * 100, hot: 90, fmt: (v) => v.toFixed(0) + " °C" },
  };
  function columns(hist, fmt, ui, metric = "cpu") {
    const M = METRICS[metric];
    const step = ui.step || (ui.hours === 1 ? 1 : ui.hours === 8 ? 5 : 10);
    const cols = [];
    for (let i = 0; i < hist.slice.length; i += step) {
      const g = hist.slice.slice(i, i + step), live = g.filter((b) => !b.closed);
      if (!live.length) { cols.push(`<b class="off" title="gtop was not running"></b>`); continue; }
      const v = live.reduce((a, b) => a + M.get(b), 0) / live.length;
      const at = GTOP.history.length - hist.slice.length + i, pick = ui.moment && ui.moment.at === at && ui.moment.step === step;
      cols.push(`<b class="${v > M.hot ? "hot" : ""} ${pick ? "pick" : ""}" data-act="moment:${at}-${step}" style="height:${Math.max(2, Math.min(100, M.pct(v)))}%" title="${fmt.clock(g[0].t)} · ${M.fmt(v)} · click for detail"></b>`);
    }
    const gs = hist.slice.find((b) => b.closed), ge = [...hist.slice].reverse().find((b) => b.closed);
    const note = gs ? `Hatched: gtop not running ${fmt.clock(gs.t)}–${fmt.clock(ge.t)}` : `Recorded ${hist.covered} of ${hist.minutes} min`;
    const n = hist.slice.length;
    const ticks = [0, .25, .5, .75, 1].map((f) => `<span class="silk">${fmt.clock(hist.slice[Math.min(n - 1, Math.floor(f * (n - 1)))].t)}</span>`).join("");
    return { cols: cols.join(""), note, ticks, step };
  }

  function histTable({ ui, fmt, hist }) {
    const max = hist.rows.length ? hist.rows[0].cpuMin : 1;
    const tr = hist.rows.map((r, i) => `<tr data-act="row:${i}" class="${i === ui.selected ? "sel" : ""} ${i < 3 ? "culprit" : ""}">
      <td class="lamp"><span class="led ${i < 3 ? "on" : ""}"></span></td><td>${r.name}</td>
      <td class="num">${fmt.mins(r.cpuMin)}</td><td><span class="share seg">${segs(30, r.cpuMin / max * 100, 2)}</span></td>
      <td class="num">${(r.cpuMin / hist.total * 100).toFixed(1)} %</td><td class="num">${r.gpuMin > 0.05 ? fmt.mins(r.gpuMin) : "·"}</td>
      <td class="num">${fmt.mem(r.peakMem)}</td><td class="num">${fmt.pct(r.peakCpu)}</td></tr>`).join("");
    return `<div class="blades" data-scroll="table"><table><thead><tr><th></th><th class="silk">Process</th><th class="silk num" aria-sort="descending">Core time</th><th class="silk">Share</th><th class="silk num">Of all CPU</th><th class="silk num">GPU time</th><th class="silk num">Peak memory</th><th class="silk num">Peak CPU %</th></tr></thead><tbody>${tr}</tbody></table></div>`;
  }

  function detailPanel({ ui, fmt, detail: d, trace }) {
    const t = trace.get(d.pid) || [];
    const pad = Array(Math.max(0, 60 - t.length)).fill(null).concat(t);
    const top = Math.max(CORES * 100 / 2, ...t);
    const bars = pad.map((v) => v === null ? `<b class="none"></b>` : `<b class="${v > LIMIT.hot ? "hot" : ""}" style="height:${Math.max(2, v / top * 100)}%"></b>`).join("");
    const env = ui.envOpen ? `<div class="well" style="margin-top:4px">PATH=/opt/homebrew/bin:/usr/bin:/bin<br>HOME=/Users/gabriel<br>CARGO_TARGET_DIR=target<br><span class="synth">Synthetic. The real app reads it on demand.</span></div>` : "";
    return `<aside class="side screws" aria-label="Process detail" data-scroll="detail">${screws}
      <header class="side-head"><div class="silk">PID ${d.pid} · ${d.user}</div><div class="side-name">${d.name}</div></header>
      <section class="side-sec"><h3 class="silk"><span>CPU · last 90 s</span><span class="px ${d.cpu > LIMIT.hot ? "hot-t" : ""}">${d.cpu.toFixed(0)}%</span></h3><div class="trace">${bars}</div></section>
      <section class="side-sec"><dl class="kv silk">
        <dt>Memory</dt><dd>${fmt.mem(d.rss)}</dd><dt>GPU</dt><dd>${d.gpu.toFixed(1)} %</dd>
        <dt>Disk read</dt><dd>${d.read.toFixed(1)} MB/s</dd><dt>Disk write</dt><dd>${d.write.toFixed(1)} MB/s</dd>
        <dt>Ports</dt><dd class="ok-t">${d.ports.length ? d.ports.join(" ") : "none"}</dd>
        <dt>Started</dt><dd>${new Date(d.start).toLocaleString()}</dd><dt>Running for</dt><dd>${fmt.up((Date.now() - d.start) / 1000)}</dd></dl></section>
      <section class="side-sec"><h3 class="silk"><span>Command</span></h3><div class="well">${d.cmd}</div></section>
      <section class="side-sec"><h3 class="silk"><span>Environment</span><button class="btn silk" data-act="env">${ui.envOpen ? "Hide" : "Load"} · E</button></h3>${env}</section>
      <div class="side-actions silk"><button class="btn" data-act="close">Close · Esc</button><button class="btn danger" data-act="kill">Kill · X</button></div></aside>`;
  }

  // gtop's own cost per refresh: the product's main claim, shown as a measurement.
  function self(s, cls = "") {
    const f = s.self;
    const cell = (l, v, u) => `<span class="self-c"><span class="silk">${l}</span><span class="px">${v}</span><span class="silk">${u}</span></span>`;
    return `<div class="self ${cls}" title="gtop's own cost, measured every refresh"><span class="self-h silk"><span class="led ok-led"></span>gtop self</span>
      ${cell("CPU", f.cpu.toFixed(2), "%")}${cell("Mem", f.mem.toFixed(0), "MB")}${cell("Tick", f.tickMs.toFixed(1), "ms")}${cell("Frame", f.frameMs.toFixed(1), "ms")}${cell("", f.fps, "fps")}</div>`;
  }

  // Unlit segments show behind lit ones, as on a real VFD tube.
  const lit = (txt, cls = "", ghost = "8") => `<span class="ghost"><span class="vfd dim ${cls}">${String(txt).replace(/[0-9]/g, ghost)}</span><span class="vfd ${cls}">${txt}</span></span>`;

  // Units as 5x7 dot-matrix characters, the way a VFD tube draws them, with unlit dots behind.
  const GLYPH = {
    "%": ["11000", "11001", "00010", "00100", "01000", "10011", "00011"],
    "°": ["01100", "10010", "10010", "01100", "00000", "00000", "00000"],
  };
  const glyph = (u, hot) => `<svg class="u-dm ${hot ? "hot" : ""}" viewBox="0 0 10 14" role="img" aria-label="${u === "%" ? "percent" : "degrees"}">${GLYPH[u].map((r, y) => [...r].map((b, x) => `<circle class="${b === "1" ? "on" : "off"}" cx="${x * 2 + 1}" cy="${y * 2 + 1}" r=".82"/>`).join("")).join("")}</svg>`;
  const unit = (txt, u, cls = "") => {
    const hot = /\bhot\b/.test(cls), size = cls.replace(/\bhot\b/, "").trim(), h = hot ? "hot" : "";
    return `<span class="unit ${size}"><span class="ghost"><span class="vfd dim ${h}">${String(txt).replace(/[0-9 ]/g, "8")}</span><span class="vfd ${h}">${txt}</span></span>${glyph(u, hot)}</span>`;
  };

  function side(m) { return m.ui.settingsOpen ? THEME.panel() : m.detail ? detailPanel(m) : ""; }

  function strip({ ui, rows }) {
    if (ui.confirmKill) {
      const p = ui.confirmKill;
      return `<footer class="strip kill" role="alertdialog" aria-label="Confirm kill"><span class="silk" style="color:var(--silk)">Kill <b>${p.name}</b> · PID ${p.pid} · started ${new Date(p.start).toLocaleString()}?</span>
        <button class="btn yes silk" data-act="confirm">Kill · Y</button><button class="btn silk" data-act="cancel">Keep · N</button></footer>`;
    }
    const k = (key, label) => `<span><kbd>${key}</kbd>${label}</span>`;
    return `<footer class="strip silk" role="status">${k("/", "Find")}${k("J K", "Move")}${k("↵", "Detail")}${k("C M G P", "Sort")}${k("Space", "Pause")}${k("X", "Kill")}${k("Tab", "History")}${k("T", "Palette")}
      <span class="end"><span class="synth">Synthetic demo data · ${rows.length} processes</span><button class="sw silk" data-act="settings" aria-pressed="${ui.settingsOpen}"><span class="lamp"></span>Settings · S</button></span></footer>`;
  }

  window.KIT = { METRICS, lit, unit, self, CORES, LIMIT, segs, machine, screws, source, bank, liveTable, histTable, columns, detailPanel, side, strip };
})();
