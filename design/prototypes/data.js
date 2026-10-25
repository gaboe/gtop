// Synthetic demo data shared by every prototype. Not real measurements.
(function () {
  let seed = 7;
  const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  const pick = (a) => a[Math.floor(rnd() * a.length)];

  const base = [
    ["cargo", "gabriel", 1, 0, 1200, "cargo build --release -p gtop"],
    ["rustc", "gabriel", 1, 0, 2600, "rustc --crate-name gpui --edition=2021"],
    ["rustc", "gabriel", 1, 0, 1900, "rustc --crate-name gtop_core"],
    ["clang", "gabriel", 0.6, 0, 410, "clang -cc1 -triple arm64-apple-macosx"],
    ["WindowServer", "_windowserver", 0.3, 1, 620, "/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer"],
    ["Google Chrome Helper (GPU)", "gabriel", 0.25, 0.8, 880, "Google Chrome Helper (GPU) --type=gpu-process"],
    ["Google Chrome", "gabriel", 0.15, 0, 1400, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
    ["Code Helper (Renderer)", "gabriel", 0.2, 0.1, 980, "Code Helper (Renderer) --type=renderer"],
    ["node", "gabriel", 0.35, 0, 720, "node vite dev --port 1420"],
    ["bun", "gabriel", 0.2, 0, 300, "bun nexus check be affected --async"],
    ["dotnet", "gabriel", 0.5, 0, 1800, "dotnet test Nexus.Integration.Tests.dll"],
    ["Docker", "gabriel", 0.3, 0, 2100, "com.docker.virtualization --kernel"],
    ["postgres", "_postgres", 0.1, 0, 240, "postgres: checkpointer"],
    ["mds_stores", "root", 0.2, 0, 160, "/System/Library/Frameworks/CoreServices.framework/mds_stores"],
    ["kernel_task", "root", 0.15, 0, 3800, "kernel_task"],
    ["Slack Helper", "gabriel", 0.05, 0.05, 520, "Slack Helper (Renderer)"],
    ["Spotify", "gabriel", 0.04, 0.02, 380, "/Applications/Spotify.app/Contents/MacOS/Spotify"],
    ["gtop", "gabriel", 0.01, 0.01, 46, "/Applications/gtop.app/Contents/MacOS/gtop"],
    ["codex", "gabriel", 0.06, 0, 210, "codex --model gpt-6.1-sol"],
    ["herdr", "gabriel", 0.02, 0, 64, "herdr server"],
    ["photoanalysisd", "gabriel", 0.08, 0.3, 190, "/System/Library/PrivateFrameworks/PhotoAnalysis.framework/photoanalysisd"],
    ["coreaudiod", "_coreaudiod", 0.02, 0, 40, "/usr/sbin/coreaudiod"],
  ];
  const filler = ["launchd", "syslogd", "cfprefsd", "trustd", "distnoted", "UserEventAgent", "secd", "bird", "cloudd", "nsurlsessiond", "sharingd", "rapportd", "powerd", "logd", "notifyd", "airportd", "bluetoothd", "corespotlightd", "fseventsd", "ReportCrash", "zsh", "login", "sshd", "ssh-agent", "gpg-agent", "git", "fsmonitor--daemon", "rg", "python3", "tccd"];

  const procs = [];
  let pid = 401;
  base.forEach(([name, user, cpuW, gpuW, mem, cmd]) => procs.push({ pid: pid += 7 + Math.floor(rnd() * 400), name, user, cpuW, gpuW, mem, cmd }));
  for (let i = 0; i < 110; i++) {
    const n = pick(filler);
    procs.push({ pid: pid += 3 + Math.floor(rnd() * 90), name: n, user: pick(["gabriel", "root", "_spotlight", "_networkd"]), cpuW: rnd() * 0.02, gpuW: 0, mem: 4 + rnd() * 90, cmd: "/usr/libexec/" + n });
  }
  procs.forEach((p) => {
    p.cpu = 0; p.gpu = 0; p.rss = p.mem; p.read = 0; p.write = 0;
    p.ports = { node: [1420], postgres: [5432], Docker: [5432, 6379], dotnet: [5050], codex: [] }[p.name] || [];
    p.start = Date.now() - Math.floor(rnd() * 86400000 * 3);
  });

  const cores = 14;
  const state = { procs, cores: Array(cores).fill(0), memTotal: 36, memUsed: 0, swap: 0, gpuUtil: 0, gpuMem: 0, temps: { CPU: 0, GPU: 0, SSD: 0, Battery: 0 }, net: { rx: 0, tx: 0 }, disk: { used: 0, total: 994 }, uptime: 3 * 86400 + 4 * 3600 + 1260, load: [0, 0, 0], tick: 0 };

  function tick() {
    state.tick++;
    const phase = (Math.sin(state.tick / 9) + 1) / 2;
    let cpuSum = 0, gpuSum = 0, mem = 0;
    procs.forEach((p) => {
      const burst = p.cpuW > 0.3 ? 0.4 + phase * 0.6 : 1;
      p.cpu = Math.max(0, p.cpuW * 100 * cores * 0.55 * burst * (0.7 + rnd() * 0.6) / (p.name === "rustc" || p.name === "cargo" ? 1 : 2));
      p.gpu = Math.max(0, p.gpuW * 22 * (0.6 + rnd() * 0.8));
      p.rss = p.mem * (0.95 + rnd() * 0.1);
      p.read = p.cpuW > 0.2 ? rnd() * 40 : rnd() * 0.3;
      p.write = p.cpuW > 0.2 ? rnd() * 25 : rnd() * 0.2;
      cpuSum += p.cpu; gpuSum += p.gpu; mem += p.rss;
    });
    state.cores = state.cores.map((_, i) => Math.min(100, (i < 10 ? 55 + phase * 40 : 10 + phase * 20) * (0.8 + rnd() * 0.4)));
    state.memUsed = Math.min(state.memTotal - 1, mem / 1024 + 9);
    state.swap = 2.1 + phase * 0.6;
    state.gpuUtil = Math.min(100, gpuSum);
    state.gpuMem = 3.4 + phase * 1.2;
    state.temps = { CPU: 62 + phase * 30, GPU: 48 + state.gpuUtil * 0.25, SSD: 41 + phase * 4, Battery: 31.5 };
    state.net = { rx: 1.2 + rnd() * 6, tx: 0.2 + rnd() * 1.4 };
    state.disk.used = 612.4;
    state.load = [6.8 + phase * 4, 7.9, 6.1];
    state.uptime += 1.5;
    state.self = { cpu: 0.18 + rnd() * 0.12, mem: 41 + rnd() * 3, tickMs: 1.1 + rnd() * 0.5, frameMs: 1.6 + rnd() * 0.9, fps: 120 };
    state.cpuTotal = state.cores.reduce((a, b) => a + b, 0) / cores;
    // Keep the processes' sum consistent with the machine total.
    const k = (state.cpuTotal * cores * 0.92) / Math.max(1, cpuSum);
    procs.forEach((p) => { p.cpu *= k; });
  }
  tick();

  // History: one bucket per minute for 24 h. gtop was closed for 4.5 hours overnight, so those buckets are null.
  const now = new Date();
  const minutes = 24 * 60;
  const history = [];
  const heavy = ["rustc", "cargo", "dotnet", "Docker", "Google Chrome Helper (GPU)", "WindowServer", "node", "clang", "Code Helper (Renderer)", "photoanalysisd", "mds_stores", "kernel_task", "Spotify", "Slack Helper"];
  for (let m = minutes - 1; m >= 0; m--) {
    const t = new Date(now.getTime() - m * 60000);
    // Demo day shape is anchored to a 14:30 "now" so the recent hour always has activity; labels keep the real clock.
    const h = (((14.5 - m / 60) % 24) + 24) % 24;
    const closed = h >= 3.17 && h < 7.67;
    if (closed) { history.push({ t, closed: true }); continue; }
    const work = h >= 9 && h < 19 ? 1 : h >= 20 && h < 23.5 ? 0.6 : 0.15;
    const build = (Math.sin(m / 23) > 0.55 ? 1 : 0) * work;
    const per = {};
    heavy.forEach((n) => {
      let c = 0, g = 0;
      if (n === "rustc") c = build * (300 + rnd() * 500);
      else if (n === "cargo") c = build * (20 + rnd() * 40);
      else if (n === "clang") c = build * rnd() * 90;
      else if (n === "dotnet") c = (Math.sin(m / 61) > 0.7 ? 1 : 0) * work * (150 + rnd() * 200);
      else if (n === "Docker") c = work * (20 + rnd() * 40);
      else if (n === "Google Chrome Helper (GPU)") { c = work * (8 + rnd() * 20); g = work * (6 + rnd() * 18); }
      else if (n === "WindowServer") { c = 6 + rnd() * 14; g = 4 + rnd() * 10; }
      else if (n === "node") c = work * (4 + rnd() * 30);
      else if (n === "Code Helper (Renderer)") c = work * (5 + rnd() * 25);
      else if (n === "photoanalysisd") { c = (1 - work) * (40 + rnd() * 60); g = (1 - work) * (10 + rnd() * 30); }
      else if (n === "mds_stores") c = build * rnd() * 60 + rnd() * 6;
      else if (n === "kernel_task") c = 5 + build * 40 * rnd();
      else c = rnd() * 4;
      per[n] = { cpu: c, gpu: g, mem: (procs.find((p) => p.name === n) || { mem: 100 }).mem * (0.8 + rnd() * 0.4) };
    });
    const cpu = Object.values(per).reduce((a, b) => a + b.cpu, 0) / cores;
    const gpu = Object.values(per).reduce((a, b) => a + b.gpu, 0);
    const mem = Math.min(35.5, 14 + Object.values(per).reduce((a, b) => a + b.mem, 0) / 1024 + work * 6 + rnd());
    history.push({ t, closed: false, per, cpu: Math.min(100, cpu), mem, gpu: Math.min(100, gpu), temp: 45 + Math.min(50, cpu * 0.6) });
  }

  function window_(hours) {
    const slice = history.slice(-hours * 60);
    const agg = {};
    let covered = 0;
    slice.forEach((b) => {
      if (b.closed) return;
      covered++;
      Object.entries(b.per).forEach(([n, v]) => {
        const a = (agg[n] ||= { name: n, cpuMin: 0, gpuMin: 0, peakMem: 0, peakCpu: 0, series: [] });
        a.cpuMin += v.cpu / 100;
        a.gpuMin += v.gpu / 100;
        a.peakMem = Math.max(a.peakMem, v.mem);
        a.peakCpu = Math.max(a.peakCpu, v.cpu);
      });
    });
    const rows = Object.values(agg).sort((a, b) => b.cpuMin - a.cpuMin);
    const total = rows.reduce((s, r) => s + r.cpuMin, 0);
    return { slice, rows, total, covered, minutes: hours * 60 };
  }

  window.GTOP = { state, tick, history, window: window_ };
})();
