//! Paging helpers for offset-paged track lists (playlists, Liked Songs).

/// Offset of the next page, or `None` when the collection is exhausted.
///
/// We compute this from `offset + len` instead of following the `next`
/// URL so that pages work identically against a mock server.
pub fn next_offset(offset: u32, len: usize, total: u32) -> Option<u32> {
    let next = offset.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
    (len > 0 && next < total).then_some(next)
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
}
