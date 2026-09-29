//! Append-only storage in fixed-size blocks. Unlike a `Vec`, growing never copies what
//! is already stored and never reserves double the space, which matters when a tree
//! holds hundreds of millions of nodes.

use std::ops::{Index, IndexMut, Range};

const ITEMS_PER_BLOCK: usize = 1 << 16;
const BYTES_PER_BLOCK: usize = 1 << 20;

/// A list that grows in blocks of 65,536 items.
#[derive(Debug)]
pub struct BlockVec<T> {
    blocks: Vec<Vec<T>>,
    len: usize,
}

impl<T> BlockVec<T> {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn push(&mut self, item: T) {
        if self.len.is_multiple_of(ITEMS_PER_BLOCK) {
            self.blocks.push(Vec::with_capacity(ITEMS_PER_BLOCK));
        }
        if let Some(block) = self.blocks.last_mut() {
            block.push(item);
        }
        self.len += 1;
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.blocks.iter().flatten()
    }
}

impl<T> Index<usize> for BlockVec<T> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        &self.blocks[index / ITEMS_PER_BLOCK][index % ITEMS_PER_BLOCK]
    }
}

impl<T> IndexMut<usize> for BlockVec<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        &mut self.blocks[index / ITEMS_PER_BLOCK][index % ITEMS_PER_BLOCK]
    }
}

/// Text stored back to back in 1 MiB blocks. A string never spans two blocks, so each
/// one can be returned as a single `&str`.
#[derive(Debug, Default)]
pub struct TextArena {
    blocks: Vec<String>,
}

impl TextArena {
    /// Stores `text` (at most 1 MiB) and returns its position for [`TextArena::get`].
    pub fn push(&mut self, text: &str) -> Range<usize> {
        assert!(text.len() <= BYTES_PER_BLOCK, "text longer than a block");
        let fits = self
            .blocks
            .last()
            .is_some_and(|block| block.len() + text.len() <= BYTES_PER_BLOCK);
        if !fits {
            self.blocks.push(String::with_capacity(BYTES_PER_BLOCK));
        }
        let block_index = self.blocks.len() - 1;
        let block = &mut self.blocks[block_index];
        let start = block_index * BYTES_PER_BLOCK + block.len();
        block.push_str(text);
        start..start + text.len()
    }

    pub fn get(&self, range: Range<usize>) -> &str {
        if range.is_empty() {
            return "";
        }
        let block = &self.blocks[range.start / BYTES_PER_BLOCK];
        let offset = range.start % BYTES_PER_BLOCK;
        &block[offset..offset + range.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_vec_indexes_across_blocks() {
        let mut items = BlockVec::new();
        for value in 0..(ITEMS_PER_BLOCK as u32 * 2 + 5) {
            items.push(value);
        }
        assert_eq!(items.len(), ITEMS_PER_BLOCK * 2 + 5);
        assert_eq!(items[ITEMS_PER_BLOCK + 3], ITEMS_PER_BLOCK as u32 + 3);
        items[7] = 99;
        assert_eq!(items[7], 99);
        assert_eq!(items.iter().count(), items.len());
    }

    #[test]
    fn text_never_spans_blocks() {
        let mut text = TextArena::default();
        let first = text.push(&"a".repeat(BYTES_PER_BLOCK - 3));
        let second = text.push("hello");
        let third = text.push("");

        assert_eq!(text.get(first).len(), BYTES_PER_BLOCK - 3);
        assert_eq!(text.get(second.clone()), "hello");
        assert_eq!(second.start, BYTES_PER_BLOCK);
        assert_eq!(text.get(third), "");
    }
}
