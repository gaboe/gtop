use super::*;
use crate::ProcessInfo;
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::Duration,
};

struct OwnedChild {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    file: PathBuf,
}

impl OwnedChild {
    fn new() -> Self {
        let script = r#"import os, sys, time
f = open(sys.argv[1] + str(os.getpid()), 'wb', buffering=0)
print('ready', flush=True)
for line in sys.stdin:
    size, busy = map(int, line.split())
    f.write(b'x' * size)
    os.fsync(f.fileno())
    end = time.process_time() + busy / 1000
    while time.process_time() < end:
        pass
    print('done', flush=True)
"#;
        let prefix = std::env::temp_dir().join("gtop-transition-");
        let mut child = Command::new("/usr/bin/python3")
            .args(["-c", script, prefix.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        output.read_line(&mut ready).unwrap();
        assert_eq!(ready.trim(), "ready");
        let file = PathBuf::from(format!("{}{}", prefix.display(), child.id()));
        Self {
            child,
            input,
            output,
            file,
        }
    }

    fn work(&mut self, bytes: usize, cpu_ms: usize) {
        writeln!(self.input, "{bytes} {cpu_ms}").unwrap();
        self.input.flush().unwrap();
        let mut done = String::new();
        self.output.read_line(&mut done).unwrap();
        assert_eq!(done.trim(), "done");
    }

    fn interval(&mut self, bytes: usize, cpu_ms: usize) {
        let start = Instant::now();
        self.work(bytes, cpu_ms);
        std::thread::sleep(Duration::from_secs(1).saturating_sub(start.elapsed()));
    }

    fn row(&self, state: &AppState, failed: bool) -> ProcessInfo {
        if failed {
            state
                .process_monitor
                .lock()
                .unwrap()
                .fail_next_native_collection();
        }
        let snapshot = state.snapshot(false, false).unwrap();
        assert_eq!(
            matches!(snapshot.process_data, ProcessData::Stale { .. }),
            failed
        );
        snapshot
            .processes
            .into_iter()
            .find(|row| row.pid == self.child.id())
            .unwrap()
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.file);
    }
}

#[test]
fn snapshots_keep_stale_rows_and_recover_exact_counter_intervals() {
    const BYTES: usize = 8 * 1024 * 1024;
    let mut child = OwnedChild::new();
    child.work(BYTES, 100);
    let state = AppState::new();
    child.interval(BYTES, 200);
    let first = child.row(&state, false);
    assert_eq!(first.disk_usage.1, BYTES as u64);
    assert!(first.cpu_usage > 1.);
    child.interval(BYTES, 200);
    let stale = child.row(&state, true);
    assert_eq!(stale.disk_usage, first.disk_usage);
    assert_eq!(stale.cpu_usage, first.cpu_usage);
    child.interval(BYTES, 200);
    let recovered = child.row(&state, false);
    assert_eq!(recovered.disk_usage.1, 2 * BYTES as u64);
    assert!(recovered.cpu_usage > 1.);
    for failed in [true, true, false] {
        child.interval(0, 0);
        let row = child.row(&state, failed);
        if !failed {
            assert_eq!(row.disk_usage.1, 0);
            assert!(row.cpu_usage < 2.);
        }
    }
    child.child.kill().unwrap();
    child.child.wait().unwrap();
    assert!(!state
        .snapshot(false, false)
        .unwrap()
        .processes
        .iter()
        .any(|p| p.pid == child.child.id()));
    assert!(state.sys.lock().unwrap().processes().is_empty());
}

#[test]
fn failed_startup_primes_first_success_and_preserves_new_process_writes() {
    const BYTES: usize = 8 * 1024 * 1024;
    let mut old = OwnedChild::new();
    old.work(BYTES, 0);
    let mut monitor = ProcessMonitor::new();
    monitor.fail_next_native_collection();
    let state = AppState::with_process_monitor(monitor);
    state
        .process_monitor
        .lock()
        .unwrap()
        .fail_next_native_collection();
    let failed = state.snapshot(false, false).unwrap();
    assert!(matches!(failed.process_data, ProcessData::Stale { .. }));
    assert!(failed.processes.is_empty());
    assert!(failed.stats.memory_total > 0);
    assert_eq!(old.row(&state, false).disk_usage.1, 0);
    let mut new = OwnedChild::new();
    new.work(BYTES, 0);
    assert_eq!(new.row(&state, false).disk_usage.1, BYTES as u64);
    assert_eq!(new.row(&state, false).disk_usage.1, 0);
}

#[test]
fn unavailable_counters_preserve_last_valid_samples_and_startup_baselines() {
    const BYTES: usize = 8 * 1024 * 1024;
    let mut child = OwnedChild::new();
    child.work(BYTES, 500);
    let mut monitor = ProcessMonitor::new();
    monitor.fail_next_counter_reads(child.child.id());
    let state = AppState::with_process_monitor(monitor);
    let first = child.row(&state, false);
    assert_eq!(first.disk_usage.1, 0);
    assert_eq!(first.cpu_usage, 0.);
    child.interval(0, 0);
    let idle = child.row(&state, false);
    assert_eq!(idle.disk_usage.1, 0);
    assert!(idle.cpu_usage < 2.);
    child.interval(BYTES, 200);
    state
        .process_monitor
        .lock()
        .unwrap()
        .fail_next_counter_reads(child.child.id());
    let unavailable = child.row(&state, false);
    assert_eq!(unavailable.disk_usage, (0, 0));
    assert_eq!(unavailable.cpu_usage, 0.);
    assert_eq!(unavailable.memory_usage, 0);
    child.interval(BYTES, 0);
    let recovery = child.row(&state, false);
    assert_eq!(recovery.disk_usage.1, 2 * BYTES as u64);
    assert!(recovery.cpu_usage > 1.);
    child.interval(0, 0);
    let idle = child.row(&state, false);
    assert_eq!(idle.disk_usage.1, 0);
    assert!(idle.cpu_usage < 2.);
}
