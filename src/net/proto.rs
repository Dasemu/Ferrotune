//! Minimal protobuf codec for `spotify.collection.proto.v2` (collection2v2).
//!
//! librespot-protocol ships the `.proto` file but does not compile it, and
//! the messages are tiny, so they are encoded by hand here instead of adding
//! a protobuf code generator. Field numbers follow
//! `librespot-protocol/proto/collection2v2.proto` (Spotify 1.2.52.442).

use crate::error::{Error, Result};

/// `CollectionItem { uri = 1; added_at = 2; is_removed = 3; context_uri = 4 }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CollectionItem {
    pub uri: String,
    pub added_at: i32,
    pub is_removed: bool,
}

/// `PageResponse { items = 1; next_page_token = 2; sync_token = 3 }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageResponse {
    pub items: Vec<CollectionItem>,
    pub next_page_token: String,
    pub sync_token: String,
}

/// `DeltaResponse { delta_update_possible = 1; items = 2; sync_token = 3 }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeltaResponse {
    pub delta_update_possible: bool,
    pub items: Vec<CollectionItem>,
    pub sync_token: String,
}

// ------------------------------------------------------------------ encode

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_str(out: &mut Vec<u8>, field: u32, s: &str) {
    if s.is_empty() {
        return; // proto3 default
    }
    put_varint(out, u64::from(field << 3 | 2));
    put_varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn put_int(out: &mut Vec<u8>, field: u32, v: i64) {
    if v == 0 {
        return;
    }
    put_varint(out, u64::from(field << 3));
    // int32/int64/bool use plain (two's complement) varints.
    put_varint(out, v as u64);
}

fn put_msg(out: &mut Vec<u8>, field: u32, msg: &[u8]) {
    put_varint(out, u64::from(field << 3 | 2));
    put_varint(out, msg.len() as u64);
    out.extend_from_slice(msg);
}

/// `PageRequest { username = 1; set = 2; pagination_token = 3; limit = 4 }`
pub fn page_request(username: &str, set: &str, token: &str, limit: i32) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, username);
    put_str(&mut out, 2, set);
    put_str(&mut out, 3, token);
    put_int(&mut out, 4, i64::from(limit));
    out
}

/// `DeltaRequest { username = 1; set = 2; last_sync_token = 3 }`
pub fn delta_request(username: &str, set: &str, sync_token: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, username);
    put_str(&mut out, 2, set);
    put_str(&mut out, 3, sync_token);
    out
}

pub fn encode_item(item: &CollectionItem) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, &item.uri);
    put_int(&mut out, 2, i64::from(item.added_at));
    put_int(&mut out, 3, i64::from(item.is_removed));
    out
}

/// `WriteRequest { username = 1; set = 2; items = 3; client_update_id = 4 }`
pub fn write_request(
    username: &str,
    set: &str,
    items: &[CollectionItem],
    client_update_id: &str,
) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, username);
    put_str(&mut out, 2, set);
    for it in items {
        put_msg(&mut out, 3, &encode_item(it));
    }
    put_str(&mut out, 4, client_update_id);
    out
}

/// Encode a page response (used by tests and fixtures).
pub fn encode_page(p: &PageResponse) -> Vec<u8> {
    let mut out = Vec::new();
    for it in &p.items {
        put_msg(&mut out, 1, &encode_item(it));
    }
    put_str(&mut out, 2, &p.next_page_token);
    put_str(&mut out, 3, &p.sync_token);
    out
}

/// Encode a delta response (used by tests and fixtures).
pub fn encode_delta(d: &DeltaResponse) -> Vec<u8> {
    let mut out = Vec::new();
    put_int(&mut out, 1, i64::from(d.delta_update_possible));
    for it in &d.items {
        put_msg(&mut out, 2, &encode_item(it));
    }
    put_str(&mut out, 3, &d.sync_token);
    out
}

// ------------------------------------------------------------------ decode

enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Skipped,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

fn bad() -> Error {
    Error::Protocol("malformed collection protobuf".into())
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.pos).ok_or_else(bad)?;
            self.pos += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(bad())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(bad)?;
        let s = self.buf.get(self.pos..end).ok_or_else(bad)?;
        self.pos = end;
        Ok(s)
    }

    /// Next `(field, value)`, skipping wire types we never use.
    fn next(&mut self) -> Result<Option<(u32, Value<'a>)>> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = u32::try_from(key >> 3).map_err(|_| bad())?;
        let value = match key & 7 {
            0 => Value::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Value::Skipped
            }
            2 => {
                let len = usize::try_from(self.varint()?).map_err(|_| bad())?;
                Value::Bytes(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Value::Skipped
            }
            _ => return Err(bad()),
        };
        Ok(Some((field, value)))
    }
}

