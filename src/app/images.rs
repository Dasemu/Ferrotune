//! Bounded LRU of decoded album art, keyed by URL with a byte budget
//! (architecture §3: default 48 MB).

use std::num::NonZeroUsize;

use iced::widget::image::Handle;
use lru::LruCache;

use crate::net::images::DecodedImage;

pub const DEFAULT_BUDGET: usize = 48 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CachedImage {
    pub handle: Handle,
    pub bytes: usize,
    pub average: [u8; 3],
}

pub struct ImageCache {
    lru: LruCache<String, CachedImage>,
    used: usize,
    budget: usize,
}

impl std::fmt::Debug for ImageCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ImageCache({} items, {} bytes)",
            self.lru.len(),
            self.used
        )
    }
}

impl ImageCache {
    pub fn new(budget: usize) -> Self {
        Self {
            // Entry count is bounded by the byte budget, not by the LRU cap.
            lru: LruCache::new(NonZeroUsize::MIN.saturating_add(4095)),
            used: 0,
            budget,
        }
    }

    pub fn insert(&mut self, url: String, img: DecodedImage) {
        let bytes = img.byte_size();
        if bytes > self.budget {
            return;
        }
        let entry = CachedImage {
            handle: Handle::from_rgba(img.width, img.height, img.rgba),
            bytes,
            average: img.average,
        };
        if let Some((_, old)) = self.lru.push(url, entry) {
            self.used -= old.bytes;
        }
        self.used += bytes;
        while self.used > self.budget {
            match self.lru.pop_lru() {
                Some((_, old)) => self.used -= old.bytes,
                None => break,
            }
        }
    }

    /// Read without touching recency (for `view`).
    pub fn peek(&self, url: &str) -> Option<&CachedImage> {
        self.lru.peek(url)
    }

    /// Mark as recently used.
    pub fn touch(&mut self, url: &str) -> bool {
        self.lru.get(url).is_some()
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.lru.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lru.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(px: u32) -> DecodedImage {
        DecodedImage::from_rgba(px, px, vec![128; (px * px * 4) as usize])
    }

    #[test]
    fn evicts_least_recently_used_within_budget() {
        let one = 10 * 10 * 4;
        let mut c = ImageCache::new(one * 3);
        c.insert("a".into(), img(10));
        c.insert("b".into(), img(10));
        c.insert("c".into(), img(10));
        assert!(c.touch("a"));
        c.insert("d".into(), img(10));
        assert_eq!(c.len(), 3);
        assert!(c.peek("b").is_none(), "b was least recently used");
        assert!(c.peek("a").is_some());
        assert_eq!(c.used_bytes(), one * 3);
        // Re-inserting the same key does not double count.
        c.insert("a".into(), img(10));
        assert_eq!(c.used_bytes(), one * 3);
        // Oversized images are refused.
        c.insert("huge".into(), img(100));
        assert!(c.peek("huge").is_none());
    }
}
