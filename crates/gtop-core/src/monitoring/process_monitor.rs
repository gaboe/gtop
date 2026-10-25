//! Process monitoring functionality
//!
//! This module handles monitoring and managing system processes, including
//! collecting process information and managing process lifecycle.

use super::{GpuProcessMonitor, ProcessInfo};
use std::collections::HashMap;
use std::ffi::OsString;
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

fn os_string_vec_to_string_vec(v: &[OsString]) -> Vec<String> {
    v.iter()
        .map(|c| c.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
}

#[cfg(target_os = "macos")]
pub(super) fn macos_environment(pid: u32) -> Result<Vec<String>, String> {
    unsafe extern "C" {
        fn sysctl(
            name: *mut i32,
            count: u32,
            old: *mut std::ffi::c_void,
            size: *mut usize,
            new: *mut std::ffi::c_void,
            new_size: usize,
        ) -> i32;
    }
    let mut mib = [1, 49, pid as i32];
    let mut size = 0usize;
    let read = |buffer: *mut std::ffi::c_void, size: &mut usize, mib: &mut [i32; 3]| unsafe {
        sysctl(mib.as_mut_ptr(), 3, buffer, size, std::ptr::null_mut(), 0)
    };
    let error = || {
        format!(
            "macOS could not read this process environment: {}",
            std::io::Error::last_os_error()
        )
    };
    if read(std::ptr::null_mut(), &mut size, &mut mib) != 0 {
        return Err(error());
    }
    let mut data = vec![0u8; size];
    if read(data.as_mut_ptr().cast(), &mut size, &mut mib) != 0 {
        return Err(error());
    }
    data.truncate(size);
    parse_macos_environment(&data)
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_environment(data: &[u8]) -> Result<Vec<String>, String> {
    let argc = data
        .get(..4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(i32::from_ne_bytes)
        .filter(|argc| *argc >= 0)
        .ok_or("macOS returned incomplete process arguments")?;
    let mut arguments = &data[4..];
    let executable_end = arguments
        .iter()
        .position(|byte| *byte == 0)
        .ok_or("macOS returned incomplete process arguments")?;
    // XNU pads the executable path to the target pointer size before argv.
    // All supported macOS processes are 64-bit. Empty argv entries follow it.
    let argv_start = (executable_end + 1).div_ceil(8) * 8;
    arguments = arguments
        .get(argv_start..)
        .ok_or("macOS returned incomplete process arguments")?;
    for _ in 0..argc {
        let end = arguments
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("macOS returned incomplete process arguments")?;
        arguments = &arguments[end + 1..];
    }
    if arguments.last().is_some_and(|byte| *byte != 0) {
        return Err("macOS returned incomplete process environment".into());
    }
    let fields = arguments
        .split(|byte| *byte == 0)
        .skip_while(|field| field.is_empty());
    Ok(fields
        .take_while(|field| field.contains(&b'='))
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect())
}

/// Monitors and manages system processes
#[derive(Debug)]
pub struct ProcessMonitor {
    /// Tracks per-process GPU engine usage between refreshes
    gpu_process_monitor: GpuProcessMonitor,
    users: sysinfo::Users,
    #[cfg(target_os = "macos")]
    native: super::native_processes::Collector,
    #[cfg(all(test, target_os = "macos"))]
    fail_next_native_collection: bool,
}

impl Default for ProcessMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessMonitor {
    /// Creates a new process monitor instance
    pub fn new() -> Self {
        Self {
            gpu_process_monitor: GpuProcessMonitor::new(),
            users: sysinfo::Users::new_with_refreshed_list(),
            #[cfg(target_os = "macos")]
            native: super::native_processes::Collector::default(),
            #[cfg(all(test, target_os = "macos"))]
            fail_next_native_collection: false,
        }
    }

    /// Refreshes the process list. Static fields (user, command, root) are read
    /// once per process; environment variables only when the caller shows them.
    pub fn refresh(sys: &mut System, include_environ: bool) {
        let mut kind = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_disk_usage()
            .with_user(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_root(UpdateKind::OnlyIfNotSet);
        if include_environ {
            kind = kind.with_environ(UpdateKind::OnlyIfNotSet);
        }
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    }

    /// Collects information about all running processes
    ///
    /// # Arguments
    ///
    /// * `sys` - System information provider
    /// * `include_environ` - Whether to include environment variables
    /// * `ports_by_pid` - Local bound ports keyed by PID (empty to skip)
    ///
    /// # Returns
    ///
    /// A vector of process information, or an error string if collection failed
    pub fn collect_processes(
        &mut self,
        sys: &System,
        include_environ: bool,
        ports_by_pid: &HashMap<u32, Vec<u16>>,
    ) -> Result<Vec<ProcessInfo>, String> {
        let current_time = Self::get_current_time()?;
        let gpu_usage = self.gpu_process_monitor.collect();
        // Processes that hold no GPU client are idle rather than unknown, so
        // they report zero wherever the platform supports the accounting
        let gpu_supported = self.gpu_process_monitor.is_supported();
        Ok(sys
            .processes()
            .iter()
            .map(|(pid, process)| {
                process_row(
                    pid.as_u32(),
                    process,
                    &self.users,
                    include_environ,
                    ports_by_pid,
                    gpu_supported.then(|| gpu_usage.get(&pid.as_u32()).copied().unwrap_or(0.0)),
                    current_time,
                )
            })
            .collect())
    }

    #[cfg(target_os = "macos")]
    pub fn collect_native_processes(
        &mut self,
        include_environ: bool,
        ports: &HashMap<u32, Vec<u16>>,
    ) -> Result<Vec<ProcessInfo>, String> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_native_collection) {
            return Err("forced native collection failure".into());
        }
        let mut rows = self.native.collect(&self.users, include_environ, ports)?;
        let gpu = self.gpu_process_monitor.collect();
        let supported = self.gpu_process_monitor.is_supported();
        for row in &mut rows {
            row.gpu_usage = supported.then(|| gpu.get(&row.pid).copied().unwrap_or(0.0));
        }
        Ok(rows)
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn fail_next_native_collection(&mut self) {
        self.fail_next_native_collection = true;
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn fail_next_counter_reads(&mut self, pid: u32) {
        self.native.fail_next_counter_reads(pid);
    }

    /// Reads the environment variables of one process
    pub fn process_environ(sys: &mut System, pid: u32) -> Vec<String> {
        let pid = Pid::from_u32(pid);
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
        );
        sys.process(pid)
            .map(|process| os_string_vec_to_string_vec(process.environ()))
            .unwrap_or_default()
    }

    pub fn read_environment(sys: &mut System, pid: u32) -> Result<Vec<String>, String> {
        #[cfg(target_os = "macos")]
        {
            let _ = sys;
            macos_environment(pid)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Self::process_environ(sys, pid))
        }
    }

    /// Attempts to kill a process
    ///
    /// # Arguments
    ///
    /// * `sys` - System information provider
    /// * `pid` - Process ID to kill
    ///
    /// # Returns
    ///
    /// Boolean indicating whether the process was successfully killed
    pub fn kill_process(sys: &System, pid: u32) -> bool {
        sys.process(Pid::from(pid as usize))
            .map(|process| process.kill())
            .unwrap_or(false)
    }

    /// Gets the current system time in seconds since UNIX epoch
    pub(super) fn get_current_time() -> Result<u64, String> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|e| format!("Failed to get system time: {}", e))
    }

    /// Formats process status into a human-readable string
    pub fn format_status(status: ProcessStatus) -> String {
        match status {
            ProcessStatus::Run => "Running",
            ProcessStatus::Sleep => "Sleeping",
            ProcessStatus::Idle => "Idle",
            ProcessStatus::Zombie => "Zombie",
            _ => "Unknown",
        }
        .to_string()
    }
}

