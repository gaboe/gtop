use gtop_disk::scan::{ScanOptions, ScanProgress, scan_with_progress};
use std::{path::PathBuf, sync::Arc, time::Instant};

fn main() -> std::io::Result<()> {
    let root = PathBuf::from(std::env::args_os().nth(1).expect("scan root"));
    let progress = Arc::new(ScanProgress::default());
    let start = Instant::now();
    let tree = scan_with_progress(&root, ScanOptions::default(), Arc::clone(&progress))?;
    let snapshot = progress.snapshot();
    println!(
        "seconds={:.6} bytes={} files={} dirs={} unreadable={} complete={} cancelled={} limited={} reserved={} workers={}",
        start.elapsed().as_secs_f64(),
        tree.bytes,
        tree.files,
        tree.dirs,
        tree.unreadable,
        tree.complete,
        snapshot.cancelled,
        snapshot.limited,
        snapshot.retained_bytes,
        snapshot.threads,
    );
    for message in snapshot.messages {
        eprintln!("{message}");
    }
    Ok(())
}
