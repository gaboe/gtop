use super::{process_monitor::process_row, ProcessInfo, ProcessMonitor};
use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    time::Instant,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const PROC_PIDTBSDINFO: i32 = 3;
const PROC_PIDTASKINFO: i32 = 4;
const PROC_PIDTHREADINFO: i32 = 5;
const PROC_PIDVNODEPATHINFO: i32 = 9;
const RUSAGE_INFO_V2: i32 = 2;

#[repr(C)]
#[derive(Default, Debug)]
struct proc_bsdinfo {
    pub pbi_flags: u32,
    pub pbi_status: u32,
    pub pbi_xstatus: u32,
    pub pbi_pid: u32,
    pub pbi_ppid: u32,
    pub pbi_uid: u32,
    pub pbi_gid: u32,
    pub pbi_ruid: u32,
    pub pbi_rgid: u32,
    pub pbi_svuid: u32,
    pub pbi_svgid: u32,
    pub rfu_1: u32,
    pub pbi_comm: [i8; 16],
    pub pbi_name: [i8; 32],
    pub pbi_nfiles: u32,
    pub pbi_pgid: u32,
    pub pbi_pjobc: u32,
    pub e_tdev: u32,
    pub e_tpgid: u32,
    pub pbi_nice: i32,
    pub pbi_start_tvsec: u64,
    pub pbi_start_tvusec: u64,
}
#[repr(C)]
#[derive(Default, Debug)]
struct proc_taskinfo {
    pub pti_virtual_size: u64,
    pub pti_resident_size: u64,
    pub pti_total_user: u64,
    pub pti_total_system: u64,
    pub pti_threads_user: u64,
    pub pti_threads_system: u64,
    pub pti_policy: i32,
    pub pti_faults: i32,
    pub pti_pageins: i32,
    pub pti_cow_faults: i32,
    pub pti_messages_sent: i32,
    pub pti_messages_received: i32,
    pub pti_syscalls_mach: i32,
    pub pti_syscalls_unix: i32,
    pub pti_csw: i32,
    pub pti_threadnum: i32,
    pub pti_numrunning: i32,
    pub pti_priority: i32,
}
#[repr(C)]
#[derive(Debug)]
struct proc_threadinfo {
    pub pth_user_time: u64,
    pub pth_system_time: u64,
    pub pth_cpu_usage: i32,
    pub pth_policy: i32,
    pub pth_run_state: i32,
    pub pth_flags: i32,
    pub pth_sleep_time: i32,
    pub pth_curpri: i32,
    pub pth_priority: i32,
    pub pth_maxpriority: i32,
    pub pth_name: [i8; 64],
}
#[repr(C)]
#[derive(Default, Debug)]
struct rusage_info_v2 {
    pub ri_uuid: [u8; 16],
    pub ri_user_time: u64,
    pub ri_system_time: u64,
    pub ri_pkg_idle_wkups: u64,
    pub ri_interrupt_wkups: u64,
    pub ri_pageins: u64,
    pub ri_wired_size: u64,
    pub ri_resident_size: u64,
    pub ri_phys_footprint: u64,
    pub ri_proc_start_abstime: u64,
    pub ri_proc_exit_abstime: u64,
    pub ri_child_user_time: u64,
    pub ri_child_system_time: u64,
    pub ri_child_pkg_idle_wkups: u64,
    pub ri_child_interrupt_wkups: u64,
    pub ri_child_pageins: u64,
    pub ri_child_elapsed_abstime: u64,
    pub ri_diskio_bytesread: u64,
    pub ri_diskio_byteswritten: u64,
}

