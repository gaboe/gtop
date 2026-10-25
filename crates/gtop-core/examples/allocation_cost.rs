use gtop_core::AppState;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
struct CountAlloc;
static COUNT: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNT.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        COUNT.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(size as u64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;
fn main() {
    let state = AppState::new();
    for tick in 0..12 {
        std::thread::sleep(Duration::from_millis(250));
        let before = (COUNT.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed));
        let snapshot = state.snapshot(false, false).unwrap();
        let count = COUNT.load(Ordering::Relaxed) - before.0;
        let bytes = BYTES.load(Ordering::Relaxed) - before.1;
        assert!(snapshot
            .processes
            .iter()
            .any(|p| p.pid == std::process::id()));
        if tick >= 2 {
            println!("{{\"tick\":{},\"allocations\":{count},\"bytes_requested\":{bytes},\"processes\":{},\"errors\":0}}",tick-2,snapshot.processes.len());
        }
    }
}
