use gtop_core::{ProcessInfo, Snapshot, SystemStats};
pub fn process(pid: u32, name: &str, cpu: f32) -> ProcessInfo {
    ProcessInfo {
        pid,
        ppid: 0,
        name: name.into(),
        cpu_usage: cpu,
        gpu_usage: Some(cpu / 2.),
        memory_usage: pid as u64 * 1024,
        status: "Running".into(),
        user: "test".into(),
        executable: format!("/bin/{name}"),
        argv: vec![format!("/bin/{name}")],
        command: format!("/bin/{name}"),
        threads: None,
        environ: vec![],
        root: "/".into(),
        virtual_memory: 0,
        start_time: pid as u64,
        run_time: 0,
        disk_usage: (0, 0),
        disk_sample_seconds: None,
        session_id: None,
        ports: vec![],
    }
}
pub fn snapshot() -> Snapshot {
    Snapshot {
        process_data: gtop_core::ProcessData::Fresh,
        processes: vec![
            process(1, "rustc", 200.),
            process(2, "rustc", 150.),
            process(3, "cargo", 110.),
            process(4, "quiet", 50.),
        ],
        stats: SystemStats {
            cpu_usage: vec![50.; 4],
            memory_total: 1000,
            memory_used: 500,
            memory_free: 500,
            memory_cached: 0,
            swap_used: 0,
            uptime: 0,
            load_avg: [0.; 3],
            network_rx_bytes: 0,
            network_tx_bytes: 0,
            disk_total_bytes: 0,
            disk_used_bytes: 0,
            disk_free_bytes: 0,
            gpus: vec![],
            temperatures: vec![gtop_core::monitoring::Temperature {
                name: "CPU".into(),
                celsius: 80.,
            }],
        },
        cpu_topology: gtop_core::CpuTopology::default(),
    }
}
