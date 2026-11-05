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

struct TreeResult {
    path: PathBuf,
    revision: u64,
    result: Result<Vec<PathBuf>, String>,
}

pub struct FolderTree {
    sender: Sender<TreeResult>,
    receiver: Receiver<TreeResult>,
    nodes: HashMap<PathBuf, Result<Vec<PathBuf>, String>>,
    pending: HashMap<PathBuf, u64>,
    next_revision: u64,
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
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(message) = self.receiver.try_recv() {
            if self.pending.get(&message.path) == Some(&message.revision) {
                self.pending.remove(&message.path);
                self.nodes.insert(message.path, message.result);
                changed = true;
            }
        }
        changed
    }

    pub fn get(&self, path: &Path) -> Option<&Result<Vec<PathBuf>, String>> {
        self.nodes.get(path)
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
            entries.push(item.path());
            if entries.len() > MAX_CHILDREN {
                return Err(std::io::Error::other(format!(
                    "Folder contains more than {MAX_CHILDREN} subfolders; tree view is limited"
                )));
            }
        }
    }
    entries.sort_by(|a, b| {
        let a_name = a
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let b_name = b
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        natural_cmp(&a_name, &b_name)
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{FolderTree, list_directories, list_directories_with};
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
        let tree = FolderTree::new();
        assert!(tree.get(&PathBuf::from("/missing")).is_none());
        fs::remove_file(root.join("loop")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
