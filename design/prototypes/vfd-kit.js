// VFD tower pieces shared by the VFD × Tower variants.
(function () {
  const { lit, segs, screws } = KIT;
  const vbar = (pct, hotAt = 0.85, n = 40) => `<div class="vbar" style="grid-template-columns:repeat(${n},1fr)">${segs(n, pct, hotAt)}</div>`;
  const pad = (v, n) => String(v).padStart(n, " ");

  // One stacked column of VFD windows. `compact` drops the per-core rows and the bars.
  function tower(m, { compact = false } = {}) {
    const { s, rows, fmt } = m;
    const src = KIT.source(rows);
    const memPct = s.memUsed / s.memTotal * 100;
    const hotSrc = src && src.share > 50;
    const big = compact ? "mid" : "big";
    return `<aside class="tower panel screws" aria-label="System">${screws}
      <div class="t-brand"><div class="silk">Apple M3 Max · 30-core GPU</div><div class="brand">gtop</div></div>
      <div class="win"><div class="split2"><span class="lab">Load source</span>${src ? lit(pad(src.share.toFixed(0), 3) + "%", hotSrc ? "sm hot" : "sm") : ""}</div>
        <div class="vfd sm ${hotSrc ? "hot" : ""}" style="margin-top:6px;position:relative;z-index:1">${src ? src.label.toUpperCase() : "IDLE"}</div>${compact ? "" : vbar(src ? src.share : 0, 0.5)}</div>
      <div class="win"><div class="split2"><span class="lab">CPU · ${KIT.CORES} cores</span><span class="lab">Load ${s.load[0].toFixed(1)}</span></div>
        <div class="${big}">${lit(pad(s.cpuTotal.toFixed(0), 3) + "%", s.cpuTotal > 85 ? "hot" : "")}</div>
        <div class="vcores" ${compact ? 'style="height:22px"' : ""}>${s.cores.map((c) => `<b class="${c > 90 ? "hot" : ""}" style="height:${Math.max(3, c)}%"></b>`).join("")}</div></div>
      <div class="win t-pair">
        <div><span class="lab">GPU</span><div class="mid">${lit(pad(s.gpuUtil.toFixed(0), 3) + "%")}</div></div>
        <div><span class="lab">VRAM</span><div class="mid">${lit(s.gpuMem.toFixed(1))}</div></div>
        ${compact ? "" : `<div style="grid-column:1/-1">${vbar(s.gpuUtil)}</div>`}</div>
      <div class="win t-pair">
        <div><span class="lab">Mem GB</span><div class="mid">${lit(s.memUsed.toFixed(1), memPct > 92 ? "hot" : "")}</div></div>
        <div><span class="lab">Swap GB</span><div class="mid">${lit(s.swap.toFixed(1))}</div></div>
        ${compact ? "" : `<div style="grid-column:1/-1">${vbar(memPct, 0.9)}</div>`}</div>
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

  // Detail as a column of VFD windows beside the table.
  function detailSide(m) {
    const { fmt, detail: d, ui } = m;
    const hot = d.cpu > KIT.LIMIT.hot;
    return `<aside class="side screws vside" aria-label="Process detail" data-scroll="detail">${screws}
      <header class="side-head"><div class="silk">PID ${d.pid} · ${d.user} · click another row to switch</div><div class="side-name">${d.name}</div></header>
      <section class="side-sec"><div class="win"><div class="split2"><span class="lab">CPU · last 90 s</span>${lit(d.cpu.toFixed(0) + "%", hot ? "mid hot" : "mid")}</div><div class="vtrace" style="margin-top:8px">${trace(m, d)}</div></div></section>
      <section class="side-sec"><div class="win t-pair">
        <div><span class="lab">Memory</span><div class="sm">${lit(fmt.mem(d.rss))}</div></div><div><span class="lab">GPU</span><div class="sm">${lit(d.gpu.toFixed(1) + "%")}</div></div>
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
      <div class="win dr-trace"><div class="split2"><span class="lab">CPU · last 90 s</span>${lit(d.cpu.toFixed(0) + "%", hot ? "mid hot" : "mid")}</div><div class="vtrace" style="margin-top:8px;height:72px">${trace(m, d, 90)}</div></div>
      <div class="win dr-nums">
        <div><span class="lab">Memory</span><div class="sm">${lit(fmt.mem(d.rss))}</div></div><div><span class="lab">GPU</span><div class="sm">${lit(d.gpu.toFixed(1) + "%")}</div></div>
        <div><span class="lab">Read</span><div class="sm">${lit(d.read.toFixed(1))}</div></div><div><span class="lab">Write MB/s</span><div class="sm">${lit(d.write.toFixed(1))}</div></div></div>
      <div class="dr-meta"><dl class="kv silk"><dt>Ports</dt><dd class="ok-t">${d.ports.length ? d.ports.join(" ") : "none"}</dd><dt>Started</dt><dd>${new Date(d.start).toLocaleString()}</dd><dt>Running</dt><dd>${fmt.up((Date.now() - d.start) / 1000)}</dd></dl>
        ${env(m)}<div class="dr-actions silk"><button class="btn silk" data-act="env">${ui.envOpen ? "Hide env" : "Env"} · E</button><button class="btn" data-act="close">Close · Esc</button><button class="btn danger" data-act="kill">Kill · X</button></div></div>
    </section>`;
  }

  function histMain(m, height = 110) {
    const c = KIT.columns(m.hist, m.fmt, m.ui);
    return `<div class="vhist"><section class="win"><div class="split2" style="margin-bottom:8px"><span class="lab">Total CPU · last ${m.ui.hours} h · one column per ${c.step} min</span><span class="lab">${c.note}</span></div>
      <div class="vtape" style="height:${height}px">${c.cols.replace(/<b class="off"/g, '<b class="off"')}</div><div class="axis" style="position:relative;z-index:1">${c.ticks}</div></section><div class="screen">${KIT.histTable(m)}</div></div>`;
  }

  window.VFD = { tower, detailSide, detailDrawer, histMain, trace, vbar };
})();
