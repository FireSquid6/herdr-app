//! Native streamed browser surfaces. No web content receives a mod/command bridge.
//!
//! Every asynchronous result retains the complete controller identity. This module
//! deliberately does not decide which core owns a workspace; the window supplies it.
pub mod panel;
use anyhow::{Result, bail};
use base64::Engine as _;
use std::sync::Arc;

pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_777_216;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub machine: String,
    pub workspace: String,
    pub target: String,
    pub generation: String,
    pub devtools: bool,
}

/// Check the IHDR before invoking any allocating decompressor. The PNG decoder
/// subsequently validates CRCs, chunk order and the actual decompressed content.
fn dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    if bytes.len() < 33
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || bytes[8..12] != 13u32.to_be_bytes()
        || &bytes[12..16] != b"IHDR"
    {
        bail!("browser frame is not a PNG with an IHDR");
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into()?);
    if w == 0 || h == 0 || w > 4096 || h > 4096 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        bail!("browser frame exceeds the decoded pixel limit");
    }
    Ok((w, h))
}

pub fn decode_png(data: &str, width: u32, height: u32) -> Result<Arc<gpui::RenderImage>> {
    if data.len() > MAX_MESSAGE {
        bail!("browser frame exceeds message limit");
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    if dimensions(&bytes)? != (width, height) {
        bail!("browser frame dimensions disagree with PNG");
    }
    let mut decoder =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    decoder.limits(limits);
    let mut rgba = decoder.decode()?.into_rgba8();
    // GPUI image textures are BGRA, as in terminal image rendering.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(gpui::RenderImage::new(smallvec::smallvec![
        image::Frame::new(rgba)
    ])))
}

pub fn decode_event(event: &serde_json::Value) -> Result<Arc<gpui::RenderImage>> {
    if event["type"] != "frame" {
        bail!("unexpected browser event type");
    }
    let width = u32::try_from(
        event["width"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing frame width"))?,
    )?;
    let height = u32::try_from(
        event["height"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing frame height"))?,
    )?;
    let scale = event["scale"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("missing frame scale"))?;
    if !scale.is_finite() || !(0.5..=3.0).contains(&scale) {
        bail!("invalid frame scale");
    }
    decode_png(
        event["data"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing PNG data"))?,
        width,
        height,
    )
}

/// Sequence validation occurs before decode and again before texture commit.
/// Revoking replaces this value, making old decoded results uncommittable.
#[derive(Default)]
pub struct SurfaceState {
    pub identity: Option<Identity>,
    pub consumed: u64,
}
impl SurfaceState {
    pub fn accepts(&self, identity: &Identity, sequence: u64) -> bool {
        self.identity.as_ref() == Some(identity) && sequence > self.consumed
    }
    pub fn commit(&mut self, identity: &Identity, sequence: u64) -> bool {
        if !self.accepts(identity, sequence) {
            return false;
        }
        self.consumed = sequence;
        true
    }
    pub fn revoke(&mut self) {
        self.identity = None;
        self.consumed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> Identity {
        Identity {
            machine: "host-a".into(),
            workspace: "ws".into(),
            target: "page".into(),
            generation: "incarnation:1".into(),
            devtools: false,
        }
    }
    #[test]
    fn stale_decodes_cannot_commit_after_revoke_or_machine_switch() {
        let id = identity();
        let mut state = SurfaceState {
            identity: Some(id.clone()),
            consumed: 0,
        };
        assert!(state.commit(&id, 1));
        assert!(!state.commit(&id, 1));
        let mut remote = id.clone();
        remote.machine = "host-b".into();
        assert!(!state.commit(&remote, 2));
        state.revoke();
        assert!(!state.commit(&id, 2));
        state.identity = Some(remote.clone());
        assert!(!state.commit(&id, 3));
        assert!(state.commit(&remote, 1));
    }
    #[test]
    fn valid_png_decodes_but_metadata_overflow_and_truncation_do_not() {
        use image::ImageEncoder as _;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        let mut event =
            serde_json::json!({"type":"frame","width":1,"height":1,"scale":1,"data":data});
        assert!(decode_event(&event).is_ok());
        event["width"] = serde_json::json!(4_294_967_297u64);
        assert!(
            decode_event(&event).is_err(),
            "must not narrow overflowing metadata into a matching PNG width"
        );
        event["width"] = 1.into();
        event["scale"] = 4.into();
        assert!(decode_event(&event).is_err());
        assert!(decode_png(&data, 2, 1).is_err());
        assert!(
            decode_png(
                &base64::engine::general_purpose::STANDARD.encode(&png[..33]),
                1,
                1
            )
            .is_err()
        );
    }

    #[test]
    fn png_dimensions_rejected_before_decompression() {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(4097u32.to_be_bytes());
        bytes.extend(1u32.to_be_bytes());
        bytes.resize(33, 0);
        assert!(dimensions(&bytes).is_err());
        bytes[16..20].copy_from_slice(&4096u32.to_be_bytes());
        assert_eq!(dimensions(&bytes).unwrap(), (4096, 1));
        bytes[20..24].copy_from_slice(&0u32.to_be_bytes());
        assert!(dimensions(&bytes).is_err());
        assert!(decode_png(&"A".repeat(MAX_MESSAGE + 1), 1, 1).is_err());
    }
}
