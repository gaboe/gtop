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
        let mut static_errors = 0;
        let mut status_errors = 0;
        if std::env::args().any(|arg| arg == "--dump") {
            eprintln!("tick={tick} before={before:#?} native={native:#?} after={after:#?}");
        }
        let mut memory_outside_bracket = 0;
        for (pid, a) in &before {
            let Some(b) = after.get(pid).filter(|b| b.start_time == a.start_time) else {
                continue;
            };
            let Some(n) = native.get(pid) else {
                eprintln!("missing tick={tick} pid={pid} before={a:?} after={b:?}");
                missing += 1;
                continue;
            };
            let fields = |p: &gtop_core::ProcessInfo| {
                format!(
                    "{:?}",
                    (
                        p.ppid,
                        &p.name,
                        &p.user,
                        &p.command,
                        &p.executable,
                        &p.argv,
                        &p.root,
                        p.start_time,
                        p.threads,
                        p.session_id,
                        &p.environ,
                        &p.ports
                    )
                )
            };
            if fields(a) == fields(b) && fields(n) != fields(a) {
                static_errors += 1;
                eprintln!("static tick={tick} pid={pid} before={a:?} native={n:?} after={b:?}");
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
        println!("{{\"tick\":{tick},\"before\":{},\"native\":{},\"after\":{},\"missing\":{missing},\"static_errors\":{static_errors},\"status_errors\":{status_errors},\"memory_outside_bracket\":{memory_outside_bracket}}}", before.len(),native.len(),after.len());
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Native process parity requires macOS");
    std::process::exit(2);
}