#[repr(C)]
struct vinfo_stat {
    pub vst_dev: u32,
    pub vst_mode: u16,
    pub vst_nlink: u16,
    pub vst_ino: u64,
    pub vst_uid: u32,
    pub vst_gid: u32,
    pub vst_atime: i64,
    pub vst_atimensec: i64,
    pub vst_mtime: i64,
    pub vst_mtimensec: i64,
    pub vst_ctime: i64,
    pub vst_ctimensec: i64,
    pub vst_birthtime: i64,
    pub vst_birthtimensec: i64,
    pub vst_size: i64,
    pub vst_blocks: i64,
    pub vst_blksize: i32,
    pub vst_flags: u32,
    pub vst_gen: u32,
    pub vst_rdev: u32,
    pub vst_qspare: [i64; 2],
}
#[repr(C)]
struct vnode_info {
    pub vi_stat: vinfo_stat,
    pub vi_type: i32,
    pub vi_pad: i32,
    pub vi_fsid: [i32; 2],
}
#[repr(C)]
struct vnode_info_path {
    pub vip_vi: vnode_info,
    pub vip_path: [[u8; 32]; 32],
}
#[repr(C)]
struct proc_vnodepathinfo {
    pub pvi_cdir: vnode_info_path,
    pub pvi_rdir: vnode_info_path,
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_listallpids(buffer: *mut c_void, bytes: i32) -> i32;
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut c_void, bytes: i32) -> i32;
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut c_void) -> i32;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
    fn getsid(pid: i32) -> i32;
}
#[repr(C)]
#[derive(Default)]
struct Timebase {
    numer: u32,
    denom: u32,
}

fn info<T>(pid: u32, flavor: i32) -> Option<T> {
    let mut data = std::mem::MaybeUninit::<T>::zeroed();
    let size = std::mem::size_of::<T>() as i32;
    (unsafe { proc_pidinfo(pid as i32, flavor, 0, data.as_mut_ptr().cast(), size) } == size)
        .then(|| unsafe { data.assume_init() })
}

