// VFD tower pieces shared by the VFD × Tower variants.
(function () {
  const { lit, unit, segs, screws } = KIT;
  const vbar = (pct, hotAt = 0.85, n = 40) => `<div class="vbar" style="grid-template-columns:repeat(${n},1fr)">${segs(n, pct, hotAt)}</div>`;
  const pad = (v, n) => String(v).padStart(n, " ");

  // Per-core load: one 12-segment column per core, Performance and Efficiency cores in their own banks with each bank's average.
  const PCORES = 10;
  function coreBanks(s) {
    const col = (c) => { const on = Math.round(c / 100 * 12); return `<div class="ccol">${Array.from({ length: 12 }, (_, k) => `<i class="${k < on ? (c > 90 && k >= 9 ? "hot" : "on") : ""}"></i>`).join("")}</div>`; };
    const bank = (cs, name) => { const avg = cs.reduce((a, b) => a + b, 0) / cs.length; return `<div class="cgroup"><div class="bank">${cs.map(col).join("")}</div><div class="cnum">${cs.map((_, k) => `<span>${k + 1}</span>`).join("")}</div><div class="gname"><span class="lab">${name}</span><span class="lab">${avg.toFixed(0)} %</span></div></div>`; };
    return `<div class="cols">${bank(s.cores.slice(0, PCORES), "P cores")}${bank(s.cores.slice(PCORES), "E cores")}</div>`;
  }

  // One stacked column of VFD windows. `compact` drops the per-core rows and the bars.
  function tower(m, { compact = false } = {}) {
    const { s, rows, fmt } = m;
    const memPct = s.memUsed / s.memTotal * 100;
    const big = compact ? "mid" : "big";
    return `<aside class="tower panel screws" aria-label="System">${screws}
      <div class="t-brand"><div class="silk">Apple M3 Max · 30-core GPU</div><div class="brand">gtop</div></div>
      <div class="win"><div class="split2"><span class="lab">CPU · 10 P + 4 E cores</span><span class="lab">Load ${s.load[0].toFixed(1)}</span></div>
        <div class="${big}">${unit(pad(s.cpuTotal.toFixed(0), 3), "%", s.cpuTotal > 85 ? "hot" : "")}</div>
        ${coreBanks(s)}</div>
      <div class="win t-pair">
        <div><span class="lab">Mem GB</span><div class="mid">${lit(s.memUsed.toFixed(1), memPct > 92 ? "hot" : "")}</div></div>
        <div><span class="lab">Swap GB</span><div class="mid">${lit(s.swap.toFixed(1))}</div></div>
        ${compact ? "" : `<div style="grid-column:1/-1">${vbar(memPct, 0.9)}</div>`}</div>
      <div class="win t-pair">
        <div><span class="lab">GPU</span><div class="mid">${unit(pad(s.gpuUtil.toFixed(0), 3), "%")}</div></div>
        <div><span class="lab">VRAM</span><div class="mid">${lit(s.gpuMem.toFixed(1))}</div></div>
        ${compact ? "" : `<div style="grid-column:1/-1">${vbar(s.gpuUtil)}</div>`}</div>
      <div class="win t-temps"><span class="lab" style="grid-column:1/-1">Temperature °C</span>
        ${[["CPU", s.temps.CPU, 90], ["GPU", s.temps.GPU, 90], ["SSD", s.temps.SSD, 70], ["Bat", s.temps.Battery, 45]].map(([l, v, hot]) => `<div><div class="mid">${lit(v.toFixed(0), v > hot ? "hot" : "")}</div><span class="lab">${l}</span></div>`).join("")}</div>
      <div class="t-self">${KIT.self(s, "stack")}</div>
      <div class="t-foot silk">Up ${fmt.up(s.uptime)} · net ↓ ${s.net.rx.toFixed(1)} ↑ ${s.net.tx.toFixed(1)} MB/s</div>
    </aside>`;
  }

  function trace(m, d, n = 60) {
    const t = m.trace.get(d.pid) || [];
    const padded = Array(Math.max(0, n - t.length)).fill(null).concat(t.slice(-n));
    const top = Math.max(KIT.CORES * 100 / 2, ...t);
    return padded.map((v) => v === null ? `<b class="none"></b>` : `<b class="${v > KIT.LIMIT.hot ? "hot" : ""}" style="height:${Math.max(2, v / top * 100)}%"></b>`).join("");
  }

  function env(m) {
    return m.ui.envOpen ? `<div class="well" style="margin-top:6px">PATH=/opt/homebrew/bin:/usr/bin:/bin<br>HOME=/Users/gabriel<br>CARGO_TARGET_DIR=target<br><span class="synth">Synthetic. The real app reads it on demand.</span></div>` : "";
  }


  // What a process is: gtop's built-in catalog first, then what its path says. The source is always named.
  const CATALOG = {
    rustc: ["Rust compiler", "Developer tool", "Compiles one Rust crate. Many at once during a cargo build; each exits when its crate is done."],
    cargo: ["Rust build tool", "Developer tool", "Drives a Rust build and starts rustc and clang for each crate."],
    clang: ["C and C++ compiler", "Developer tool", "Compiles native code, often for Rust crates with C dependencies."],
    dotnet: [".NET runtime and CLI", "Developer tool", "Runs .NET builds, tests and apps."],
    Docker: ["Docker virtual machine", "Developer tool", "The Linux VM behind Docker containers. Its load is the sum of everything running in containers."],
    node: ["Node.js runtime", "Developer tool", "Runs JavaScript tools such as dev servers and bundlers."],
    bun: ["Bun runtime", "Developer tool", "Runs JavaScript and TypeScript tools and tests."],
    postgres: ["PostgreSQL server", "Service", "Database server. Brief spikes are checkpoints and vacuum."],
    WindowServer: ["macOS window compositor", "macOS", "Draws every window on screen. Load rises with animation, many displays and busy apps."],
    kernel_task: ["macOS kernel", "macOS", "The kernel itself. High CPU can also be thermal throttling: macOS keeps the CPU busy here to cool down."],
    mds_stores: ["Spotlight indexer", "macOS", "Indexes files for Spotlight search. Busy after builds or large file changes."],
    photoanalysisd: ["Photos analysis", "macOS", "Analyzes the Photos library for faces and scenes, mostly when the Mac is idle."],
    "Google Chrome Helper (GPU)": ["Chrome GPU process", "App helper", "Draws Chrome tabs on the GPU for all of Chrome."],
    "Code Helper (Renderer)": ["VS Code window renderer", "App helper", "Renders one VS Code window and its extensions UI."],
    "Slack Helper": ["Slack helper", "App helper", "Part of the Slack app."],
    Spotify: ["Spotify", "App", "Music player."],
    codex: ["Codex CLI", "Developer tool", "OpenAI coding agent running in a terminal."],
  };
  function identity(d) {
    const c = CATALOG[d.name];
    const app = (d.cmd.match(/\/([^/]+)\.app\//) || [])[1];
    if (c) return { what: c[0], kind: c[1], about: c[2], source: "gtop catalog" };
    if (app) return { what: `Part of ${app}`, kind: "App helper", about: `Runs from inside ${app}.app.`, source: "from its path" };
    if (d.cmd.startsWith("/usr/libexec/") || d.cmd.startsWith("/System/")) return { what: "macOS system process", kind: "macOS", about: "Ships with macOS.", source: "from its path" };
    return { what: "Unknown process", kind: "Unknown", about: "Not in gtop's catalog and its path says nothing more.", source: "no match" };
  }
  // History of every process with this name in the recorded 24 h, in minute buckets.
  function behavior(d, fmt) {
    const H = GTOP.history, live = H.filter((b) => !b.closed);
    const series = H.map((b) => (b.closed ? null : (b.per[d.name] || { cpu: 0, gpu: 0, mem: 0 })));
    const seen = series.filter((x) => x && (x.cpu > 1 || x.mem > 0));
    if (!seen.length) return null;
    const active = series.filter((x) => x && x.cpu > 5);
    const coreMin = series.reduce((a, x) => a + (x ? x.cpu / 100 : 0), 0);
    let peak = { cpu: 0, t: null }, peakMem = 0, run = 0, longest = 0;
    series.forEach((x, i) => { if (!x) { run = 0; return; } if (x.cpu > peak.cpu) peak = { cpu: x.cpu, t: H[i].t }; peakMem = Math.max(peakMem, x.mem); run = x.cpu > 50 ? run + 1 : 0; longest = Math.max(longest, run); });
    const avgActive = active.length ? active.reduce((a, x) => a + x.cpu, 0) / active.length : 0;
    const share = active.length / live.length;
    const pattern = share > 0.8 ? `Runs steadily: busy in ${Math.round(share * 100)} % of recorded minutes.` : share > 0.25 ? `Busy often: ${Math.round(share * 100)} % of recorded minutes.` : `Runs in bursts: busy in ${active.length} of ${live.length} recorded minutes.`;
    const cols = [];
    for (let i = 0; i < series.length; i += 10) {
      const g = series.slice(i, i + 10), l = g.filter(Boolean);
      if (!l.length) { cols.push(`<b class="off"></b>`); continue; }
      const v = l.reduce((a, x) => a + x.cpu, 0) / l.length;
      cols.push(`<b class="${v > KIT.LIMIT.hot ? "hot" : ""}" style="height:${Math.max(2, Math.min(100, v / (KIT.CORES * 100 / 2) * 100))}%"></b>`);
    }
    const ratio = avgActive ? d.cpu / avgActive : 0;
    const now = ratio > 1.5 ? `Now ${d.cpu.toFixed(0)} %, ${ratio.toFixed(1)}× its usual busy load.` : ratio && ratio < 0.5 ? `Now ${d.cpu.toFixed(0)} %, well below its usual busy load.` : `Now ${d.cpu.toFixed(0)} %, about its usual busy load.`;
    return { coreMin, peak, peakMem, longest, avgActive, pattern, now, cols: cols.join("") };
  }
  function analysis(m) {
    const { fmt, detail: d } = m;
    const id = identity(d), b = behavior(d, fmt);
    const what = `<section class="side-sec"><h3 class="silk"><span>What it is</span><span class="src">${id.source}</span></h3>
      <div class="what"><b>${id.what}</b><span class="kind">${id.kind}</span></div><p class="about">${id.about}</p></section>`;
    if (!b) return what + `<section class="side-sec"><h3 class="silk"><span>History · 24 h</span></h3><p class="about">Not seen in the recorded history. gtop records only while it runs.</p></section>`;
    return what + `<section class="side-sec"><h3 class="silk"><span>History · 24 h</span><span class="src">all ${d.name} processes</span></h3>
      <div class="win"><div class="vtape" style="height:40px">${b.cols}</div><div class="split2" style="margin-top:4px"><span class="lab">24 h ago</span><span class="lab">now</span></div></div>
      <p class="about strong">${b.pattern} ${b.now}</p>
      <dl class="kv silk"><dt>Core time</dt><dd>${fmt.mins(b.coreMin)}</dd><dt>Busy average</dt><dd>${b.avgActive.toFixed(0)} %</dd>
        <dt>Peak CPU</dt><dd>${b.peak.cpu.toFixed(0)} % at ${fmt.clock(b.peak.t)}</dd><dt>Longest busy run</dt><dd>${b.longest} min over 50 %</dd><dt>Peak memory</dt><dd>${fmt.mem(b.peakMem)}</dd></dl></section>`;
  }

  // Detail as a column of VFD windows beside the table.
  function detailSide(m) {
    const { fmt, detail: d, ui } = m;
    const hot = d.cpu > KIT.LIMIT.hot;
    return `<aside class="side screws vside" aria-label="Process detail" data-scroll="detail">${screws}
      <header class="side-head"><div class="silk">PID ${d.pid} · ${d.user} · click another row to switch</div><div class="side-name">${d.name}</div></header>
      <section class="side-sec"><div class="win"><div class="split2"><span class="lab">CPU · last 90 s</span>${unit(pad(d.cpu.toFixed(0), 4), "%", hot ? "mid hot" : "mid")}</div><div class="vtrace" style="margin-top:8px">${trace(m, d)}</div></div></section>
      ${analysis(m)}
      <section class="side-sec"><div class="win t-pair">
        <div><span class="lab">Memory</span><div class="sm">${lit(fmt.mem(d.rss))}</div></div><div><span class="lab">GPU</span><div class="sm">${unit(pad(d.gpu.toFixed(1), 5), "%")}</div></div>
        <div><span class="lab">Disk read</span><div class="sm">${lit(d.read.toFixed(1) + " MB/s")}</div></div><div><span class="lab">Disk write</span><div class="sm">${lit(d.write.toFixed(1) + " MB/s")}</div></div></div>
        <dl class="kv silk" style="margin-top:12px"><dt>Ports</dt><dd class="ok-t">${d.ports.length ? d.ports.join(" ") : "none"}</dd><dt>Started</dt><dd>${new Date(d.start).toLocaleString()}</dd><dt>Running for</dt><dd>${fmt.up((Date.now() - d.start) / 1000)}</dd></dl></section>
      <section class="side-sec"><h3 class="silk"><span>Command</span></h3><div class="well">${d.cmd}</div></section>
      <section class="side-sec"><h3 class="silk"><span>Environment</span><button class="btn silk" data-act="env">${ui.envOpen ? "Hide" : "Load"} · E</button></h3>${env(m)}</section>
      <div class="side-actions silk"><button class="btn" data-act="close">Close · Esc</button><button class="btn danger" data-act="kill">Kill · X</button></div></aside>`;
  }

  // Detail as a wide drawer under the table.
  function detailDrawer(m) {
    const { fmt, detail: d, ui } = m;
    const hot = d.cpu > KIT.LIMIT.hot;
    return `<section class="drawer panel screws" aria-label="Process detail">${screws}
      <div class="dr-id"><div class="silk">PID ${d.pid} · ${d.user}</div><div class="side-name">${d.name}</div><div class="well" style="margin-top:8px">${d.cmd}</div></div>
      <div class="win dr-trace"><div class="split2"><span class="lab">CPU · last 90 s</span>${unit(pad(d.cpu.toFixed(0), 4), "%", hot ? "mid hot" : "mid")}</div><div class="vtrace" style="margin-top:8px;height:72px">${trace(m, d, 90)}</div></div>
      <div class="win dr-nums">
        <div><span class="lab">Memory</span><div class="sm">${lit(fmt.mem(d.rss))}</div></div><div><span class="lab">GPU</span><div class="sm">${unit(pad(d.gpu.toFixed(1), 5), "%")}</div></div>
        <div><span class="lab">Read</span><div class="sm">${lit(d.read.toFixed(1))}</div></div><div><span class="lab">Write MB/s</span><div class="sm">${lit(d.write.toFixed(1))}</div></div></div>
      <div class="dr-meta"><dl class="kv silk"><dt>Ports</dt><dd class="ok-t">${d.ports.length ? d.ports.join(" ") : "none"}</dd><dt>Started</dt><dd>${new Date(d.start).toLocaleString()}</dd><dt>Running</dt><dd>${fmt.up((Date.now() - d.start) / 1000)}</dd></dl>
        ${env(m)}<div class="dr-actions silk"><button class="btn silk" data-act="env">${ui.envOpen ? "Hide env" : "Env"} · E</button><button class="btn" data-act="close">Close · Esc</button><button class="btn danger" data-act="kill">Kill · X</button></div></div>
    </section>`;
  }


  // Detail of one histogram column: what the Mac and its processes did in that span.
  function momentSide(m, metric = "cpu") {
    const { fmt, ui } = m;
    const { at, step } = ui.moment;
    const span = GTOP.history.slice(at, at + step), live = span.filter((b) => !b.closed);
    const t0 = span[0].t, t1 = new Date(span[span.length - 1].t.getTime() + 60000);
    const when = step === 1 ? fmt.clock(t0) : `${fmt.clock(t0)}–${fmt.clock(t1)}`;
    const head = `<header class="side-head"><div class="silk">${t0.toLocaleDateString()} · ${step} min · ${live.length} recorded · ← → step</div><div class="side-name">${when}</div></header>`;
    if (!live.length) return `<aside class="side screws vside" aria-label="Moment detail">${screws}${head}<section class="side-sec"><div class="well">gtop was not running. Nothing was recorded for this span.</div></section><div class="side-actions silk"><button class="btn" data-act="close">Close · Esc</button></div></aside>`;
    const avg = (f) => live.reduce((a, b) => a + f(b), 0) / live.length, peak = (f) => Math.max(...live.map(f));
    const num = (k, l, f, d, u) => { const M = KIT.METRICS[k], a = avg(f), p = peak(f); return `<div class="${k === metric ? "mpick" : ""}"><span class="lab">${l}</span><div class="sm">${u === "G" ? lit(a.toFixed(d) + u, a > M.hot ? "hot" : "") : unit(a.toFixed(d), u, a > M.hot ? "hot" : "")}</div><span class="lab">Peak ${p.toFixed(d)}${u}</span></div>`; };
    const agg = {};
    live.forEach((b) => Object.entries(b.per).forEach(([n, x]) => { const a = (agg[n] ||= { name: n, cpuMin: 0, gpuMin: 0, peakMem: 0 }); a.cpuMin += x.cpu / 100; a.gpuMin += x.gpu / 100; a.peakMem = Math.max(a.peakMem, x.mem); }));
    const key = { cpu: "cpuMin", gpu: "gpuMin", mem: "peakMem", temp: "cpuMin" }[metric];
    const rows = Object.values(agg).sort((a, b) => b[key] - a[key]).slice(0, 8);
    const max = rows.length ? rows[0][key] || 1 : 1;
    const show = (r) => key === "peakMem" ? fmt.mem(r.peakMem) : fmt.mins(r[key]);
    const running = (n) => GTOP.state.procs.find((p) => p.name === n);
    const list = rows.map((r, i) => { const p = running(r.name); return `<tr class="${i < 3 ? "culprit" : ""}" ${p ? `data-act="detail:${p.pid}" title="Open the running process"` : ""}>
      <td class="lamp"><span class="led ${i < 3 ? "on" : ""}"></span></td><td>${r.name}${p ? "" : ' <span class="silk">· exited</span>'}</td>
      <td><span class="share seg">${segs(16, r[key] / max * 100, 2)}</span></td><td class="num">${show(r)}</td></tr>`; }).join("");
    const by = { cpu: "core time", mem: "peak memory", gpu: "GPU time", temp: "core time (heat follows CPU)" }[metric];
    return `<aside class="side screws vside" aria-label="Moment detail" data-scroll="detail">${screws}${head}
      <section class="side-sec"><div class="win mnums">${num("cpu", "CPU", (b) => b.cpu, 0, "%")}${num("mem", "Memory", (b) => b.mem, 1, "G")}${num("gpu", "GPU", (b) => b.gpu, 0, "%")}${num("temp", "CPU °C", (b) => b.temp, 0, "°")}</div></section>
      <section class="side-sec"><h3 class="silk"><span>Top processes · ${by}</span></h3><div class="blades mtable"><table><tbody>${list}</tbody></table></div>
        <p class="silk" style="margin:8px 0 0">Click a running process to open it. Exited processes keep only their history.</p></section>
      <div class="side-actions silk"><button class="btn" data-act="close">Close · Esc</button>${ui.view === "live" ? '<button class="btn" data-act="momenthist">Full hour · 1</button>' : ""}</div></aside>`;
  }

  function histMain(m, height = 110) {
    const c = KIT.columns(m.hist, m.fmt, m.ui);
    return `<div class="vhist"><section class="win"><div class="split2" style="margin-bottom:8px"><span class="lab">Total CPU · last ${m.ui.hours} h · one column per ${c.step} min</span><span class="lab">${c.note}</span></div>
      <div class="vtape" style="height:${height}px">${c.cols.replace(/<b class="off"/g, '<b class="off"')}</div><div class="axis" style="position:relative;z-index:1">${c.ticks}</div></section><div class="screen">${KIT.histTable(m)}</div></div>`;
  }

  window.VFD = { tower, detailSide, momentSide, detailDrawer, histMain, trace, vbar };
})();
