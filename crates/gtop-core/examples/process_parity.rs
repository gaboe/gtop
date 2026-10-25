#[cfg(target_os = "macos")]
use gtop_core::monitoring::ProcessMonitor;
#[cfg(target_os = "macos")]
use std::{collections::HashMap, time::Duration};
#[cfg(target_os = "macos")]
use sysinfo::System;

#[cfg(target_os = "macos")]
fn main() {
    let mut monitor = ProcessMonitor::new();
    let mut sys = System::new();
    let ports = HashMap::new();
    let mut failed = false;
    for tick in 0..4 {
        ProcessMonitor::refresh(&mut sys, false);
        let before = monitor.collect_processes(&sys, false, &ports).unwrap();
        let native = monitor.collect_native_processes(false, &ports).unwrap();
        ProcessMonitor::refresh(&mut sys, false);
        let after = monitor.collect_processes(&sys, false, &ports).unwrap();
        let before: HashMap<_, _> = before.into_iter().map(|p| (p.pid, p)).collect();
        let after: HashMap<_, _> = after.into_iter().map(|p| (p.pid, p)).collect();
        let native: HashMap<_, _> = native.into_iter().map(|p| (p.pid, p)).collect();
        let mut missing = 0;
        let mut identity_unavailable = 0;
        let mut static_errors = 0;
        let mut static_compared = 0;
        let mut static_skipped = 0;
        let mut status_errors = 0;
        if std::env::args().any(|arg| arg == "--dump") {
            eprintln!("tick={tick} before={before:#?} native={native:#?} after={after:#?}");
        }
        let mut memory_outside_bracket = 0;
        for (pid, a) in &before {
            let Some(b) = after.get(pid).filter(|b| b.start_time == a.start_time) else {
                continue;
            };
            if a.start_time == 0 {
                identity_unavailable += 1;
                continue;
            }
            let Some(n) = native.get(pid) else {
                eprintln!("missing tick={tick} pid={pid} before={a:?} after={b:?}");
                missing += 1;
                continue;
            };
            let fields = [
                ("ppid", a.ppid == b.ppid, n.ppid == a.ppid),
                ("name", a.name == b.name, n.name == a.name),
                ("user", a.user == b.user, n.user == a.user),
                ("command", a.command == b.command, n.command == a.command),
                (
                    "executable",
                    a.executable == b.executable,
                    n.executable == a.executable,
                ),
                ("argv", a.argv == b.argv, n.argv == a.argv),
                ("root", a.root == b.root, n.root == a.root),
                (
                    "start_time",
                    a.start_time == b.start_time,
                    n.start_time == a.start_time,
                ),
                ("threads", a.threads == b.threads, n.threads == a.threads),
                (
                    "session_id",
                    a.session_id == b.session_id,
                    n.session_id == a.session_id,
                ),
                ("environ", a.environ == b.environ, n.environ == a.environ),
                ("ports", a.ports == b.ports, n.ports == a.ports),
            ];
            for (field, stable, matches) in fields {
                if stable {
                    static_compared += 1;
                    if !matches {
                        static_errors += 1;
                        eprintln!(
                            "static tick={tick} pid={pid} field={field} before={a:?} native={n:?} after={b:?}"
                        );
                    }
                } else {
                    static_skipped += 1;
                }
            }
            if tick > 0 && a.status == b.status && n.status != a.status {
                status_errors += 1;
                eprintln!(
                    "status tick={tick} pid={pid} before={} native={} after={}",
                    a.status, n.status, b.status
                );
            }
            if n.memory_usage < a.memory_usage.min(b.memory_usage)
                || n.memory_usage > a.memory_usage.max(b.memory_usage)
            {
                memory_outside_bracket += 1;
            }
        }
        failed |= missing > 0 || static_errors > 0 || status_errors > 0;
        println!("{{\"tick\":{tick},\"before\":{},\"native\":{},\"after\":{},\"missing\":{missing},\"identity_unavailable\":{identity_unavailable},\"static_errors\":{static_errors},\"static_compared\":{static_compared},\"static_skipped\":{static_skipped},\"status_errors\":{status_errors},\"memory_outside_bracket\":{memory_outside_bracket}}}", before.len(),native.len(),after.len());
        std::thread::sleep(Duration::from_secs(1));
    }
    if failed {
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Native process parity requires macOS");
    std::process::exit(2);
}
