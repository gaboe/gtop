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

// Darwin's public 64-bit kinfo_proc ABI. libc does not expose this type on macOS.
#[repr(C, align(8))]
#[derive(Clone)]
struct KernelProcess([u8; 648]);

impl KernelProcess {
    fn bsd(&self) -> proc_bsdinfo {
        let bytes = &self.0;
        let mut bsd = proc_bsdinfo {
            pbi_start_tvsec: u64::from_ne_bytes(bytes[0..8].try_into().unwrap()),
            pbi_start_tvusec: u32::from_ne_bytes(bytes[8..12].try_into().unwrap()) as u64,
            pbi_status: bytes[36] as u32,
            pbi_pid: u32::from_ne_bytes(bytes[40..44].try_into().unwrap()),
            pbi_uid: u32::from_ne_bytes(bytes[420..424].try_into().unwrap()),
            pbi_ppid: u32::from_ne_bytes(bytes[560..564].try_into().unwrap()),
            ..Default::default()
        };
        for (to, from) in bsd.pbi_name.iter_mut().zip(&bytes[243..260]) {
            *to = *from as i8;
        }
        bsd
    }
}

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn sysctl(
        name: *mut i32,
        namelen: u32,
        old: *mut c_void,
        oldlen: *mut usize,
        new: *mut c_void,
        newlen: usize,
    ) -> i32;
}

