use gtop_core::AppState;
use std::time::{Duration, Instant};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let ticks: usize = args
        .first()
        .map(|v| v.parse().expect("ticks"))
        .unwrap_or(60);
    let interval: f64 = args
        .get(1)
        .map(|v| v.parse().expect("seconds"))
        .unwrap_or(3.);
    assert!(ticks > 0 && interval.is_finite() && interval > 0.);
    let interval = Duration::from_secs_f64(interval);
    let state = AppState::new();
    let mut deadline = Instant::now() + interval;
    for tick in 0..ticks + 2 {
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        let started = Instant::now();
        let result = state.snapshot(false, false);
        let elapsed = started.elapsed();
        deadline = started + interval;
        let snapshot = result.expect("snapshot failed");
        assert!(snapshot
            .processes
            .iter()
            .any(|p| p.pid == std::process::id()));
        assert!(snapshot.stats.memory_total > 0 && !snapshot.stats.cpu_usage.is_empty());
        if tick >= 2 {
            println!(
                "{{\"tick\":{},\"elapsed_us\":{},\"processes\":{},\"errors\":0}}",
                tick - 2,
                elapsed.as_micros(),
                snapshot.processes.len()
            );
        }
    }
}
