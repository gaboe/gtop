use crate::{
    scan::{ScanOptions, scan},
    tree::path_of,
};
use std::{ffi::OsString, fs, os::unix::ffi::OsStringExt};

#[test]
fn raw_names_survive_lossy_display_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let names = [
        OsString::from_vec(vec![b'f', 0x80]),
        OsString::from_vec(vec![b'f', 0x81]),
    ];
    for name in &names {
        fs::write(temp.path().join(name), [1; 4096]).unwrap();
    }
    let tree = scan(temp.path(), ScanOptions::default()).unwrap();
    assert_eq!(tree.children.len(), 2);
    assert_eq!(tree.children[0].name, tree.children[1].name);
    let paths: Vec<_> = (0..2)
        .map(|index| path_of(temp.path(), &tree, &[index]))
        .collect();
    assert_ne!(paths[0], paths[1]);
    for name in &names {
        assert!(paths.contains(&temp.path().join(name)));
    }
}

#[test]
fn default_scan_uses_full_pool_without_adaptive_cpu_governor() {
    let options = ScanOptions::default();
    assert_eq!(options.threads.max_threads, usize::MAX);
    assert!(!options.threads.adaptive);
    assert_eq!(options.threads.system_cpu_limit, None);
    assert!(options.one_filesystem);
    assert!(!options.follow_links);
}
