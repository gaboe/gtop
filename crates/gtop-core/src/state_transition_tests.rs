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
        state
            .snapshot(false, false)
            .unwrap()
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
fn snapshots_share_cpu_and_disk_history_across_collector_transitions() {
    const BYTES: usize = 8 * 1024 * 1024;
    let mut child = OwnedChild::new();
    child.work(BYTES, 100);
    let state = AppState::new();
    for (failed, busy) in [
        (false, true),
        (true, true),
        (false, false),
        (true, true),
        (true, false),
        (false, false),
    ] {
        child.interval(BYTES, if busy { 200 } else { 0 });
        let row = child.row(&state, failed);
        assert_eq!(row.disk_usage.1, BYTES as u64, "fallback={failed}");
        assert!(row.cpu_usage.is_finite());
        if busy {
            assert!(
                row.cpu_usage > 1.,
                "busy CPU={} fallback={failed}",
                row.cpu_usage
            );
        } else {
            assert!(
                row.cpu_usage < 2.,
                "idle CPU={} fallback={failed}",
                row.cpu_usage
            );
        }
    }
    child.interval(0, 0);
    assert_eq!(child.row(&state, false).disk_usage.1, 0);
    child.child.kill().unwrap();
    child.child.wait().unwrap();
    for failed in [true, false] {
        if failed {
            state
                .process_monitor
                .lock()
                .unwrap()
                .fail_next_native_collection();
        }
        assert!(!state
            .snapshot(false, false)
            .unwrap()
            .processes
            .iter()
            .any(|row| row.pid == child.child.id()));
        assert!(state.sys.lock().unwrap().processes().is_empty());
    }
}

#[test]
fn failed_startup_primes_fallback_and_preserves_new_process_writes() {
    const BYTES: usize = 8 * 1024 * 1024;
    let mut old = OwnedChild::new();
    old.work(BYTES, 0);
    let mut monitor = ProcessMonitor::new();
    monitor.fail_next_native_collection();
    let state = AppState::with_process_monitor(monitor);
    assert_eq!(old.row(&state, true).disk_usage.1, 0);
    assert_eq!(old.row(&state, false).disk_usage.1, 0);
    for failed in [true, false] {
        let mut new = OwnedChild::new();
        new.work(BYTES, 0);
        assert_eq!(new.row(&state, failed).disk_usage.1, BYTES as u64);
        assert_eq!(new.row(&state, !failed).disk_usage.1, 0);
    }
}
