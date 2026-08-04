// Tree-walk analyses over a completed scan result. Pure functions; no Win32.

use crate::types::{FileEntry, FolderNode};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

// A file pinned to its owning folder, returned by the analysis queries. The
// path is reconstructed by the caller via `folder.full_path` + `file.name` so
// we keep the heap entries cheap.
pub struct FileHit<'a> {
    pub file: &'a FileEntry,
    pub folder: &'a FolderNode,
}

// BinaryHeap is a max-heap. We want the N *largest* files, so we put a min-heap
// in there of size N (smallest at the top) — every incoming file > current min
// pops the min and pushes itself. At the end, we drain and sort descending.
struct HeapNode<'a> {
    size: i64,
    file: &'a FileEntry,
    folder: &'a FolderNode,
}

impl<'a> PartialEq for HeapNode<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size
    }
}
impl<'a> Eq for HeapNode<'a> {}
impl<'a> PartialOrd for HeapNode<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<'a> Ord for HeapNode<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed so BinaryHeap behaves as a min-heap.
        other.size.cmp(&self.size)
    }
}

pub fn top_n_files(root: &FolderNode, n: usize) -> Vec<FileHit<'_>> {
    if n == 0 {
        return Vec::new();
    }

    let mut heap: BinaryHeap<HeapNode<'_>> = BinaryHeap::with_capacity(n + 1);
    let mut stack: Vec<&FolderNode> = Vec::with_capacity(64);
    stack.push(root);

    while let Some(node) = stack.pop() {
        for f in &node.files {
            if heap.len() < n {
                heap.push(HeapNode {
                    size: f.size,
                    file: f,
                    folder: node,
                });
            } else if let Some(top) = heap.peek() {
                if f.size > top.size {
                    heap.pop();
                    heap.push(HeapNode {
                        size: f.size,
                        file: f,
                        folder: node,
                    });
                }
            }
        }
        for c in &node.children {
            stack.push(c);
        }
    }

    let mut out: Vec<FileHit<'_>> = heap
        .into_iter()
        .map(|h| FileHit {
            file: h.file,
            folder: h.folder,
        })
        .collect();
    out.sort_by_key(|h| std::cmp::Reverse(h.file.size));
    out
}

// Oldest-N by last-modified. NTFS disables last-access updates by default
// (since Vista), so mtime is the practical "least used" proxy. Files with
// last_modified_ft == 0 (no mtime captured, e.g. on access failure) are
// skipped so they don't fake-pin as "oldest".
struct OldestHeapNode<'a> {
    mtime: i64,
    file: &'a FileEntry,
    folder: &'a FolderNode,
}

impl<'a> PartialEq for OldestHeapNode<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.mtime == other.mtime
    }
}
impl<'a> Eq for OldestHeapNode<'a> {}
impl<'a> PartialOrd for OldestHeapNode<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<'a> Ord for OldestHeapNode<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is max-heap by default — keep the newest at the top so we
        // can pop it to make room for an older candidate.
        self.mtime.cmp(&other.mtime)
    }
}

pub fn oldest_n_files(root: &FolderNode, n: usize) -> Vec<FileHit<'_>> {
    if n == 0 {
        return Vec::new();
    }

    let mut heap: BinaryHeap<OldestHeapNode<'_>> = BinaryHeap::with_capacity(n + 1);
    let mut stack: Vec<&FolderNode> = Vec::with_capacity(64);
    stack.push(root);

    while let Some(node) = stack.pop() {
        for f in &node.files {
            if f.last_modified_ft == 0 {
                continue;
            }
            if heap.len() < n {
                heap.push(OldestHeapNode {
                    mtime: f.last_modified_ft,
                    file: f,
                    folder: node,
                });
            } else if let Some(top) = heap.peek() {
                if f.last_modified_ft < top.mtime {
                    heap.pop();
                    heap.push(OldestHeapNode {
                        mtime: f.last_modified_ft,
                        file: f,
                        folder: node,
                    });
                }
            }
        }
        for c in &node.children {
            stack.push(c);
        }
    }

    let mut out: Vec<FileHit<'_>> = heap
        .into_iter()
        .map(|h| FileHit {
            file: h.file,
            folder: h.folder,
        })
        .collect();
    out.sort_by_key(|a| a.file.last_modified_ft);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: i64, mtime: i64) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            size,
            last_modified_ft: mtime,
        }
    }

    // root(10MB, [a.bin]) -> child(1..) -> grandchild(...)
    fn sample_tree() -> FolderNode {
        let grandchild = FolderNode {
            name: "gc".into(),
            files: vec![file("huge.iso", 9_000, 500), file("tiny.txt", 5, 100)],
            ..Default::default()
        };
        let child = FolderNode {
            name: "c".into(),
            files: vec![file("mid.dat", 3_000, 300)],
            children: vec![grandchild],
            ..Default::default()
        };
        FolderNode {
            name: "root".into(),
            files: vec![file("small.log", 50, 200), file("big.zip", 7_000, 400)],
            children: vec![child],
            ..Default::default()
        }
    }

    #[test]
    fn top_n_returns_largest_in_descending_order() {
        let root = sample_tree();
        let hits = top_n_files(&root, 3);
        let sizes: Vec<i64> = hits.iter().map(|h| h.file.size).collect();
        assert_eq!(sizes, vec![9_000, 7_000, 3_000]);
    }

    #[test]
    fn top_n_zero_is_empty() {
        assert!(top_n_files(&sample_tree(), 0).is_empty());
    }

    #[test]
    fn top_n_larger_than_file_count_returns_all_sorted() {
        let root = sample_tree();
        let hits = top_n_files(&root, 100);
        assert_eq!(hits.len(), 5); // 5 files total across the tree
                                   // Fully sorted descending.
        let sizes: Vec<i64> = hits.iter().map(|h| h.file.size).collect();
        assert_eq!(sizes, vec![9_000, 7_000, 3_000, 50, 5]);
    }

    #[test]
    fn top_n_pins_correct_owning_folder() {
        let root = sample_tree();
        let hits = top_n_files(&root, 1);
        assert_eq!(hits[0].file.name, "huge.iso");
        assert_eq!(hits[0].folder.name, "gc");
    }

    #[test]
    fn oldest_n_returns_smallest_mtime_first() {
        let root = sample_tree();
        let hits = oldest_n_files(&root, 3);
        let mtimes: Vec<i64> = hits.iter().map(|h| h.file.last_modified_ft).collect();
        assert_eq!(mtimes, vec![100, 200, 300]);
    }

    #[test]
    fn oldest_n_skips_zero_mtime() {
        let root = FolderNode {
            files: vec![file("no-time.tmp", 10, 0), file("dated.txt", 20, 42)],
            ..Default::default()
        };
        let hits = oldest_n_files(&root, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file.name, "dated.txt");
    }

    #[test]
    fn empty_tree_yields_no_hits() {
        let root = FolderNode::default();
        assert!(top_n_files(&root, 5).is_empty());
        assert!(oldest_n_files(&root, 5).is_empty());
    }
}
