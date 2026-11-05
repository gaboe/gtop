use crate::{
    scan::{ScanOptions, scan},
    tree::{Node, NodeKind, path_of},
};
use std::{ffi::OsString, fs, os::unix::ffi::OsStringExt};

#[test]
fn raw_path_projection_survives_lossy_display_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let names = [
        OsString::from_vec(vec![b'f', 0x80]),
        OsString::from_vec(vec![b'f', 0x81]),
    ];
    let mut tree = Node::directory("root");
    for name in &names {
        let mut leaf = Node::entry(name.to_string_lossy().as_ref(), NodeKind::File, 4096);
        leaf.raw_name = Some(name.clone().into_boxed_os_str());
        tree.children.push(leaf);
    }
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
fn scanner_retains_exact_names_for_files_and_directories() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("é-界")).unwrap();
    fs::write(temp.path().join("é-界/data"), [1; 4096]).unwrap();
    let tree = scan(temp.path(), ScanOptions::default()).unwrap();
    let directory = &tree.children[0];
    assert_eq!(
        directory.raw_name.as_deref(),
        Some(std::ffi::OsStr::new("é-界"))
    );
    assert_eq!(
        directory.children[0].raw_name.as_deref(),
        Some(std::ffi::OsStr::new("data"))
    );
    assert_eq!(
        path_of(temp.path(), &tree, &[0, 0]),
        temp.path().join("é-界/data")
    );
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