fn utf8(b: &[u8]) -> Result<String> {
    String::from_utf8(b.to_vec()).map_err(|_| bad())
}

pub fn decode_item(buf: &[u8]) -> Result<CollectionItem> {
    let mut r = Reader::new(buf);
    let mut item = CollectionItem::default();
    while let Some((field, v)) = r.next()? {
        match (field, v) {
            (1, Value::Bytes(b)) => item.uri = utf8(b)?,
            (2, Value::Varint(n)) => item.added_at = n as i32,
            (3, Value::Varint(n)) => item.is_removed = n != 0,
            _ => {}
        }
    }
    Ok(item)
}

pub fn decode_page(buf: &[u8]) -> Result<PageResponse> {
    let mut r = Reader::new(buf);
    let mut p = PageResponse::default();
    while let Some((field, v)) = r.next()? {
        match (field, v) {
            (1, Value::Bytes(b)) => p.items.push(decode_item(b)?),
            (2, Value::Bytes(b)) => p.next_page_token = utf8(b)?,
            (3, Value::Bytes(b)) => p.sync_token = utf8(b)?,
            _ => {}
        }
    }
    Ok(p)
}

pub fn decode_delta(buf: &[u8]) -> Result<DeltaResponse> {
    let mut r = Reader::new(buf);
    let mut d = DeltaResponse::default();
    while let Some((field, v)) = r.next()? {
        match (field, v) {
            (1, Value::Varint(n)) => d.delta_update_possible = n != 0,
            (2, Value::Bytes(b)) => d.items.push(decode_item(b)?),
            (3, Value::Bytes(b)) => d.sync_token = utf8(b)?,
            _ => {}
        }
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(uri: &str, at: i32, removed: bool) -> CollectionItem {
        CollectionItem {
            uri: uri.into(),
            added_at: at,
            is_removed: removed,
        }
    }

    #[test]
    fn page_request_matches_reference_encoding() {
        // Hand-checked wire bytes: field 1 "u", field 2 "collection", field 4 = 100.
        let b = page_request("u", "collection", "", 100);
        assert_eq!(
            b,
            [
                &[0x0a, 0x01, b'u', 0x12, 0x0a][..],
                b"collection",
                &[0x20, 100]
            ]
            .concat()
        );
    }

    #[test]
    fn page_roundtrip() {
        let p = PageResponse {
            items: vec![
                item("spotify:track:a", 1_700_000_000, false),
                item("spotify:album:b", 5, false),
            ],
            next_page_token: "next".into(),
            sync_token: "sync".into(),
        };
        assert_eq!(decode_page(&encode_page(&p)).unwrap(), p);
    }

    #[test]
    fn delta_roundtrip_and_removed_flag() {
        let d = DeltaResponse {
            delta_update_possible: true,
            items: vec![item("spotify:track:a", 9, true)],
            sync_token: "s2".into(),
        };
        assert_eq!(decode_delta(&encode_delta(&d)).unwrap(), d);
    }

    #[test]
    fn unknown_fields_are_skipped_and_garbage_is_an_error() {
        let mut b = encode_page(&PageResponse {
            items: vec![item("spotify:track:x", 1, false)],
            ..Default::default()
        });
        // Field 9, fixed32 and fixed64 values: ignored.
        b.extend_from_slice(&[0x4d, 1, 2, 3, 4, 0x49, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decode_page(&b).unwrap().items.len(), 1);
        assert!(decode_page(&[0x0a, 0x05, b'a']).is_err(), "truncated");
        assert!(decode_page(&[0xff; 11]).is_err(), "overlong varint");
    }

    #[test]
    fn write_request_contains_items() {
        let b = write_request("u", "collection", &[item("spotify:track:a", 1, true)], "id");
        let mut r = Reader::new(&b);
        let mut items = 0;
        while let Some((f, v)) = r.next().unwrap() {
            if let (3, Value::Bytes(m)) = (f, v) {
                let it = decode_item(m).unwrap();
                assert!(it.is_removed);
                items += 1;
            }
        }
        assert_eq!(items, 1);
    }
}