#[derive(Debug)]
struct Record {
    identity: (u64, u64),
    row: ProcessInfo,
    cpu: u64,
    disk: (u64, u64),
    sampled: Instant,
}
#[derive(Debug, Default)]
pub(super) struct Collector {
    pids: Vec<i32>,
    records: HashMap<u32, Record>,
}
impl Collector {
    pub fn collect(
        &mut self,
        users: &sysinfo::Users,
        include_environ: bool,
        ports: &HashMap<u32, Vec<u16>>,
    ) -> Result<Vec<ProcessInfo>, String> {
        loop {
            if self.pids.is_empty() {
                let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
                if count <= 0 {
                    return Err("macOS could not enumerate processes".into());
                }
                self.pids.resize(count as usize + 128, 0);
            }
            let count = unsafe {
                proc_listallpids(
                    self.pids.as_mut_ptr().cast(),
                    std::mem::size_of_val(self.pids.as_slice()) as i32,
                )
            };
            if count <= 0 {
                return Err("macOS could not enumerate processes".into());
            }
            if count as usize >= self.pids.len() {
                self.pids.resize(self.pids.len() * 2, 0);
                continue;
            }
            let mut live = HashSet::new();
            let mut metadata = Vec::new();
            let mut samples = Vec::new();
            for &pid in &self.pids[..count as usize] {
                if pid <= 0 {
                    continue;
                }
                let pid = pid as u32;
                live.insert(pid);
                let bsd = info::<proc_bsdinfo>(pid, PROC_PIDTBSDINFO);
                let identity = bsd
                    .as_ref()
                    .map(|b| (b.pbi_start_tvsec, b.pbi_start_tvusec));
                if identity.is_none() || self.records.get(&pid).map(|r| r.identity) != identity {
                    self.records.remove(&pid);
                    metadata.push(Pid::from_u32(pid));
                }
                if self.records.get(&pid).is_some_and(|r| {
                    r.row.command.is_empty() || r.row.name.is_empty() || r.row.executable.is_empty()
                }) {
                    metadata.push(Pid::from_u32(pid));
                }
                samples.push((pid, bsd));
            }
            self.records.retain(|pid, _| live.contains(pid));
            let mut sys = System::new();
            if !metadata.is_empty() {
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
                sys.refresh_processes_specifics(ProcessesToUpdate::Some(&metadata), true, kind);
            }
            let mut timebase = Timebase::default();
            if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
                return Err("macOS could not read the CPU timebase".into());
            }
            let nanos_per_unit = timebase.numer as f64 / timebase.denom as f64;
            let now = ProcessMonitor::get_current_time()?;
            let mut rows = Vec::with_capacity(samples.len());
            for (pid, bsd) in samples {
                let sampled = Instant::now();
                let task = info::<proc_taskinfo>(pid, PROC_PIDTASKINFO).unwrap_or_default();
                let cpu = task.pti_total_user.saturating_add(task.pti_total_system);
                let mut usage = rusage_info_v2::default();
                let disk = (unsafe {
                    proc_pid_rusage(
                        pid as i32,
                        RUSAGE_INFO_V2,
                        (&mut usage as *mut rusage_info_v2).cast(),
                    )
                } == 0)
                    .then_some((usage.ri_diskio_bytesread, usage.ri_diskio_byteswritten));
                let existing = self.records.contains_key(&pid);
                if !existing {
                    let Some(process) = sys.process(Pid::from_u32(pid)) else {
                        continue;
                    };
                    self.records.insert(
                        pid,
                        Record {
                            identity: bsd
                                .as_ref()
                                .map(|b| (b.pbi_start_tvsec, b.pbi_start_tvusec))
                                .unwrap_or_default(),
                            row: process_row(
                                pid,
                                process,
                                users,
                                include_environ,
                                ports,
                                None,
                                now,
                            ),
                            cpu,
                            disk: disk.unwrap_or_default(),
                            sampled,
                        },
                    );
                }
                let record = self.records.get_mut(&pid).unwrap();
                if record.row.command.is_empty()
                    || record.row.name.is_empty()
                    || record.row.executable.is_empty()
                {
                    if let Some(process) = sys.process(Pid::from_u32(pid)) {
                        if record.row.name.is_empty() {
                            record.row.name = process.name().to_string_lossy().into_owned();
                        }
                        record.row.argv = process
                            .cmd()
                            .iter()
                            .map(|s| s.to_string_lossy().into_owned())
                            .collect();
                        record.row.command = record.row.argv.join(" ");
                        record.row.executable = process
                            .exe()
                            .map(|path| path.to_string_lossy().into_owned())
                            .unwrap_or_default();
                    }
                }
                if record.row.root.is_empty() {
                    if let Some(vnodes) = info::<proc_vnodepathinfo>(pid, PROC_PIDVNODEPATHINFO) {
                        if vnodes.pvi_rdir.vip_vi.vi_stat.vst_dev != 0 {
                            let bytes = vnodes.pvi_rdir.vip_path.as_flattened();
                            let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
                            record.row.root = String::from_utf8_lossy(&bytes[..end]).into_owned();
                        }
                    }
                }
                let mut row = record.row.clone();
                if let Some(bsd) = bsd {
                    row.ppid = bsd.pbi_ppid;
                    row.start_time = bsd.pbi_start_tvsec;
                    let status = if bsd.pbi_status == 2 {
                        match info::<proc_threadinfo>(pid, PROC_PIDTHREADINFO)
                            .map(|t| t.pth_run_state)
                        {
                            Some(3) => sysinfo::ProcessStatus::Sleep,
                            Some(1) | None => sysinfo::ProcessStatus::Run,
                            _ => sysinfo::ProcessStatus::Unknown(0),
                        }
                    } else {
                        sysinfo::ProcessStatus::from(bsd.pbi_status)
                    };
                    row.status = ProcessMonitor::format_status(status);
                }
                row.cpu_usage = if existing && record.cpu > 0 {
                    (cpu.saturating_sub(record.cpu) as f64 * nanos_per_unit
                        / sampled.duration_since(record.sampled).as_nanos().max(1) as f64
                        * 100.) as f32
                } else {
                    0.
                };
                row.memory_usage = task.pti_resident_size;
                row.virtual_memory = task.pti_virtual_size;
                row.disk_usage = disk
                    .map(|d| {
                        (
                            d.0.saturating_sub(record.disk.0),
                            d.1.saturating_sub(record.disk.1),
                        )
                    })
                    .unwrap_or_default();
                row.run_time = if row.start_time > 0 {
                    now.saturating_sub(row.start_time)
                } else {
                    0
                };
                row.session_id = u32::try_from(unsafe { getsid(pid as i32) }).ok();
                row.ports = ports.get(&pid).cloned().unwrap_or_default();
                if !include_environ {
                    row.environ.clear();
                } else if row.environ.is_empty() {
                    row.environ =
                        super::process_monitor::macos_environment(pid).unwrap_or_default();
                }
                record.cpu = cpu;
                if let Some(disk) = disk {
                    record.disk = disk;
                }
                record.sampled = sampled;
                rows.push(row);
            }
            return Ok(rows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Child, Command},
        time::Duration,
    };

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn darwin_process_layouts_match_sdk() {
        assert_eq!(std::mem::size_of::<proc_bsdinfo>(), 136);
        assert_eq!(std::mem::size_of::<proc_taskinfo>(), 96);
        assert_eq!(std::mem::size_of::<proc_threadinfo>(), 112);
        assert_eq!(std::mem::size_of::<rusage_info_v2>(), 160);
        assert_eq!(std::mem::size_of::<proc_vnodepathinfo>(), 2352);
        assert_eq!(std::mem::offset_of!(proc_vnodepathinfo, pvi_rdir), 1176);
    }

