use gtop_core::AppState;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
fn cpu_micros() -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct Usage {
        user_seconds: i64,
        user_micros: i32,
        user_padding: i32,
        system_seconds: i64,
        system_micros: i32,
        system_padding: i32,
        counters: [i64; 14],
    }
    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut Usage) -> i32;
    }
    let mut usage = Usage::default();
    assert_eq!(unsafe { getrusage(0, &mut usage) }, 0, "getrusage failed");
    (usage.user_seconds + usage.system_seconds) as u64 * 1_000_000
        + (usage.user_micros + usage.system_micros) as u64
}

#[cfg(not(target_os = "macos"))]
fn cpu_micros() -> u64 {
    panic!("CPU-time tick measurement currently requires macOS")
}

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
        let cpu_started = cpu_micros();
        let started = Instant::now();
        let result = state.snapshot(false, false);
        let elapsed = started.elapsed();
        let cpu_elapsed = cpu_micros()
            .checked_sub(cpu_started)
            .expect("CPU clock moved backwards");
        deadline = started + interval;
        let snapshot = result.expect("snapshot failed");
        assert!(snapshot
            .processes
            .iter()
            .any(|p| p.pid == std::process::id()));
        assert!(snapshot.stats.memory_total > 0 && !snapshot.stats.cpu_usage.is_empty());
        if tick >= 2 {
            println!(
                "{{\"tick\":{},\"elapsed_us\":{},\"cpu_us\":{},\"processes\":{},\"errors\":0}}",
                tick - 2,
                elapsed.as_micros(),
                cpu_elapsed,
                snapshot.processes.len()
            );
        }
    }
}
