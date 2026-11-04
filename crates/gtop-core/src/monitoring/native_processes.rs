use super::{
    process_monitor::{process_row, run_time},
    ProcessInfo, ProcessMonitor,
};
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
const ESRCH: i32 = 3;
const ENOMEM: i32 = 12;

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
    kernel_processes_with(|buffer, bytes| {
        let buffer = buffer.map_or(std::ptr::null_mut(), |buffer| buffer.as_mut_ptr().cast());
        if unsafe { sysctl(mib.as_mut_ptr(), 3, buffer, bytes, std::ptr::null_mut(), 0) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().raw_os_error())
        }
    })
}

/// Reads the process table with `read`, which fills a buffer (or only sizes the
/// table when it gets none) like `sysctl`. A table that outgrows the buffer is
/// read again with a larger one, at most three times.
fn kernel_processes_with(
    mut read: impl FnMut(Option<&mut [KernelProcess]>, &mut usize) -> Result<(), Option<i32>>,
) -> Option<HashMap<u32, proc_bsdinfo>> {
    let mut bytes = 0;
    read(None, &mut bytes).ok()?;
    for _ in 0..3 {
        let mut buffer = vec![KernelProcess([0; 648]); bytes / 648 + 128];
        bytes = std::mem::size_of_val(buffer.as_slice());
        match read(Some(&mut buffer), &mut bytes) {
            Ok(()) => return kernel_table(&buffer, bytes),
            Err(Some(ENOMEM)) => {
                bytes = bytes.max(std::mem::size_of_val(buffer.as_slice())) * 2;
            }
            Err(_) => return None,
        }
    }
    None
}

/// The first `bytes` of `buffer` as records keyed by PID, if they are whole records.
fn kernel_table(buffer: &[KernelProcess], bytes: usize) -> Option<HashMap<u32, proc_bsdinfo>> {
    if bytes > std::mem::size_of_val(buffer) || !bytes.is_multiple_of(648) {
        return None;
    }
    Some(
        buffer[..bytes / 648]
            .iter()
            .map(|p| {
                let bsd = p.bsd();
                (bsd.pbi_pid, bsd)
            })
            .collect(),
    )
}

