use super::*;
use tempfile::TempDir;

#[test]
fn snapshot_finishing_allows_nested_completed_publication() {
    let progress = ScanProgress::default();
    let node = Node::directory("outer");
    let options = ScanOptions::default();
    publish_completed(
        &progress,
        Path::new("/fixture/outer"),
        &node,
        &options,
        |_| {
            assert!(
                progress.completed.try_lock().is_ok(),
                "snapshot finishing holds the publication slot and blocks nested completion"
            );
            publish_completed(
                &progress,
                Path::new("/fixture/nested"),
                &Node::directory("nested"),
                &options,
                |_| {},
            );
        },
    );
    let (path, _) = progress
        .take_completed()
        .expect("a completed subtree survives");
    assert_eq!(path, Path::new("/fixture/nested"));
}

fn fixed_options() -> ScanOptions {
    ScanOptions {
        threads: ScanThreads {
            max_threads: 1,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn wait(handle: ScanHandle) -> Node {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(result) = handle.poll() {
            return result.expect("scan");
        }
        assert!(std::time::Instant::now() < deadline, "scan did not finish");
        thread::park_timeout(std::time::Duration::from_millis(2));
    }
}

#[test]
fn apparent_cache_cannot_supply_allocated_bytes() {
    use std::os::unix::fs::MetadataExt;
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let inner = root.join("inner");
    fs::create_dir(&inner).unwrap();
    let sparse = inner.join("sparse");
    fs::File::create(&sparse).unwrap().set_len(1 << 30).unwrap();
    let metadata = fs::symlink_metadata(&sparse).unwrap();
    assert!(
        metadata.blocks() * 512 < metadata.len(),
        "fixture must be sparse"
    );
    let cached_options = ScanOptions {
        apparent_size: true,
        ..fixed_options()
    };
    let cached = scan(&inner, cached_options.clone()).unwrap();
    let fresh = scan(&root, fixed_options()).unwrap();
    let result = wait(ScanHandle::spawn_with(
        root,
        fixed_options(),
        Some(Known {
            path: inner,
            tree: Arc::new(cached),
            options: cached_options,
        }),
    ));
    assert_eq!(
        result.bytes, fresh.bytes,
        "allocated scan reused apparent lengths"
    );
}

#[test]
fn hidden_filter_change_requires_fresh_contents() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let inner = root.join("inner");
    fs::create_dir(&inner).unwrap();
    fs::write(inner.join("visible"), b"visible").unwrap();
    fs::write(inner.join(".hidden"), b"hidden").unwrap();
    let cached_options = ScanOptions {
        include_hidden: false,
        ..fixed_options()
    };
    let cached = scan(&inner, cached_options.clone()).unwrap();
    let result = wait(ScanHandle::spawn_with(
        root,
        fixed_options(),
        Some(Known {
            path: inner,
            tree: Arc::new(cached),
            options: cached_options,
        }),
    ));
    assert_eq!(
        result.files, 2,
        "hidden-enabled scan reused filtered contents"
    );
}

#[test]
fn volume_hardlink_metric_and_depth_changes_reject_cached_contents() {
    let cases = [
        ScanOptions {
            one_filesystem: false,
            ..fixed_options()
        },
        ScanOptions {
            dedup_hardlinks: false,
            ..fixed_options()
        },
        ScanOptions {
            metric: Metric::Files,
            ..fixed_options()
        },
        ScanOptions {
            max_depth: Some(8),
            ..fixed_options()
        },
    ];
    for requested in cases {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let inner = root.join("inner");
        fs::create_dir(&inner).unwrap();
        fs::write(inner.join("first"), b"first").unwrap();
        let cached_options = fixed_options();
        let cached = scan(&inner, cached_options.clone()).unwrap();
        fs::write(inner.join("second"), b"second").unwrap();
        let result = wait(ScanHandle::spawn_with(
            root,
            requested,
            Some(Known {
                path: inner,
                tree: Arc::new(cached),
                options: cached_options,
            }),
        ));
        assert_eq!(result.files, 2, "incompatible scan reused cached contents");
    }
}
