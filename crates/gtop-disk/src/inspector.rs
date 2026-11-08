//! Measured space for the scan root. Unknown observations carry no old numbers.
use crate::space::{self, SpaceInfo};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeReadout {
    pub root: PathBuf,
    pub identity: Option<String>,
    pub space: Option<SpaceInfo>,
}

impl VolumeReadout {
    pub fn query(root: &Path) -> Self {
        Self::query_with(root, |root| {
            let space = space::space_info(root)?;
            Ok((space::scan_volume_identity(root)?, space))
        })
    }

    pub fn query_with(
        root: &Path,
        query: impl FnOnce(&Path) -> io::Result<(String, SpaceInfo)>,
    ) -> Self {
        let (identity, space) = match query(root) {
            Ok((identity, space)) => (Some(identity), Some(space)),
            Err(_) => (None, None),
        };
        Self {
            root: root.to_path_buf(),
            identity,
            space,
        }
    }

    pub fn identity_label(&self) -> &str {
        self.identity.as_deref().unwrap_or("Unknown")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scan_volume_query_uses_root_and_failed_refresh_clears_numbers() {
        let root = Path::new("/Volumes/External/projects");
        let mut readout = VolumeReadout::query_with(root, |received| {
            assert_eq!(received, root);
            Ok((
                "External · /dev/disk9".into(),
                SpaceInfo {
                    total: 100,
                    free: 40,
                    available: 30,
                },
            ))
        });
        assert_eq!(readout.identity_label(), "External · /dev/disk9");
        assert_eq!(readout.space.unwrap().used(), 60);
        assert_eq!(readout.space.unwrap().available, 30);
        readout = VolumeReadout::query_with(root, |_| Err(io::Error::other("injected failure")));
        assert_eq!(readout.identity_label(), "Unknown");
        assert_eq!(readout.space, None);
        assert_eq!(readout.root, root);
    }
    #[test]
    fn live_query_names_the_scan_root_volume() {
        let root = std::env::temp_dir();
        let readout = VolumeReadout::query(&root);
        assert_eq!(readout.root, root);
        assert!(
            readout
                .identity_label()
                .contains(&space::device_for(&root).unwrap())
        );
        assert!(readout.space.unwrap().total > 0);
    }
}