    #[test]
    fn retains_owned_process_data_and_removes_exited_child() {
        let mut child = OwnedChild(
            Command::new("/usr/bin/python3")
                .args([
                    "-c",
                    "import time; x=bytearray(16*1024*1024); time.sleep(30)",
                ])
                .env("GTOP_NATIVE_MARKER", "owned-child")
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(Duration::from_millis(500));
        let users = sysinfo::Users::new_with_refreshed_list();
        let mut collector = Collector::default();
        let mut ports = HashMap::new();
        ports.insert(child.0.id(), vec![12345]);
        let first = collector.collect(&users, false, &ports).unwrap();
        let first = first.iter().find(|p| p.pid == child.0.id()).unwrap();
        assert!(first.command.contains("bytearray"));
        assert!(!first.executable.is_empty());
        assert_eq!(first.command, first.argv.join(" "));
        assert!(!first.user.is_empty());
        assert!(first.environ.is_empty());
        assert!(first.memory_usage >= 16 * 1024 * 1024);
        assert!(first.virtual_memory >= first.memory_usage);
        assert_eq!(first.ports, [12345]);
        let identity = collector.records[&child.0.id()].identity;
        let cached = &mut collector.records.get_mut(&child.0.id()).unwrap().row;
        cached.executable.clear();
        cached.argv.clear();
        std::thread::sleep(Duration::from_millis(250));
        let rows = collector.collect(&users, true, &ports).unwrap();
        let row = rows.iter().find(|p| p.pid == child.0.id()).unwrap();
        assert_eq!(row.name, first.name);
        assert_eq!(row.user, first.user);
        assert_eq!(row.command, first.command);
        assert_eq!(row.executable, first.executable);
        assert_eq!(row.argv, first.argv);
        assert_eq!(row.root, first.root);
        assert_eq!(row.start_time, first.start_time);
        assert_eq!(row.ppid, std::process::id());
        assert!(row
            .environ
            .iter()
            .any(|v| v == "GTOP_NATIVE_MARKER=owned-child"));
        assert_eq!(collector.records[&child.0.id()].identity, identity);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let rows = collector.collect(&users, false, &ports).unwrap();
        assert!(!rows.iter().any(|p| p.pid == child.0.id()));
        assert!(!collector.records.contains_key(&child.0.id()));
    }

    #[test]
    fn cpu_and_disk_counters_track_owned_busy_child() {
        let scratch = std::env::temp_dir().join(format!("gtop-native-{}", std::process::id()));
        let child = OwnedChild(Command::new("/usr/bin/python3").args(["-c",
            "import os,sys,time; f=open(sys.argv[1],'wb',buffering=0); end=time.monotonic()+30; block=b'x'*1048576\nwhile time.monotonic()<end:\n f.write(block); os.fsync(f.fileno()); f.seek(0)\n", scratch.to_str().unwrap()]).spawn().unwrap());
        std::thread::sleep(Duration::from_millis(300));
        let users = sysinfo::Users::new_with_refreshed_list();
        let mut collector = Collector::default();
        collector.collect(&users, false, &HashMap::new()).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        let rows = collector.collect(&users, false, &HashMap::new()).unwrap();
        let row = rows.iter().find(|p| p.pid == child.0.id()).unwrap();
        assert!(row.cpu_usage.is_finite() && row.cpu_usage > 0.);
        assert!(row.disk_usage.1 > 0, "owned writes missing");
        drop(child);
        std::fs::remove_file(scratch).unwrap();
    }
}