fn kernel_process(pid: u32) -> Option<proc_bsdinfo> {
    let pid = i32::try_from(pid).ok()?;
    let mut mib = [1, 14, 1, pid]; // CTL_KERN, KERN_PROC, KERN_PROC_PID.
    let mut buffer = vec![KernelProcess([0; 648]); 8];
    let mut bytes = std::mem::size_of_val(buffer.as_slice());
    if unsafe {
        sysctl(
            mib.as_mut_ptr(),
            4,
            buffer.as_mut_ptr().cast(),
            &mut bytes,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    if bytes != 648 {
        return None;
    }
    let bsd = buffer[0].bsd();
    (bsd.pbi_pid == pid as u32).then_some(bsd)
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

enum IdentityRead {
    Available(proc_bsdinfo),
    Unavailable,
    ConfirmedAbsent,
}

fn permission_denied(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(1 | 13))
}

/// The identity a guarded action may trust. A denied read falls back to the
/// kernel table; any other failure, a zombie's ESRCH included, gives none.
fn trusted_identity(
    read: Result<proc_bsdinfo, std::io::Error>,
    kernel: impl FnOnce() -> Option<proc_bsdinfo>,
) -> Option<proc_bsdinfo> {
    match read {
        Ok(bsd) => Some(bsd),
        Err(error) if permission_denied(&error) => kernel(),
        Err(_) => None,
    }
}

/// Classifies one listed PID. `kernel` caches the process table, read by `load`
/// at most once per collection, for PIDs whose own read fails. If the table is
/// denied, `lookup` can still read that PID with KERN_PROC_PID.
fn listed_identity(
    pid: u32,
    read: Result<proc_bsdinfo, std::io::Error>,
    kernel: &mut Option<Option<HashMap<u32, proc_bsdinfo>>>,
    load: impl FnOnce() -> Option<HashMap<u32, proc_bsdinfo>>,
    lookup: impl FnOnce() -> Option<proc_bsdinfo>,
) -> IdentityRead {
    match read {
        Ok(bsd) => IdentityRead::Available(bsd),
        Err(error) if error.raw_os_error() == Some(ESRCH) || permission_denied(&error) => {
            match kernel.get_or_insert_with(load) {
                Some(processes) => {
                    let missing = if error.raw_os_error() == Some(ESRCH) {
                        IdentityRead::ConfirmedAbsent
                    } else {
                        IdentityRead::Unavailable
                    };
                    processes
                        .remove(&pid)
                        .map_or(missing, IdentityRead::Available)
                }
                None => lookup().map_or(IdentityRead::Unavailable, IdentityRead::Available),
            }
        }
        Err(_) => IdentityRead::Unavailable,
    }
}

fn nanos_per_unit(status: i32, timebase: &Timebase) -> Option<f64> {
    (status == 0 && timebase.denom != 0).then(|| timebase.numer as f64 / timebase.denom as f64)
}

/// Lists all PIDs into `pids` with `list`, which works like `proc_listallpids`:
/// without a buffer it counts the PIDs. The buffer grows until the list fits.
fn list_pids(
    pids: &mut Vec<i32>,
    mut list: impl FnMut(Option<&mut [i32]>) -> i32,
) -> Option<usize> {
    loop {
        if pids.is_empty() {
            let count = list(None);
            if count <= 0 {
                return None;
            }
            pids.resize(count as usize + 128, 0);
        }
        let count = list(Some(pids));
        if count <= 0 {
            return None;
        }
        if count as usize >= pids.len() {
            pids.resize(pids.len() * 2, 0);
            continue;
        }
        return Some(count as usize);
    }
}

fn needs_metadata(row: &ProcessInfo) -> bool {
    row.command.is_empty() || row.name.is_empty() || row.executable.is_empty()
}

/// The cached row repeated for a PID whose identity cannot be read this tick.
fn unavailable_row(cached: &ProcessInfo, include_environ: bool) -> ProcessInfo {
    let mut row = cached.clone();
    row.cpu_usage = 0.;
    row.cpu_sample_seconds = None;
    row.disk_usage = (0, 0);
    row.disk_sample_seconds = None;
    if !include_environ {
        row.environ.clear();
    }
    row
}

/// The root directory of a process that has one set, such as a chroot.
fn root_path(vnodes: &proc_vnodepathinfo) -> Option<String> {
    if vnodes.pvi_rdir.vip_vi.vi_stat.vst_dev == 0 {
        return None;
    }
    let bytes = vnodes.pvi_rdir.vip_path.as_flattened();
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

fn bsd_name(bsd: &proc_bsdinfo) -> String {
    let bytes: Vec<u8> = bsd
        .pbi_name
        .iter()
        .map(|b| *b as u8)
        .take_while(|b| *b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Names an unknown owner from `users`, or by its numeric UID.
fn fill_user<'a>(user: &mut String, uid: u32, mut users: impl Iterator<Item = (u32, &'a str)>) {
    if user.is_empty() || user == "-" {
        *user = users
            .find(|(id, _)| *id == uid)
            .map_or_else(|| uid.to_string(), |(_, name)| name.to_owned());
    }
}

/// A runnable process is sleeping when its thread waits.
fn run_status(bsd_status: u32, run_state: impl FnOnce() -> Option<i32>) -> sysinfo::ProcessStatus {
    if bsd_status == 2 {
        match run_state() {
            Some(3) => sysinfo::ProcessStatus::Sleep,
            Some(1) | None => sysinfo::ProcessStatus::Run,
            _ => sysinfo::ProcessStatus::Unknown(0),
        }
    } else {
        sysinfo::ProcessStatus::from(bsd_status)
    }
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
    confirmed_absent_identity: Option<u32>,
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
        if pid == 0 || i32::try_from(pid).is_err() {
            return false;
        }
        trusted_identity(self.read_bsd_info(pid), || kernel_process(pid))
            .is_some_and(|bsd| bsd.pbi_pid == pid && bsd.pbi_start_tvsec == start_time)
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
        let status = unsafe { mach_timebase_info(&mut timebase) };
        let nanos_per_unit =
            nanos_per_unit(status, &timebase).ok_or("macOS could not read the CPU timebase")?;
        let now = ProcessMonitor::get_current_time()?;
        let count = list_pids(&mut self.pids, |buffer| match buffer {
            None => unsafe { proc_listallpids(std::ptr::null_mut(), 0) },
            Some(buffer) => unsafe {
                proc_listallpids(
                    buffer.as_mut_ptr().cast(),
                    std::mem::size_of_val(buffer) as i32,
                )
            },
        })
        .ok_or("macOS could not enumerate processes")?;
        let mut kernel = None;
        let mut live = HashSet::new();
        let mut metadata = Vec::new();
        let mut samples = Vec::new();
        for &pid in &self.pids[..count] {
            if pid <= 0 {
                continue;
            }
            let pid = pid as u32;
            let identity = listed_identity(
                pid,
                self.read_bsd_info(pid),
                &mut kernel,
                kernel_processes,
                || kernel_process(pid),
            );
            #[cfg(test)]
            let identity = if self.confirmed_absent_identity == Some(pid) {
                self.confirmed_absent_identity = None;
                IdentityRead::ConfirmedAbsent
            } else if self.unavailable_identity == Some(pid) {
                self.unavailable_identity = None;
                IdentityRead::Unavailable
            } else {
                identity
            };
            let bsd = match identity {
                IdentityRead::Available(bsd) => Some(bsd),
                IdentityRead::Unavailable => None,
                IdentityRead::ConfirmedAbsent => continue,
            };
            live.insert(pid);
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
            if self
                .records
                .get(&pid)
                .is_some_and(|r| needs_metadata(&r.row))
            {
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
                    rows.push(unavailable_row(&record.row, include_environ));
                }
                continue;
            };
            let sampled = Instant::now();
            let task = info::<proc_taskinfo>(pid, PROC_PIDTASKINFO);
            #[cfg(test)]
            let task = task.filter(|_| self.unavailable_counters != Some(pid));
            let cpu = task.as_ref().map(|task| {
                (task.pti_total_user.saturating_add(task.pti_total_system) as f64 * nanos_per_unit)
                    as u64
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
                        row: process_row(pid, process, users, include_environ, ports, None, now),
                        cpu_sample: None,
                        disk_sample: self
                            .last_collection
                            .filter(|at| {
                                let wall_now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs_f64();
                                born_in_interval(
                                    bsd.pbi_start_tvsec as f64
                                        + bsd.pbi_start_tvusec as f64 / 1_000_000.,
                                    wall_now,
                                    at.elapsed().as_secs_f64(),
                                )
                            })
                            .map(|at| ((0, 0), at)),
                    },
                );
            }
            let record = self.records.get_mut(&pid).unwrap();
            if needs_metadata(&record.row) {
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
                if let Some(root) = info::<proc_vnodepathinfo>(pid, PROC_PIDVNODEPATHINFO)
                    .as_ref()
                    .and_then(root_path)
                {
                    record.row.root = root;
                }
            }
            if record.row.name.is_empty() {
                record.row.name = bsd_name(&bsd);
            }
            fill_user(
                &mut record.row.user,
                bsd.pbi_uid,
                users.iter().map(|user| (**user.id(), user.name())),
            );
            record.row.start_time = bsd.pbi_start_tvsec;
            record.row.ppid = bsd.pbi_ppid;
            let mut row = record.row.clone();
            {
                row.ppid = bsd.pbi_ppid;
                row.start_time = bsd.pbi_start_tvsec;
                let status = run_status(bsd.pbi_status, || {
                    info::<proc_threadinfo>(pid, PROC_PIDTHREADINFO).map(|t| t.pth_run_state)
                });
                row.status = ProcessMonitor::format_status(status);
            }
            row.memory_usage = task.as_ref().map_or(0, |task| task.pti_resident_size);
            row.virtual_memory = task.as_ref().map_or(0, |task| task.pti_virtual_size);
            row.run_time = run_time(row.start_time, now);
            row.session_id = u32::try_from(unsafe { getsid(pid as i32) }).ok();
            row.ports = ports.get(&pid).cloned().unwrap_or_default();
            if !include_environ {
                row.environ.clear();
            } else if row.environ.is_empty() {
                row.environ = super::process_monitor::macos_environment(pid).unwrap_or_default();
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
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        os::fd::AsRawFd,
        process::{Child, Command, Stdio},
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
                    "import time; x=bytearray(16*1024*1024); x[:]=b'x'*len(x); print('ready', flush=True); time.sleep(30)",
                ])
                .env("GTOP_NATIVE_MARKER", "owned-child")
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut stdout = child.0.stdout.take().unwrap();
        let mut ready = libc::pollfd {
            fd: stdout.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut ready, 1, 10_000) },
            1,
            "owned child did not finish touching its memory"
        );
        let mut message = [0; 6];
        stdout.read_exact(&mut message).unwrap();
        assert_eq!(&message, b"ready\n");
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
        let discovered = unseen.collect(&users, false, &HashMap::new()).unwrap();
        let discovered = discovered.iter().find(|p| p.pid == child.0.id()).unwrap();
        assert_eq!(
            discovered.disk_sample_seconds, None,
            "older first discovery must only prime disk counters"
        );
        assert_eq!(discovered.disk_usage, (0, 0));
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
    fn confirmed_absence_drops_record_while_unavailable_identity_keeps_row() {
        let child = OwnedChild(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let pid = child.0.id();
        let users = sysinfo::Users::new_with_refreshed_list();
        let mut collector = Collector::default();

        let rows = collector.collect(&users, false, &HashMap::new()).unwrap();
        assert!(rows.iter().any(|row| row.pid == pid));
        assert!(collector.records.contains_key(&pid));

        collector.unavailable_identity = Some(pid);
        let rows = collector.collect(&users, false, &HashMap::new()).unwrap();
        assert!(rows.iter().any(|row| row.pid == pid));
        assert!(collector.records.contains_key(&pid));

        collector.confirmed_absent_identity = Some(pid);
        let rows = collector.collect(&users, false, &HashMap::new()).unwrap();
        assert!(!rows.iter().any(|row| row.pid == pid));
        assert!(!collector.records.contains_key(&pid));
    }

    #[test]
    fn permission_denial_uses_kernel_identity_for_new_and_existing_rows() {
        let child = OwnedChild(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let pid = child.0.id();
        let users = sysinfo::Users::new_with_refreshed_list();
        let expected = bsd_info(pid).unwrap();
        let fallback = kernel_process(pid).expect("KERN_PROC_PID can read an owned child");
        assert_eq!(fallback.pbi_pid, pid);
        assert_eq!(fallback.pbi_ppid, expected.pbi_ppid);
        assert_eq!(fallback.pbi_uid, expected.pbi_uid);
        assert_eq!(fallback.pbi_start_tvsec, expected.pbi_start_tvsec);
        assert_eq!(fallback.pbi_start_tvusec, expected.pbi_start_tvusec);
        assert_eq!(fallback.pbi_status, expected.pbi_status);
        assert_eq!(bsd_name(&fallback), bsd_name(&expected));
        assert!(!bsd_name(&fallback).is_empty());

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
    #[ignore = "host test: KERN_PROC_ALL is denied in the Codex sandbox"]
    fn kernel_table_and_pid_lookup_agree_on_live_and_zombie_identity() {
        let mut child = OwnedChild(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let pid = child.0.id();
        let fields = |bsd: &proc_bsdinfo| {
            (
                bsd.pbi_pid,
                bsd.pbi_ppid,
                bsd.pbi_uid,
                bsd.pbi_start_tvsec,
                bsd.pbi_start_tvusec,
                bsd.pbi_status,
                bsd_name(bsd),
            )
        };
        let live = bsd_info(pid).unwrap();
        let single = kernel_process(pid).unwrap();
        let table = kernel_processes().expect("KERN_PROC_ALL requires native host visibility");
        assert_eq!(fields(&single), fields(&table[&pid]));
        assert_eq!(fields(&single), fields(&live));

        child.0.kill().unwrap();
        let mut status = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        assert_eq!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    status.as_mut_ptr(),
                    libc::WEXITED | libc::WNOWAIT,
                )
            },
            0
        );
        let zombie = kernel_process(pid).unwrap();
        let table = kernel_processes().unwrap();
        assert_eq!(zombie.pbi_status, 5);
        assert_eq!(fields(&zombie), fields(&table[&pid]));
        assert_eq!(bsd_info(pid).unwrap_err().raw_os_error(), Some(ESRCH));
    }

    #[test]
    fn reports_unreaped_child_as_zombie_then_removes_it_after_reaping() {
        let mut child = OwnedChild(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let pid = child.0.id();
        let users = sysinfo::Users::new_with_refreshed_list();
        let mut collector = Collector::default();

        let running = collector.collect(&users, false, &HashMap::new()).unwrap();
        assert_eq!(
            running.iter().find(|row| row.pid == pid).unwrap().status,
            "Running"
        );

        child.0.kill().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let zombie = loop {
            let zombie = kernel_process(pid).filter(|process| process.pbi_status == 5);
            if let Some(zombie) = zombie {
                break zombie;
            }
            assert!(Instant::now() < deadline, "child did not become a zombie");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(zombie.pbi_pid, pid);
        assert_eq!(bsd_info(pid).unwrap_err().raw_os_error(), Some(ESRCH));

        let rows = collector.collect(&users, false, &HashMap::new()).unwrap();
        assert_eq!(
            rows.iter().find(|row| row.pid == pid).unwrap().status,
            "Zombie"
        );

        child.0.wait().unwrap();
        let rows = collector.collect(&users, false, &HashMap::new()).unwrap();
        assert!(!rows.iter().any(|row| row.pid == pid));
        assert!(!collector.records.contains_key(&pid));
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

#[cfg(test)]
mod pure_tests {
    use super::*;

    fn record(pid: u32) -> KernelProcess {
        let mut record = KernelProcess([0; 648]);
        record.0[40..44].copy_from_slice(&pid.to_ne_bytes());
        record
    }

    fn bsd(pid: u32) -> proc_bsdinfo {
        proc_bsdinfo {
            pbi_pid: pid,
            ..Default::default()
        }
    }

    fn error(code: i32) -> Result<proc_bsdinfo, std::io::Error> {
        Err(std::io::Error::from_raw_os_error(code))
    }

    fn pids(table: Option<HashMap<u32, proc_bsdinfo>>) -> Option<Vec<u32>> {
        table.map(|table| {
            let mut pids: Vec<_> = table.into_keys().collect();
            pids.sort_unstable();
            pids
        })
    }

    #[test]
    fn kernel_table_keeps_only_whole_records_within_the_buffer() {
        let buffer = [record(1), record(2), record(3)];
        assert_eq!(pids(kernel_table(&buffer, 2 * 648)), Some(vec![1, 2]));
        assert_eq!(pids(kernel_table(&buffer, 3 * 648)), Some(vec![1, 2, 3]));
        assert_eq!(pids(kernel_table(&buffer, 0)), Some(vec![]));
        assert_eq!(pids(kernel_table(&buffer, 4 * 648)), None);
        assert_eq!(pids(kernel_table(&buffer, 648 + 1)), None);
    }

    /// A process table of `count` records that the probe reports as `probed` records.
    fn table_reader(
        probed: usize,
        count: usize,
        calls: &mut usize,
    ) -> impl FnMut(Option<&mut [KernelProcess]>, &mut usize) -> Result<(), Option<i32>> + '_ {
        move |buffer, bytes| {
            *calls += 1;
            let Some(buffer) = buffer else {
                *bytes = probed * 648;
                return Ok(());
            };
            assert_eq!(*bytes, std::mem::size_of_val(buffer));
            if buffer.len() < count {
                return Err(Some(ENOMEM));
            }
            for (pid, slot) in buffer[..count].iter_mut().enumerate() {
                *slot = record(pid as u32 + 1);
            }
            *bytes = count * 648;
            Ok(())
        }
    }

    #[test]
    fn kernel_table_read_doubles_the_buffer_while_the_table_grows() {
        let mut calls = 0;
        let table = kernel_processes_with(table_reader(0, 700, &mut calls));
        assert_eq!(pids(table), Some((1..=700).collect()));
        assert_eq!(calls, 4);

        let mut calls = 0;
        assert!(kernel_processes_with(table_reader(0, 1000, &mut calls)).is_none());
        assert_eq!(calls, 4, "gives up after three reads");

        let mut calls = 0;
        let table = kernel_processes_with(table_reader(10, 10, &mut calls));
        assert_eq!(pids(table), Some((1..=10).collect()));
        assert_eq!(calls, 2);
    }

    #[test]
    fn kernel_table_read_stops_on_other_errors() {
        let mut calls = 0;
        let table = kernel_processes_with(|buffer, _| {
            calls += 1;
            if buffer.is_some() {
                Err(Some(1))
            } else {
                Ok(())
            }
        });
        assert!(table.is_none());
        assert_eq!(calls, 2);
        assert!(kernel_processes_with(|_, _| Err(None)).is_none());
    }

    #[test]
    fn guarded_actions_trust_the_kernel_table_only_after_a_permission_denial() {
        let kernel = || Some(bsd(7));
        assert_eq!(
            trusted_identity(Ok(bsd(5)), || unreachable!())
                .unwrap()
                .pbi_pid,
            5
        );
        assert_eq!(trusted_identity(error(1), kernel).unwrap().pbi_pid, 7);
        assert_eq!(trusted_identity(error(13), kernel).unwrap().pbi_pid, 7);
        assert!(trusted_identity(error(ESRCH), kernel).is_none());
        assert!(trusted_identity(error(5), kernel).is_none());
    }

    #[test]
    fn listed_pids_fall_back_to_the_kernel_table_by_failure_kind() {
        let classify = |code: i32, table: Option<&[u32]>| {
            let mut kernel =
                Some(table.map(|pids| pids.iter().map(|&pid| (pid, bsd(pid))).collect()));
            listed_identity(7, error(code), &mut kernel, || unreachable!(), || None)
        };
        let available =
            |identity| matches!(identity, IdentityRead::Available(bsd) if bsd.pbi_pid == 7);
        assert!(available(classify(ESRCH, Some(&[7]))));
        assert!(matches!(
            classify(ESRCH, Some(&[8])),
            IdentityRead::ConfirmedAbsent
        ));
        assert!(matches!(classify(ESRCH, None), IdentityRead::Unavailable));
        assert!(available(classify(1, Some(&[7]))));
        assert!(available(classify(13, Some(&[7]))));
        assert!(matches!(classify(1, Some(&[8])), IdentityRead::Unavailable));
        assert!(matches!(classify(13, None), IdentityRead::Unavailable));
        assert!(matches!(classify(5, Some(&[7])), IdentityRead::Unavailable));

        let mut kernel = None;
        let mut loads = 0;
        for pid in [7, 8] {
            listed_identity(
                pid,
                error(ESRCH),
                &mut kernel,
                || {
                    loads += 1;
                    Some([(7, bsd(7)), (8, bsd(8))].into())
                },
                || unreachable!(),
            );
        }
        assert_eq!(loads, 1, "one table read per collection");
        assert!(matches!(
            listed_identity(9, Ok(bsd(9)), &mut kernel, || unreachable!(), || unreachable!()),
            IdentityRead::Available(bsd) if bsd.pbi_pid == 9
        ));
    }

    #[test]
    fn listed_pids_use_single_lookup_only_when_the_table_is_unavailable() {
        for code in [ESRCH, 1, 13] {
            let mut kernel = None;
            let mut loads = 0;
            for pid in [7, 8] {
                let identity = listed_identity(
                    pid,
                    error(code),
                    &mut kernel,
                    || {
                        loads += 1;
                        None
                    },
                    || Some(bsd(pid)),
                );
                assert!(matches!(identity, IdentityRead::Available(bsd) if bsd.pbi_pid == pid));
            }
            assert_eq!(loads, 1);
            assert!(matches!(
                listed_identity(9, error(code), &mut kernel, || unreachable!(), || None),
                IdentityRead::Unavailable
            ));
            let mut kernel = Some(Some(HashMap::new()));
            let identity = listed_identity(
                7,
                error(code),
                &mut kernel,
                || unreachable!(),
                || unreachable!(),
            );
            if code == ESRCH {
                assert!(matches!(identity, IdentityRead::ConfirmedAbsent));
            } else {
                assert!(matches!(identity, IdentityRead::Unavailable));
            }
        }
        assert!(matches!(
            listed_identity(7, error(5), &mut None, || unreachable!(), || unreachable!()),
            IdentityRead::Unavailable
        ));
    }

    #[test]
    fn timebase_converts_ticks_only_after_a_valid_read() {
        let timebase = |numer, denom| Timebase { numer, denom };
        assert_eq!(nanos_per_unit(0, &timebase(125, 3)), Some(125. / 3.));
        assert_eq!(nanos_per_unit(1, &timebase(125, 3)), None);
        assert_eq!(nanos_per_unit(0, &timebase(1, 0)), None);
    }

    #[test]
    fn pid_list_grows_until_every_pid_fits() {
        const LIVE: usize = 300;
        let mut calls = 0;
        let mut buffer = Vec::new();
        let count = list_pids(&mut buffer, |pids| {
            calls += 1;
            assert!(calls <= 6, "the buffer never fit the list");
            let Some(pids) = pids else {
                return 10;
            };
            let filled = pids.len().min(LIVE);
            for (slot, pid) in pids.iter_mut().zip(1..) {
                *slot = pid;
            }
            filled as i32
        });
        assert_eq!(count, Some(LIVE));
        assert_eq!(buffer[..LIVE], (1..=LIVE as i32).collect::<Vec<_>>());
        assert_eq!(calls, 4);
        assert_eq!(list_pids(&mut Vec::new(), |_| 0), None);
        assert_eq!(list_pids(&mut vec![0; 4], |_| -1), None);
    }

    #[test]
    fn rows_missing_a_command_name_or_executable_need_metadata() {
        let complete = ProcessInfo {
            command: "sleep 30".into(),
            name: "sleep".into(),
            executable: "/bin/sleep".into(),
            ..Default::default()
        };
        assert!(!needs_metadata(&complete));
        for clear in [
            |row: &mut ProcessInfo| row.command.clear(),
            |row: &mut ProcessInfo| row.name.clear(),
            |row: &mut ProcessInfo| row.executable.clear(),
        ] {
            let mut row = complete.clone();
            clear(&mut row);
            assert!(needs_metadata(&row));
        }
    }

    #[test]
    fn unavailable_row_drops_counters_and_unrequested_environment() {
        let cached = ProcessInfo {
            pid: 7,
            command: "sleep 30".into(),
            cpu_usage: 12.,
            cpu_sample_seconds: Some(1.),
            disk_usage: (3, 4),
            disk_sample_seconds: Some(1.),
            environ: vec!["PATH=/bin".into()],
            ..Default::default()
        };
        let row = unavailable_row(&cached, false);
        assert_eq!((row.pid, row.command.as_str()), (7, "sleep 30"));
        assert_eq!(row.cpu_usage, 0.);
        assert_eq!(row.cpu_sample_seconds, None);
        assert_eq!(row.disk_usage, (0, 0));
        assert_eq!(row.disk_sample_seconds, None);
        assert!(row.environ.is_empty());
        assert_eq!(unavailable_row(&cached, true).environ, ["PATH=/bin"]);
    }

    #[test]
    fn root_path_reads_a_set_root_directory() {
        // SAFETY: the vnode path record holds only integers and byte arrays.
        let mut vnodes: proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        vnodes.pvi_rdir.vip_path[0][..6].copy_from_slice(b"/jail\0");
        assert_eq!(
            root_path(&vnodes),
            None,
            "a zero device means no root is set"
        );
        vnodes.pvi_rdir.vip_vi.vi_stat.vst_dev = 1;
        assert_eq!(root_path(&vnodes).as_deref(), Some("/jail"));
    }

    #[test]
    fn bsd_name_stops_at_the_first_nul() {
        let mut info = bsd(7);
        for (slot, byte) in info.pbi_name.iter_mut().zip(b"sleep\0junk") {
            *slot = *byte as i8;
        }
        assert_eq!(bsd_name(&info), "sleep");
    }

    #[test]
    fn unknown_owner_is_named_from_the_users_or_by_uid() {
        let users = [(501, "gabriel"), (0, "root")];
        for unknown in ["", "-"] {
            let mut user = unknown.to_string();
            fill_user(&mut user, 0, users.into_iter());
            assert_eq!(user, "root");
        }
        let mut user = "-".to_string();
        fill_user(&mut user, 7, users.into_iter());
        assert_eq!(user, "7");
        let mut user = "alice".to_string();
        fill_user(&mut user, 0, users.into_iter());
        assert_eq!(user, "alice");
    }

    #[test]
    fn runnable_status_follows_the_thread_run_state() {
        use sysinfo::ProcessStatus;
        assert_eq!(run_status(2, || Some(3)), ProcessStatus::Sleep);
        assert_eq!(run_status(2, || Some(1)), ProcessStatus::Run);
        assert_eq!(run_status(2, || None), ProcessStatus::Run);
        assert_eq!(run_status(2, || Some(5)), ProcessStatus::Unknown(0));
        assert_eq!(run_status(5, || unreachable!()), ProcessStatus::Zombie);
    }

    #[test]
    fn identity_never_matches_the_kernel_task() {
        let kernel_task = kernel_process(0).expect("pid 0 is in the kernel table");
        assert_eq!(kernel_task.pbi_pid, 0);
        assert!(!Collector::default().identity_matches(0, kernel_task.pbi_start_tvsec));
    }
}

fn born_in_interval(birth: f64, now: f64, span: f64) -> bool {
    birth >= now - span && birth <= now
}

#[cfg(test)]
mod disk_baseline_tests {
    use super::*;
    #[test]
    fn older_first_discovery_only_primes_disk_counter() {
        assert!(!born_in_interval(1000., 6003., 3.));
        assert!(born_in_interval(6001., 6003., 3.));
    }
}
