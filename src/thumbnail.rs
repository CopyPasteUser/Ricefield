//! Low-memory thumbnail scheduling for currently visible file cells.
//! Only one decoder runs at a time; stale requests are dropped when scrolling.

use crate::{filesystem::Entry, preview};
use eframe::egui;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::SystemTime,
};

// Keep roughly one viewport of decoded thumbnails. Each 108x108 image can
// occupy multiple CPU/GPU copies; a larger LRU provides little scrolling
// benefit but retains textures after leaving an image-heavy directory.
const MAX_CACHED: usize = 48;
const MAX_PENDING: usize = 48;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
}

impl Key {
    fn from_entry(entry: &Entry) -> Option<Self> {
        if entry.is_dir || entry.is_symlink || entry.kind() != "Image" {
            return None;
        }
        Some(Self {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
        })
    }
}

struct Cached {
    texture: Option<egui::TextureHandle>,
    touched: u64,
}

struct ResultMessage {
    key: Key,
    result: Option<preview::Thumbnail>,
}

pub struct ThumbnailManager {
    cache: HashMap<Key, Cached>,
    pending: VecDeque<Key>,
    active: Option<Key>,
    tick: u64,
    sender: Sender<ResultMessage>,
    receiver: Receiver<ResultMessage>,
}

impl Default for ThumbnailManager {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            cache: HashMap::new(),
            pending: VecDeque::new(),
            active: None,
            tick: 0,
            sender,
            receiver,
        }
    }
}

impl ThumbnailManager {
    /// Cached negative results prevent a corrupt image being decoded every frame.
    pub fn request(&mut self, entry: &Entry) -> Option<(egui::TextureId, egui::Vec2)> {
        let key = Key::from_entry(entry)?;
        self.tick = self.tick.wrapping_add(1);
        if let Some(cached) = self.cache.get_mut(&key) {
            cached.touched = self.tick;
            return cached
                .texture
                .as_ref()
                .map(|texture| (texture.id(), texture.size_vec2()));
        }
        if self.active.as_ref() == Some(&key) {
            return None;
        }
        if let Some(index) = self.pending.iter().position(|pending| pending == &key) {
            self.pending.remove(index);
        }
        self.pending.push_front(key);
        self.pending.truncate(MAX_PENDING);
        None
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub fn has_pending(&self) -> bool {
        self.active.is_some() || !self.pending.is_empty()
    }

    pub fn poll(&mut self, context: &egui::Context) {
        while let Ok(message) = self.receiver.try_recv() {
            if self.active.as_ref() != Some(&message.key) {
                continue;
            }
            self.active = None;
            self.tick = self.tick.wrapping_add(1);
            let texture = message.result.map(|image| {
                let pixels = egui::ColorImage::from_rgba_unmultiplied(
                    [image.width, image.height],
                    &image.rgba,
                );
                context.load_texture("rice-thumbnail", pixels, egui::TextureOptions::LINEAR)
            });
            if self.cache.len() >= MAX_CACHED
                && let Some(oldest) = self.cache.iter().min_by_key(|(_, value)| value.touched)
            {
                let key = oldest.0.clone();
                self.cache.remove(&key);
            }
            self.cache.insert(
                message.key,
                Cached {
                    texture,
                    touched: self.tick,
                },
            );
        }
    }

    pub fn dispatch(&mut self) {
        if self.active.is_some() {
            return;
        }
        let Some(key) = self.pending.pop_front() else {
            return;
        };
        let sender = self.sender.clone();
        self.active = Some(key.clone());
        thread::spawn(move || {
            // Malformed image decoders must never permanently wedge the queue.
            let result = std::panic::catch_unwind(|| preview::thumbnail(&key.path).ok().flatten())
                .ok()
                .flatten();
            let _ = sender.send(ResultMessage { key, result });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(name: &str) -> Entry {
        Entry {
            path: PathBuf::from(name),
            is_dir: false,
            is_symlink: false,
            size: 1,
            modified: None,
        }
    }

    #[test]
    fn pending_is_bounded_and_newly_visible_images_take_priority() {
        let mut manager = ThumbnailManager::default();
        for index in 0..500 {
            manager.request(&image(&format!("{index}.png")));
        }
        assert_eq!(manager.pending.len(), MAX_PENDING);
        assert_eq!(
            manager.pending.front().unwrap().path,
            PathBuf::from("499.png")
        );
        assert_eq!(
            manager.pending.back().unwrap().path,
            PathBuf::from("452.png")
        );
        manager.request(&image("485.png"));
        assert_eq!(manager.pending.len(), MAX_PENDING);
        assert_eq!(
            manager.pending.front().unwrap().path,
            PathBuf::from("485.png")
        );
        manager.clear_pending();
        assert!(!manager.has_pending());
    }

    #[test]
    fn folder_and_links_do_not_request_image_decoding() {
        let mut manager = ThumbnailManager::default();
        let mut entry = image("file.png");
        entry.is_dir = true;
        manager.request(&entry);
        entry.is_dir = false;
        entry.is_symlink = true;
        manager.request(&entry);
        assert!(!manager.has_pending());
    }

    #[test]
    fn completed_thumbnails_evict_old_entries_at_the_memory_budget() {
        let mut manager = ThumbnailManager::default();
        let context = egui::Context::default();
        for index in 0..MAX_CACHED + 10 {
            let key = Key::from_entry(&image(&format!("{index}.png"))).unwrap();
            manager.active = Some(key.clone());
            manager
                .sender
                .send(ResultMessage { key, result: None })
                .unwrap();
            manager.poll(&context);
            assert!(manager.cache.len() <= MAX_CACHED);
        }
        assert_eq!(manager.cache.len(), MAX_CACHED);
        assert!(
            !manager
                .cache
                .contains_key(&Key::from_entry(&image("0.png")).unwrap())
        );
        assert!(
            manager.cache.contains_key(
                &Key::from_entry(&image(&format!("{}.png", MAX_CACHED + 9))).unwrap()
            )
        );
    }
}
