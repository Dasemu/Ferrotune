//! Paging helpers for the Web API's offset- and cursor-based collections.

/// Offset of the next page, or `None` when the collection is exhausted.
///
/// We compute this from `offset + len` instead of following the `next`
/// URL so that pages work identically against a mock server.
pub fn next_offset(offset: u32, len: usize, total: u32) -> Option<u32> {
    let next = offset.saturating_add(len as u32);
    (len > 0 && next < total).then_some(next)
}

/// Accumulates pages up to a cap, used for library lists.
#[derive(Debug, Clone)]
pub struct Collector<T> {
    pub items: Vec<T>,
    pub cap: usize,
}

impl<T> Collector<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            items: Vec::new(),
            cap,
        }
    }

    /// Append a page; returns true when another page should be requested.
    pub fn push(&mut self, page: Vec<T>, more_available: bool) -> bool {
        let room = self.cap.saturating_sub(self.items.len());
        let full_page = page.len() <= room;
        self.items.extend(page.into_iter().take(room));
        more_available && full_page && self.items.len() < self.cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_offset_cases() {
        assert_eq!(next_offset(0, 50, 120), Some(50));
        assert_eq!(next_offset(50, 50, 120), Some(100));
        assert_eq!(next_offset(100, 20, 120), None);
        assert_eq!(next_offset(0, 0, 120), None, "empty page must stop");
        assert_eq!(next_offset(0, 10, 10), None);
    }

    #[test]
    fn collector_caps() {
        let mut c = Collector::new(5);
        assert!(c.push(vec![1, 2, 3], true));
        assert!(!c.push(vec![4, 5, 6], true));
        assert_eq!(c.items, vec![1, 2, 3, 4, 5]);
        let mut c = Collector::new(10);
        assert!(!c.push(vec![1], false));
    }
}
