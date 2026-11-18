//! Album-art download and decode. Decoding (and downscaling) happens off
//! the UI thread so the renderer only gets ready RGBA buffers whose exact
//! size is known to the LRU cache.

use std::fmt;

use crate::error::{Error, Result};

/// A decoded, downscaled RGBA image plus its average colour (used for the
/// gradient header behind playlists / albums).
#[derive(Clone)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub average: [u8; 3],
}

impl fmt::Debug for DecodedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DecodedImage({}x{})", self.width, self.height)
    }
}

impl DecodedImage {
    pub fn byte_size(&self) -> usize {
        self.rgba.len()
    }

    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        let average = average_color(&rgba);
        Self {
            width,
            height,
            rgba,
            average,
        }
    }
}

/// Mean RGB of an RGBA buffer, sampled for speed.
pub fn average_color(rgba: &[u8]) -> [u8; 3] {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for px in rgba.chunks_exact(4).step_by(7) {
        r += u64::from(px[0]);
        g += u64::from(px[1]);
        b += u64::from(px[2]);
        n += 1;
    }
    if n == 0 {
        return [83, 83, 83];
    }
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

/// Decode JPEG/PNG bytes and downscale to at most `max_px` on the long side.
pub fn decode(bytes: &[u8], max_px: u32) -> Result<DecodedImage> {
    let img = image::load_from_memory(bytes).map_err(|e| Error::Protocol(format!("image: {e}")))?;
    let img = if img.width() > max_px || img.height() > max_px {
        img.thumbnail(max_px, max_px)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(DecodedImage::from_rgba(w, h, rgba.into_raw()))
}

pub async fn fetch(http: &reqwest::Client, url: &str, max_px: u32) -> Result<DecodedImage> {
    let resp = http.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(Error::Http {
            status: resp.status().as_u16(),
            message: "image".into(),
        });
    }
    let bytes = resp.bytes().await?;
    tokio::task::spawn_blocking(move || decode(&bytes, max_px))
        .await
        .map_err(|_| Error::Cancelled)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_png_and_downscale() {
        let mut buf = Vec::new();
        let img = image::RgbaImage::from_pixel(400, 200, image::Rgba([200, 10, 10, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        let d = decode(&buf, 100).unwrap();
        assert_eq!((d.width, d.height), (100, 50));
        assert_eq!(d.byte_size(), 100 * 50 * 4);
        assert_eq!(d.average, [200, 10, 10]);
        assert!(decode(b"not an image", 100).is_err());
    }
}
