pub mod monitoring;
mod state;

pub use monitoring::{ProcessInfo, SystemStats};
pub use state::AppState;

#[derive(Debug)]
pub struct Snapshot {
    pub processes: Vec<ProcessInfo>,
    pub stats: SystemStats,
}

pub fn perf_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("NEOHTOP_PERF").is_some())
}
