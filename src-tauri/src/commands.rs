use gtop_core::{AppState, ProcessInfo, SystemStats};
use tauri::State;

#[tauri::command]
pub async fn get_processes(
    include_environ: bool,
    include_ports: bool,
    state: State<'_, AppState>,
) -> Result<(std::sync::Arc<Vec<ProcessInfo>>, SystemStats), String> {
    let snapshot = state.snapshot(include_environ, include_ports)?;
    Ok((snapshot.processes, snapshot.stats))
}

#[tauri::command]
pub async fn get_process_environ(
    pid: u32,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state.process_environ(pid)
}

#[tauri::command]
pub async fn kill_process(pid: u32, state: State<'_, AppState>) -> Result<bool, String> {
    state.kill_process(pid)
}
