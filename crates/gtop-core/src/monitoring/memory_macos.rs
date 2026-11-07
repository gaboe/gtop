use super::memory::MemoryPageCounts;
use std::sync::OnceLock;

static HOST: OnceLock<libc::host_t> = OnceLock::new();

unsafe extern "C" {
    #[link_name = "mach_host_self"]
    fn host_self() -> libc::mach_port_t;
}

pub(super) fn sample() -> Option<(MemoryPageCounts, u64)> {
    let host = *HOST.get_or_init(|| unsafe { host_self() });
    let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let result = unsafe {
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }

    let stats = unsafe { stats.assume_init() };
    let page_size = unsafe { libc::vm_page_size } as u64;
    (page_size > 0).then_some((
        MemoryPageCounts {
            wired: stats.wire_count as u64,
            active: stats.active_count as u64,
            compressed: stats.compressor_page_count as u64,
            inactive: stats.inactive_count as u64,
            purgeable: stats.purgeable_count as u64,
            speculative: stats.speculative_count as u64,
            file_backed: stats.external_page_count as u64,
            free: stats.free_count as u64,
        },
        page_size,
    ))
}
