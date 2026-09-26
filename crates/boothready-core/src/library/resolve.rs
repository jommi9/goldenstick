use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Resolves volume-relative paths the way a FAT/exFAT/HFS+ player would:
/// case-insensitively. Directory listings are cached so validating thousands
/// of tracks costs one listing per folder.
pub struct PathResolver {
    root: PathBuf,
    listings: HashMap<PathBuf, HashMap<String, String>>,
}

impl PathResolver {
    pub fn new(root: &Path) -> Self {
        PathResolver { root: root.to_path_buf(), listings: HashMap::new() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `rel` uses forward slashes and may start with `/`. `..` components are
    /// rejected so a hostile database cannot point outside the volume.
    pub fn resolve(&mut self, rel: &str) -> Option<PathBuf> {
        let mut cur = self.root.clone();
        for part in rel.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".") {
            if part == ".." {
                return None;
            }
            let direct = cur.join(part);
            if direct.symlink_metadata().is_ok() {
                cur = direct;
                continue;
            }
            let listing = self.listings.entry(cur.clone()).or_insert_with(|| {
                fs::read_dir(&cur)
                    .map(|rd| {
                        rd.filter_map(Result::ok)
                            .map(|e| {
                                let n = e.file_name().to_string_lossy().into_owned();
                                (n.to_lowercase(), n)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            });
            let actual = listing.get(&part.to_lowercase())?.clone();
            cur = cur.join(actual);
        }
        cur.symlink_metadata().ok().map(|_| cur)
    }
}