pub(super) fn process_row(
    pid: u32,
    process: &sysinfo::Process,
    users: &sysinfo::Users,
    include_environ: bool,
    ports_by_pid: &HashMap<u32, Vec<u16>>,
    gpu_usage: Option<f32>,
    current_time: u64,
) -> ProcessInfo {
    let start_time = process.start_time();
    let disk_usage = process.disk_usage();
    ProcessInfo {
        pid,
        ppid: process.parent().map(|p| p.as_u32()).unwrap_or(0),
        name: process.name().to_string_lossy().into_owned(),
        cpu_usage: process.cpu_usage(),
        gpu_usage,
        memory_usage: process.memory(),
        status: ProcessMonitor::format_status(process.status()),
        user: process
            .user_id()
            .map(|uid| {
                users
                    .get_user_by_id(uid)
                    .map(|user| user.name().to_string())
                    .unwrap_or_else(|| uid.to_string())
            })
            .unwrap_or_else(|| "-".to_string()),
        command: os_string_vec_to_string_vec(process.cmd()).join(" "),
        executable: process
            .exe()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        argv: os_string_vec_to_string_vec(process.cmd()),
        threads: None,
        environ: if include_environ {
            os_string_vec_to_string_vec(process.environ())
        } else {
            Vec::new()
        },
        root: process
            .root()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        virtual_memory: process.virtual_memory(),
        start_time,
        run_time: if start_time > 0 {
            current_time.saturating_sub(start_time)
        } else {
            0
        },
        disk_usage: (disk_usage.read_bytes, disk_usage.written_bytes),
        disk_sample_seconds: None,
        session_id: process.session_id().map(|id| id.as_u32()),
        ports: ports_by_pid.get(&pid).cloned().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_parser_keeps_empty_first_argument_and_rejects_truncation() {
        let mut data = 2i32.to_ne_bytes().to_vec();
        data.extend_from_slice(b"/bin/foo\0");
        data.resize(4 + 16, 0);
        data.extend_from_slice(b"\0tail\0MARKER=value\0OTHER=two\0");
        assert_eq!(
            parse_macos_environment(&data).unwrap(),
            ["MARKER=value", "OTHER=two"]
        );
        data.pop();
        assert!(parse_macos_environment(&data).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn owned_child_with_empty_argv0_keeps_first_environment_field() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("/bin/sleep")
            .arg0("")
            .arg("30")
            .env_clear()
            .env("GTOP_FIRST_MARKER", "retained")
            .spawn()
            .unwrap();
        let result = macos_environment(child.id());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(result
            .unwrap()
            .iter()
            .any(|value| value == "GTOP_FIRST_MARKER=retained"));
    }

    #[test]
    fn environment_parser_counts_empty_arguments() {
        let mut data = 3i32.to_ne_bytes().to_vec();
        data.extend_from_slice(b"/bin/proc\0");
        data.resize(4 + 16, 0);
        data.extend_from_slice(b"/bin/proc\0\0tail\0PATH=/bin\0GTOP_ENV_MARKER=unit\0\0");
        assert_eq!(
            parse_macos_environment(&data).unwrap(),
            vec!["PATH=/bin", "GTOP_ENV_MARKER=unit"]
        );
        assert!(parse_macos_environment(&[]).is_err());
    }

    #[test]
    fn collects_static_fields_but_environ_only_on_request() {
        let mut sys = System::new();
        ProcessMonitor::refresh(&mut sys, false);
        let own_pid = std::process::id();

        let processes = ProcessMonitor::new()
            .collect_processes(&sys, false, &HashMap::new())
            .unwrap();
        let own = processes.iter().find(|p| p.pid == own_pid).unwrap();
        assert!(!own.command.is_empty(), "command is read on first refresh");
        assert!(own.environ.is_empty(), "environ is skipped by default");
        assert!(
            !own.executable.is_empty(),
            "executable is read on first refresh"
        );
        assert_eq!(own.argv.join(" "), own.command);

        assert!(
            !ProcessMonitor::process_environ(&mut sys, own_pid).is_empty(),
            "environ is read on demand"
        );
    }
}