fn kernel_processes() -> Option<HashMap<u32, proc_bsdinfo>> {
    let mut mib = [1, 14, 0]; // CTL_KERN, KERN_PROC, KERN_PROC_ALL.
    let mut bytes = 0;
    if unsafe {
        sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut bytes,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    for _ in 0..3 {
        let mut buffer = vec![KernelProcess([0; 648]); bytes / 648 + 128];
        bytes = std::mem::size_of_val(buffer.as_slice());
        if unsafe {
            sysctl(
                mib.as_mut_ptr(),
                3,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
                std::ptr::null_mut(),
                0,
            )
        } == 0
        {
            if bytes > std::mem::size_of_val(buffer.as_slice()) || bytes % 648 != 0 {
                return None;
            }
            return Some(
                buffer[..bytes / 648]
                    .iter()
                    .map(|p| {
                        let bsd = p.bsd();
                        (bsd.pbi_pid, bsd)
                    })
                    .collect(),
            );
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(12) {
            return None;
        }
        bytes = bytes.max(std::mem::size_of_val(buffer.as_slice())) * 2;
    }
    None
}

fn kernel_process(pid: u32) -> Option<proc_bsdinfo> {
    let pid = i32::try_from(pid).ok()?;
    let mut mib = [1, 14, 1, pid]; // CTL_KERN, KERN_PROC, KERN_PROC_PID.
    let mut buffer = vec![KernelProcess([0; 648]); 8];
    for _ in 0..3 {
        let capacity = std::mem::size_of_val(buffer.as_slice());
        let mut bytes = capacity;
        if unsafe {
            sysctl(
                mib.as_mut_ptr(),
                4,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
                std::ptr::null_mut(),
                0,
            )
        } == 0
        {
            if bytes != 648 {
                return None;
            }
            let bsd = buffer[0].bsd();
            return (bsd.pbi_pid == pid as u32).then_some(bsd);
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(12) {
            return None;
        }
        buffer.resize(buffer.len() * 2, KernelProcess([0; 648]));
    }
    None
}

fn bsd_info(pid: u32) -> Result<proc_bsdinfo, std::io::Error> {
    unsafe { *__error() = 0 };
    info(pid, PROC_PIDTBSDINFO).ok_or_else(std::io::Error::last_os_error)
}

#[derive(Debug)]
struct Record {
    identity: (u64, u64),
    row: ProcessInfo,
    cpu_sample: Option<(u64, Instant)>,
    disk_sample: Option<((u64, u64), Instant)>,
}

#[derive(Debug)]
struct Sample {
    cpu_nanos: Option<u64>,
    disk: Option<(u64, u64)>,
    sampled: Instant,
}

impl Record {
    fn emit(&mut self, row: &mut ProcessInfo, sample: Sample) {
        row.cpu_sample_seconds = sample
            .cpu_nanos
            .zip(self.cpu_sample)
            .map(|(_, (_, at))| sample.sampled.duration_since(at).as_secs_f64());
        row.cpu_usage =
            sample
                .cpu_nanos
                .zip(self.cpu_sample)
                .map_or(0., |(cpu, (previous, at))| {
                    (cpu.saturating_sub(previous) as f64
                        / sample.sampled.duration_since(at).as_nanos().max(1) as f64
                        * 100.) as f32
                });
        row.disk_sample_seconds = sample.disk.zip(self.disk_sample).map(|(_, (_, at))| {
            sample
                .sampled
                .duration_since(at)
                .as_secs_f64()
                .max(0.000001)
        });
        row.disk_usage =
            sample
                .disk
                .zip(self.disk_sample)
                .map_or((0, 0), |(disk, (previous, _))| {
                    (
                        disk.0.saturating_sub(previous.0),
                        disk.1.saturating_sub(previous.1),
                    )
                });
        if let Some(cpu) = sample.cpu_nanos {
            self.cpu_sample = Some((cpu, sample.sampled));
        }
        if let Some(disk) = sample.disk {
            self.disk_sample = Some((disk, sample.sampled));
        }
    }
}
#[derive(Debug, Default)]
pub(super) struct Collector {
    pids: Vec<i32>,
    records: HashMap<u32, Record>,
    primed: bool,
    last_collection: Option<Instant>,
    #[cfg(test)]
    unavailable_counters: Option<u32>,
    #[cfg(test)]
    unavailable_identity: Option<u32>,
    #[cfg(test)]
    denied_identity: Option<u32>,
}
impl Collector {
    #[cfg(test)]
    pub(super) fn fail_next_counter_reads(&mut self, pid: u32) {
        self.unavailable_counters = Some(pid);
    }

    fn read_bsd_info(&self, pid: u32) -> Result<proc_bsdinfo, std::io::Error> {
        let bsd = bsd_info(pid);
        #[cfg(test)]
        if self.denied_identity == Some(pid) {
            return Err(std::io::Error::from_raw_os_error(1));
        }
        bsd
    }

    pub(super) fn identity_matches(&self, pid: u32, start_time: u64) -> bool {
        if pid == 0 || pid > i32::MAX as u32 || start_time == 0 {
            return false;
        }
        let bsd = match self.read_bsd_info(pid) {
            Ok(bsd) => Some(bsd),
            Err(error) if matches!(error.raw_os_error(), Some(1 | 13)) => kernel_process(pid),
            Err(_) => None,
        };
        bsd.is_some_and(|bsd| bsd.pbi_pid == pid && bsd.pbi_start_tvsec == start_time)
    }

    #[cfg(test)]
    pub(super) fn deny_identity(&mut self, pid: u32) {
        self.denied_identity = Some(pid);
    }

    pub fn collect(
        &mut self,
        users: &sysinfo::Users,
        include_environ: bool,
        ports: &HashMap<u32, Vec<u16>>,
    ) -> Result<Vec<ProcessInfo>, String> {
        let mut timebase = Timebase::default();
        if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
            return Err("macOS could not read the CPU timebase".into());
        }
        let nanos_per_unit = timebase.numer as f64 / timebase.denom as f64;
        let now = ProcessMonitor::get_current_time()?;
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
            let mut kernel = None;
            let mut live = HashSet::new();
            let mut metadata = Vec::new();
            let mut samples = Vec::new();
            for &pid in &self.pids[..count as usize] {
                if pid <= 0 {
                    continue;
                }
                let pid = pid as u32;
                live.insert(pid);
                let bsd = self.read_bsd_info(pid);
                let bsd = match bsd {
                    Ok(bsd) => Some(bsd),
                    Err(error) if matches!(error.raw_os_error(), Some(1 | 13)) => kernel
                        .get_or_insert_with(kernel_processes)
                        .as_mut()
                        .and_then(|processes| processes.remove(&pid)),
                    Err(_) => None,
                };
                #[cfg(test)]
                let bsd = if self.unavailable_identity == Some(pid) {
                    self.unavailable_identity = None;
                    None
                } else {
                    bsd
                };
                let identity = bsd
                    .as_ref()
                    .map(|b| (b.pbi_start_tvsec, b.pbi_start_tvusec));
                if identity.is_none() {
                    samples.push((pid, bsd));
                    continue;
                }
                if self.records.get(&pid).map(|r| r.identity) != identity {
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
                    .with_user(UpdateKind::OnlyIfNotSet)
                    .with_cmd(UpdateKind::OnlyIfNotSet)
                    .with_exe(UpdateKind::OnlyIfNotSet)
                    .with_root(UpdateKind::OnlyIfNotSet);
                if include_environ {
                    kind = kind.with_environ(UpdateKind::OnlyIfNotSet);
                }
                sys.refresh_processes_specifics(ProcessesToUpdate::Some(&metadata), true, kind);
            }
            let mut rows = Vec::with_capacity(samples.len());
            for (pid, bsd) in samples {
                let Some(bsd) = bsd else {
                    if let Some(record) = self.records.get(&pid) {
                        let mut row = record.row.clone();
                        row.cpu_usage = 0.;
                        row.cpu_sample_seconds = None;
                        row.disk_usage = (0, 0);
                        row.disk_sample_seconds = None;
                        if !include_environ {
                            row.environ.clear();
                        }
                        rows.push(row);
                    }
                    continue;
                };
                let sampled = Instant::now();
                let task = info::<proc_taskinfo>(pid, PROC_PIDTASKINFO);
                #[cfg(test)]
                let task = task.filter(|_| self.unavailable_counters != Some(pid));
                let cpu = task.as_ref().map(|task| {
                    (task.pti_total_user.saturating_add(task.pti_total_system) as f64
                        * nanos_per_unit) as u64
                });
                let mut usage = rusage_info_v2::default();
                let disk = (unsafe {
                    proc_pid_rusage(
                        pid as i32,
                        RUSAGE_INFO_V2,
                        (&mut usage as *mut rusage_info_v2).cast(),
                    )
                } == 0)
                    .then_some((usage.ri_diskio_bytesread, usage.ri_diskio_byteswritten));
                #[cfg(test)]
                let disk = if self.unavailable_counters == Some(pid) {
                    self.unavailable_counters = None;
                    None
                } else {
                    disk
                };
                let existing = self.records.contains_key(&pid);
                if !existing {
                    let Some(process) = sys.process(Pid::from_u32(pid)) else {
                        continue;
                    };
                    self.records.insert(
                        pid,
                        Record {
                            identity: (bsd.pbi_start_tvsec, bsd.pbi_start_tvusec),
                            row: process_row(
                                pid,
                                process,
                                users,
                                include_environ,
                                ports,
                                None,
                                now,
                            ),
                            cpu_sample: None,
                            disk_sample: self.last_collection.map(|at| ((0, 0), at)),
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
                if record.row.name.is_empty() {
                    let bytes: Vec<u8> = bsd
                        .pbi_name
                        .iter()
                        .map(|b| *b as u8)
                        .take_while(|b| *b != 0)
                        .collect();
                    record.row.name = String::from_utf8_lossy(&bytes).into_owned();
                }
                if record.row.user.is_empty() || record.row.user == "-" {
                    record.row.user = users
                        .iter()
                        .find(|user| **user.id() == bsd.pbi_uid)
                        .map(|user| user.name().to_owned())
                        .unwrap_or_else(|| bsd.pbi_uid.to_string());
                }
                record.row.start_time = bsd.pbi_start_tvsec;
                record.row.ppid = bsd.pbi_ppid;
                let mut row = record.row.clone();
                {
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
                row.memory_usage = task.as_ref().map_or(0, |task| task.pti_resident_size);
                row.virtual_memory = task.as_ref().map_or(0, |task| task.pti_virtual_size);
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
                record.emit(
                    &mut row,
                    Sample {
                        cpu_nanos: cpu,
                        disk,
                        sampled,
                    },
                );
                if !self.primed {
                    row.cpu_usage = 0.;
                    row.cpu_sample_seconds = None;
                    row.disk_usage = (0, 0);
                }
                rows.push(row);
            }
            self.primed = true;
            self.last_collection = Some(Instant::now());
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
        assert_eq!(std::mem::size_of::<KernelProcess>(), 648);
        assert_eq!(std::mem::align_of::<KernelProcess>(), 8);
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
        let mut unseen = Collector {
            unavailable_identity: Some(child.0.id()),
            ..Default::default()
        };
        assert!(!unseen
            .collect(&users, false, &HashMap::new())
            .unwrap()
            .iter()
            .any(|p| p.pid == child.0.id()));
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
        collector.unavailable_identity = Some(child.0.id());
        let unavailable = collector.collect(&users, false, &ports).unwrap();
        let unavailable = unavailable.iter().find(|p| p.pid == child.0.id()).unwrap();
        assert_eq!(unavailable.start_time, first.start_time);
        assert_eq!(unavailable.command, first.command);
        assert_eq!(unavailable.disk_usage, (0, 0));
        assert_eq!(unavailable.disk_sample_seconds, None);
        assert_eq!(unavailable.cpu_usage, 0.);
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
    fn permission_denial_uses_kernel_identity_for_new_and_existing_rows() {
        let child = OwnedChild(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let pid = child.0.id();
        let users = sysinfo::Users::new_with_refreshed_list();
        let expected = bsd_info(pid).unwrap();
        let kernel = kernel_processes().expect("KERN_PROC_ALL requires native host visibility");
        let fallback = &kernel[&pid];
        assert_eq!(fallback.pbi_pid, pid);
        assert_eq!(fallback.pbi_ppid, expected.pbi_ppid);
        assert_eq!(fallback.pbi_uid, expected.pbi_uid);
        assert_eq!(fallback.pbi_start_tvsec, expected.pbi_start_tvsec);
        assert_eq!(fallback.pbi_start_tvusec, expected.pbi_start_tvusec);
        assert!((1..=5).contains(&fallback.pbi_status));
        assert!(fallback.pbi_name.iter().any(|byte| *byte != 0));

        let mut collector = Collector {
            denied_identity: Some(pid),
            unavailable_counters: Some(pid),
            ..Default::default()
        };
        let first = collector.collect(&users, false, &HashMap::new()).unwrap();
        let first = first.iter().find(|row| row.pid == pid).unwrap();
        assert_eq!(first.start_time, expected.pbi_start_tvsec);
        assert_eq!(first.ppid, std::process::id());
        assert!(!first.name.is_empty());
        assert_ne!(first.user, "-");
        assert_eq!(first.memory_usage, 0);
        assert_eq!(first.cpu_sample_seconds, None);
        assert_eq!(first.disk_sample_seconds, None);
        let identity = collector.records[&pid].identity;
        let next = collector.collect(&users, false, &HashMap::new()).unwrap();
        let next = next.iter().find(|row| row.pid == pid).unwrap();
        assert_eq!(next.start_time, first.start_time);
        assert_eq!(collector.records[&pid].identity, identity);
        assert!(next.memory_usage > 0);
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

    #[test]
    fn reports_disk_writes_before_first_collection_after_priming() {
        let users = sysinfo::Users::new_with_refreshed_list();
        let mut collector = Collector::default();
        collector.collect(&users, false, &HashMap::new()).unwrap();

        let suffix = format!(
            "{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        );
        let scratch = std::env::temp_dir().join(format!("gtop-native-first-disk-{suffix}"));
        let ready = scratch.with_extension("ready");
        let script = "import os,sys,time; f=open(sys.argv[1],'wb',buffering=0); f.write(b'x'*(16*1024*1024)); os.fsync(f.fileno()); open(sys.argv[2],'w').close(); time.sleep(30)";
        let mut child = OwnedChild(
            Command::new("/usr/bin/python3")
                .args([
                    "-c",
                    script,
                    scratch.to_str().unwrap(),
                    ready.to_str().unwrap(),
                ])
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready.exists(), "owned child did not finish its fsync");

        let rows = collector.collect(&users, false, &HashMap::new()).unwrap();
        let row = rows.iter().find(|row| row.pid == child.0.id()).unwrap();
        assert!(
            row.disk_usage.1 >= 8 * 1024 * 1024,
            "first disk delta was {} bytes",
            row.disk_usage.1
        );

        child.0.kill().unwrap();
        child.0.wait().unwrap();
        std::fs::remove_file(scratch).unwrap();
        std::fs::remove_file(ready).unwrap();
    }
}
