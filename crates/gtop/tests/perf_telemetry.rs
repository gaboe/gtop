use gtop::perf::{PerfCounters, PerfRecorder, WindowState};
use rusqlite::Connection;
use std::{sync::Arc, time::Duration};

#[test]
fn recorder_writes_after_one_minute_of_injected_time_in_scratch_home() {
    let home = std::env::temp_dir().join(format!("gtop-perf-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let path = home.join("Library/Application Support/gtop/perf.sqlite");
    let counters = Arc::new(PerfCounters::default());
    let mut recorder = PerfRecorder::open(&path, counters.clone()).unwrap();
    counters.add_window_time(WindowState::Hidden, Duration::from_secs(59));
    counters.add_process_cpu(WindowState::Hidden, Duration::from_millis(250));
    assert!(!recorder
        .record_if_due(Duration::from_secs(59), 2_000_000_000)
        .unwrap());
    counters.add_window_time(WindowState::Hidden, Duration::from_secs(1));
    assert!(recorder
        .record_if_due(Duration::from_secs(60), 2_000_000_060)
        .unwrap());
    drop(recorder);
    let connection = Connection::open(&path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM perf_minute", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let (hidden_seconds, hidden_cpu): (f64, f64) = connection
        .query_row(
            "SELECT hidden_seconds, cpu_hidden_seconds FROM perf_minute",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(hidden_seconds, 60.);
    assert_eq!(hidden_cpu, 0.25);
    drop(connection);
    let _ = std::fs::remove_dir_all(home);
}
