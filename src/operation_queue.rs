//! Bounded, in-memory FIFO for explicit copy requests.
//!
//! Jobs never execute on construction: the UI owns worker dispatch and the
//! queue only stores immutable action snapshots. The queue is deliberately
//! not persisted, because replaying a filesystem mutation after a restart
//! would be unsafe without an independently verified recovery protocol.

use crate::filesystem::Action;
use std::{collections::VecDeque, io};

pub const MAX_QUEUED: usize = 32;

pub struct Job {
    pub id: u64,
    pub label: String,
    pub action: Action,
}

#[derive(Default)]
pub struct OperationQueue {
    pending: VecDeque<Job>,
    next_id: u64,
}

impl OperationQueue {
    pub fn enqueue(&mut self, action: Action) -> io::Result<u64> {
        if self.pending.len() >= MAX_QUEUED {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Transfer queue is full (32 waiting jobs)",
            ));
        }
        let id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Transfer ID limit reached"))?;
        self.next_id = id;
        self.pending.push_back(Job {
            id,
            label: describe(&action),
            action,
        });
        Ok(id)
    }

    pub fn dequeue(&mut self) -> Option<Job> {
        self.pending.pop_front()
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.pending.len();
        self.pending.retain(|job| job.id != id);
        self.pending.len() != before
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Job> {
        self.pending.iter()
    }
}

pub fn describe(action: &Action) -> String {
    match action {
        Action::Copy { sources, .. } => format!("Copy {} item(s)", sources.len()),
        Action::Move { sources, .. } => format!("Move {} item(s)", sources.len()),
        Action::Trash { paths } => format!("Trash {} item(s)", paths.len()),
        Action::Delete { paths } => format!("Delete {} item(s)", paths.len()),
        Action::Link { sources, .. } => format!("Link {} item(s)", sources.len()),
        Action::Rename { .. } => "Rename item".into(),
        Action::BatchRename { changes } => format!("Batch rename {} item(s)", changes.len()),
        Action::NewFolder { .. } => "New folder".into(),
        Action::NewFile { .. } => "New file".into(),
        Action::SetMode { .. } => "Change permissions".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn copy(number: usize) -> Action {
        Action::Copy {
            sources: vec![PathBuf::from(format!("/tmp/src-{number}"))],
            destination: PathBuf::from("/tmp/dest"),
        }
    }

    #[test]
    fn queued_jobs_have_stable_ids_and_fifo_order() {
        let mut queue = OperationQueue::default();
        assert_eq!(queue.enqueue(copy(1)).unwrap(), 1);
        assert_eq!(queue.enqueue(copy(2)).unwrap(), 2);
        assert_eq!(queue.enqueue(copy(3)).unwrap(), 3);
        assert_eq!(queue.dequeue().unwrap().id, 1);
        assert_eq!(queue.dequeue().unwrap().id, 2);
        assert_eq!(queue.dequeue().unwrap().id, 3);
        assert!(queue.dequeue().is_none());
        assert_eq!(queue.enqueue(copy(4)).unwrap(), 4);
    }

    #[test]
    fn queued_jobs_can_be_removed_without_running() {
        let mut queue = OperationQueue::default();
        let first = queue.enqueue(copy(1)).unwrap();
        let second = queue.enqueue(copy(2)).unwrap();
        assert!(queue.remove(first));
        assert!(!queue.remove(first));
        assert_eq!(queue.dequeue().unwrap().id, second);
        assert!(queue.is_empty());
    }

    #[test]
    fn capacity_rejects_extra_jobs_without_dropping_existing_work() {
        let mut queue = OperationQueue::default();
        for index in 0..MAX_QUEUED {
            queue.enqueue(copy(index)).unwrap();
        }
        assert_eq!(queue.len(), MAX_QUEUED);
        assert_eq!(
            queue.enqueue(copy(100)).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(queue.dequeue().unwrap().id, 1);
        assert_eq!(queue.enqueue(copy(101)).unwrap(), 33);
    }
}
