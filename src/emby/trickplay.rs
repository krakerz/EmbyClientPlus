//! Seek-bar thumbnails ("trickplay"): Emby serves them as a Roku BIF file
//! once the server has extracted thumbnails for a video. Without one,
//! chapter images stand in.

use anyhow::{Result, bail};

use super::EmbyClient;

const MAGIC: [u8; 8] = [0x89, b'B', b'I', b'F', 0x0d, 0x0a, 0x1a, 0x0a];
const HEADER: usize = 64;
/// Width the thumbnails are asked for (Emby's own default).
pub const THUMB_WIDTH: u32 = 320;

/// Parsed BIF: JPEG frames, each from its timestamp until the next.
#[derive(Debug)]
pub struct Thumbnails {
    /// (start seconds, byte range in `data`), in order.
    frames: Vec<(f64, std::ops::Range<usize>)>,
    data: Vec<u8>,
}

impl Thumbnails {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        if data.len() < HEADER || data[..8] != MAGIC {
            bail!("not a BIF file");
        }
        let word = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
        };
        let count = word(12).unwrap_or(0) as usize;
        // Timestamps count in this many milliseconds (0 means 1000).
        let unit = match word(16).unwrap_or(0) {
            0 => 1000,
            ms => ms,
        };
        let mut frames = Vec::with_capacity(count);
        for i in 0..count {
            let entry = HEADER + i * 8;
            let (Some(timestamp), Some(start), Some(end)) =
                (word(entry), word(entry + 4), word(entry + 12))
            else {
                bail!("BIF index is cut short");
            };
            let (start, end) = (start as usize, end as usize);
            if timestamp == u32::MAX || start >= end || end > data.len() {
                break;
            }
            frames.push((f64::from(timestamp) * f64::from(unit) / 1000.0, start..end));
        }
        if frames.is_empty() {
            bail!("BIF has no images");
        }
        Ok(Thumbnails { frames, data })
    }

    /// The frame showing at `seconds`: (its index, JPEG bytes).
    pub fn at(&self, seconds: f64) -> (usize, &[u8]) {
        let index = self
            .frames
            .partition_point(|(start, _)| *start <= seconds)
            .saturating_sub(1);
        (index, &self.data[self.frames[index].1.clone()])
    }
}

impl EmbyClient {
    /// The video's BIF, or `None` when the server has no thumbnails for it.
    pub async fn trickplay(
        &self,
        item_id: &str,
        media_source_id: &str,
    ) -> Result<Option<Thumbnails>> {
        let path = format!(
            "/emby/Videos/{item_id}/index.bif?Width={THUMB_WIDTH}&MediaSourceId={media_source_id}"
        );
        match self.fetch_bytes(&path).await {
            Ok(bytes) => Thumbnails::parse(bytes).map(Some),
            // Not extracted (404) or not available: chapter images instead.
            Err(e) => {
                tracing::debug!("no trickplay thumbnails: {e:#}");
                Ok(None)
            }
        }
    }

    /// A chapter's image, scaled to the thumbnail width.
    pub async fn chapter_image(&self, item_id: &str, index: usize, tag: &str) -> Result<Vec<u8>> {
        self.fetch_bytes(&format!(
            "/emby/Items/{item_id}/Images/Chapter/{index}?maxWidth={THUMB_WIDTH}&tag={tag}&quality=85"
        ))
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bif(timestamps: &[u32], unit: u32) -> Vec<u8> {
        let mut data = MAGIC.to_vec();
        data.extend(0u32.to_le_bytes());
        data.extend((timestamps.len() as u32).to_le_bytes());
        data.extend(unit.to_le_bytes());
        data.resize(HEADER, 0);
        let images_at = HEADER + (timestamps.len() + 1) * 8;
        for (i, t) in timestamps.iter().enumerate() {
            data.extend(t.to_le_bytes());
            data.extend(((images_at + i * 3) as u32).to_le_bytes());
        }
        data.extend(u32::MAX.to_le_bytes());
        data.extend(((images_at + timestamps.len() * 3) as u32).to_le_bytes());
        for i in 0..timestamps.len() {
            data.extend([i as u8; 3]);
        }
        data
    }

    #[test]
    fn frames_are_found_by_time() {
        let thumbs = Thumbnails::parse(bif(&[0, 10, 20], 1000)).unwrap();
        assert_eq!(thumbs.at(0.0), (0, &[0u8, 0, 0][..]));
        assert_eq!(thumbs.at(9.9).0, 0);
        assert_eq!(thumbs.at(10.0), (1, &[1u8, 1, 1][..]));
        assert_eq!(thumbs.at(999.0).0, 2);
    }

    #[test]
    fn timestamp_unit_defaults_to_seconds() {
        let thumbs = Thumbnails::parse(bif(&[0, 5], 0)).unwrap();
        assert_eq!(thumbs.at(5.0).0, 1);
        let fine = Thumbnails::parse(bif(&[0, 5], 2000)).unwrap();
        assert_eq!(fine.at(9.0).0, 0);
        assert_eq!(fine.at(10.0).0, 1);
    }

    #[test]
    fn junk_is_rejected() {
        assert!(Thumbnails::parse(vec![0; 80]).is_err());
        assert!(Thumbnails::parse(bif(&[], 1000)).is_err());
    }
}
