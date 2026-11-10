//! Lazily expanded directory tree. Filesystem enumeration never runs in UI.

use crate::file_view::natural_cmp;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

const MAX_CHILDREN: usize = 256;
// Older, unused branches need not retain their child paths forever.
const MAX_CACHED_NODES: usize = 128;
// Node count alone still permits 128 * 256 retained child PathBuf allocations.
const MAX_CACHED_CHILDREN: usize = 4_096;

struct TreeResult {
    path: PathBuf,
    revision: u64,
    result: Result<Vec<PathBuf>, String>,
}

struct CachedNode {
    result: Result<Vec<PathBuf>, String>,
    touched: u64,
}

pub struct FolderTree {
    sender: Sender<TreeResult>,
    receiver: Receiver<TreeResult>,
    nodes: HashMap<PathBuf, CachedNode>,
    pending: HashMap<PathBuf, u64>,
    next_revision: u64,
    tick: u64,
}

impl FolderTree {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            nodes: HashMap::new(),
            pending: HashMap::new(),
            next_revision: 0,
            tick: 0,
        }
    }

    fn cache_result(&mut self, path: PathBuf, result: Result<Vec<PathBuf>, String>) {
        self.tick = self.tick.wrapping_add(1);
        self.nodes.insert(
            path,
            CachedNode {
                result,
                touched: self.tick,
            },
        );
        while self.nodes.len() > MAX_CACHED_NODES
            || self
                .nodes
                .values()
                .map(|node| node.result.as_ref().map_or(0, Vec::len))
                .sum::<usize>()
                > MAX_CACHED_CHILDREN
        {
            let Some(oldest) = self.nodes.iter().min_by_key(|(_, node)| node.touched) else {
                break;
            };
            let oldest = oldest.0.clone();
            self.nodes.remove(&oldest);
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(message) = self.receiver.try_recv() {
            if self.pending.get(&message.path) == Some(&message.revision) {
                self.pending.remove(&message.path);
                self.cache_result(message.path, message.result);
                changed = true;
            }
        }
        changed
    }

    pub fn get(&mut self, path: &Path) -> Option<&Result<Vec<PathBuf>, String>> {
        let node = self.nodes.get_mut(path)?;
        self.tick = self.tick.wrapping_add(1);
        node.touched = self.tick;
        Some(&node.result)
    }

    pub fn is_pending(&self, path: &Path) -> bool {
        self.pending.contains_key(path)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn request(&mut self, path: PathBuf, force: bool) {
        if !force && (self.pending.contains_key(&path) || self.nodes.contains_key(&path)) {
            return;
        }
        self.next_revision = self.next_revision.wrapping_add(1);
        let revision = self.next_revision;
        self.pending.insert(path.clone(), revision);
        self.nodes.remove(&path);
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = list_directories(&path).map_err(|error| error.to_string());
            let _ = sender.send(TreeResult {
                path,
                revision,
                result,
            });
        });
    }
}

fn list_directories(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    list_directories_with(path, |entry| entry.file_type())
}

fn list_directories_with(
    path: &Path,
    file_type: impl Fn(&fs::DirEntry) -> std::io::Result<fs::FileType>,
) -> std::io::Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let item = match item {
            Ok(item) => item,
            Err(_) => continue,
        };
        // file_type() is not a recursive stat of the symlink target; only
        // real directories are traversable and loops are never followed.
        // An unplugged child mount must not hide unrelated tree branches.
        if file_type(&item).is_ok_and(|kind| kind.is_dir()) {
            let path = item.path();
            let folded_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            entries.push((path, folded_name));
            if entries.len() > MAX_CHILDREN {
                return Err(std::io::Error::other(format!(
                    "Folder contains more than {MAX_CHILDREN} subfolders; tree view is limited"
                )));
            }
        }
    }
    entries.sort_by(|a, b| natural_cmp(&a.1, &b.1));
    Ok(entries.into_iter().map(|(path, _)| path).collect())
}

#[cfg(test)]
mod tests {
    use super::{
        FolderTree, MAX_CACHED_CHILDREN, MAX_CACHED_NODES, list_directories, list_directories_with,
    };
    use std::{ffi::OsStr, fs, os::unix::fs::symlink, path::PathBuf};

    #[test]
    fn stale_child_mount_does_not_hide_other_tree_directories() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("rice-tree-stale-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("stale")).unwrap();
        fs::create_dir(root.join("good")).unwrap();
        let result = list_directories_with(&root, |item| {
            if item.file_name() == OsStr::new("stale") {
                Err(std::io::Error::from_raw_os_error(libc::ENODEV))
            } else {
                item.file_type()
            }
        })
        .unwrap();
        assert_eq!(result, vec![root.join("good")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tree_lists_real_folders_naturally_without_following_symlinks() {
        let root = std::env::temp_dir().join(format!("rice-tree-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for name in ["folder10", "folder2"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        fs::write(root.join("ignore.txt"), b"file").unwrap();
        symlink(&root, root.join("loop")).unwrap();
        let children = list_directories(&root).unwrap();
        assert_eq!(children, vec![root.join("folder2"), root.join("folder10")]);
        let mut tree = FolderTree::new();
        assert!(tree.get(&PathBuf::from("/missing")).is_none());
        fs::remove_file(root.join("loop")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_tree_branches_are_evicted_but_recently_used_branches_survive() {
        let mut tree = FolderTree::new();
        for index in 0..MAX_CACHED_NODES {
            tree.cache_result(PathBuf::from(format!("/branch-{index}")), Ok(Vec::new()));
        }
        assert!(tree.get(PathBuf::from("/branch-0").as_path()).is_some());
        tree.cache_result(PathBuf::from("/new-branch"), Ok(Vec::new()));
        assert_eq!(tree.nodes.len(), MAX_CACHED_NODES);
        assert!(tree.get(PathBuf::from("/branch-0").as_path()).is_some());
        assert!(tree.get(PathBuf::from("/branch-1").as_path()).is_none());
        assert!(tree.get(PathBuf::from("/new-branch").as_path()).is_some());
    }

    #[test]
    fn tree_cache_limits_total_retained_children() {
        let mut tree = FolderTree::new();
        let children: Vec<_> = (0..256)
            .map(|index| PathBuf::from(format!("/folder-{index}")))
            .collect();
        for index in 0..(MAX_CACHED_CHILDREN / children.len()) {
            tree.cache_result(PathBuf::from(format!("/branch-{index}")), Ok(children.clone()));
        }
        assert!(tree.get(PathBuf::from("/branch-0").as_path()).is_some());
        tree.cache_result(PathBuf::from("/new-branch"), Ok(children));
        assert!(tree.get(PathBuf::from("/branch-0").as_path()).is_some());
        assert!(tree.get(PathBuf::from("/branch-1").as_path()).is_none());
        assert_eq!(tree.nodes.len(), MAX_CACHED_CHILDREN / 256);
    }
}
