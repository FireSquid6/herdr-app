//! Native streamed browser surfaces. No web content receives a mod/command bridge.
//!
//! Every asynchronous result retains the complete controller identity. This module
//! deliberately does not decide which core owns a workspace; the window supplies it.
pub mod panel;
use anyhow::{Result, bail};
use base64::Engine as _;
use std::sync::{Arc, Weak};

// Stored using the existing mod-block envelope so older app versions can still
// read every terminal/layout; they show a missing-mod placeholder for this slot.
// This is native-only: no JS mod is installed and references never start Chrome.
pub const NATIVE_SLOT: &str = "herdr-native-browser";

pub fn is_outer(tab: &crate::model::Tab) -> bool {
    tab.root.walk().into_iter().any(
        |n| matches!(n.content(), Some(crate::layout::Content::Mod{name,..}) if name==NATIVE_SLOT),
    )
}

/// Closing a pure Browser outer tab detaches it without deleting its workspace
/// identity, even when it was the last tab. Chromium lifetime stays core-owned.
pub fn close_outer(model: &mut crate::model::Model, id: &str) -> bool {
    let Some(tab) = model.tab(id) else {
        return false;
    };
    if !is_outer(tab) || !tab.root.terminals().is_empty() {
        return false;
    }
    let Some(space) = model.space_of(id).map(|s| s.id.clone()) else {
        return false;
    };
    let space = model.space_mut(&space).unwrap();
    space.tabs.retain(|t| t.id != id);
    if space.active.as_deref() == Some(id) {
        space.active = space
            .tabs
            .iter()
            .find(|t| t.settled.is_none())
            .or_else(|| space.tabs.first())
            .map(|t| t.id.clone());
    }
    true
}

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

/// Keep exactly the last validated PNG per streamed surface. Reusing its image
/// avoids PNG inflation, RGBA conversion and a fresh GPUI texture identity for
/// unchanged frames. The displayed snapshot owns the pixels; a weak reference
/// must not prolong their lifetime when it is hidden/closed. No lease/ACK caching.
#[derive(Default)]
pub struct FrameDecoder {
    last: Option<(String, u32, u32, Weak<gpui::RenderImage>)>,
}
impl FrameDecoder {
    pub fn decode(&mut self, event: &serde_json::Value) -> Result<Arc<gpui::RenderImage>> {
        let (data, width, height) = frame_data(event)?;
        if let Some((previous, w, h, image)) = &self.last {
            if *w == width && *h == height && previous == data {
                if let Some(image) = image.upgrade() {
                    return Ok(image);
                }
            }
        }
        let image = decode_png(data, width, height)?;
        self.last = Some((data.to_owned(), width, height, Arc::downgrade(&image)));
        Ok(image)
    }

    pub fn clear(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
fn decode_event(event: &serde_json::Value) -> Result<Arc<gpui::RenderImage>> {
    let (data, width, height) = frame_data(event)?;
    decode_png(data, width, height)
}

fn frame_data(event: &serde_json::Value) -> Result<(&str, u32, u32)> {
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
    Ok((
        event["data"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing PNG data"))?,
        width,
        height,
    ))
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
    #[test]
    fn closing_last_browser_outer_keeps_workspace_and_old_layout_envelope() {
        use crate::{
            layout::{Content, Node},
            model::{Model, Tab},
        };
        let mut model = Model::default();
        let node = Node::block(
            "browser-pane",
            Content::Mod {
                name: NATIVE_SLOT.into(),
                props: serde_json::Value::Null,
            },
        );
        let serialized = serde_json::to_value(&node).unwrap();
        assert_eq!(serialized["content"]["mod"], NATIVE_SLOT);
        let restored: Node = serde_json::from_value(serialized).unwrap();
        model.add_space(
            "ws",
            "Workspace",
            "/fixture",
            Tab::with_pane("browser-tab", "Browser", restored),
        );
        assert!(is_outer(model.active_tab().unwrap()));
        assert!(close_outer(&mut model, "browser-tab"));
        assert_eq!(model.active_space().unwrap().id, "ws");
        assert!(model.active_tab().is_none());
        model.add_tab("ws", Tab::new("terminal-tab", "Shell", "pane", "t1"));
        assert!(!close_outer(&mut model, "terminal-tab"));
        assert_eq!(model.active_tab().unwrap().id, "terminal-tab");
    }

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

    fn frame_event(red: u8) -> serde_json::Value {
        use image::ImageEncoder as _;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[red, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        serde_json::json!({"type":"frame","width":1,"height":1,"scale":1,
            "data":base64::engine::general_purpose::STANDARD.encode(&png)})
    }

    #[test]
    fn unchanged_png_reuses_render_image_but_cache_only_retains_last_frame() {
        let mut decoder = FrameDecoder::default();
        let mut event = frame_event(255);
        let first = decoder.decode(&event).unwrap();
        event["timestamp_ms"] = 200.into();
        let unchanged = decoder.decode(&event).unwrap();
        assert!(Arc::ptr_eq(&first, &unchanged));
        let changed = decoder.decode(&frame_event(0)).unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        let returned = decoder.decode(&event).unwrap();
        assert!(
            !Arc::ptr_eq(&first, &returned),
            "retain only one PNG, not a growing history"
        );
    }

    #[test]
    fn cached_frames_still_validate_metadata_and_failures_do_not_replace_good_pixels() {
        let mut decoder = FrameDecoder::default();
        let event = frame_event(255);
        let first = decoder.decode(&event).unwrap();
        for (key, value) in [
            ("type", serde_json::json!("other")),
            ("width", serde_json::json!(2)),
            ("width", serde_json::json!(4_294_967_297u64)),
            ("height", serde_json::json!(0)),
            ("scale", serde_json::json!(4)),
            ("data", serde_json::json!("not a PNG")),
        ] {
            let mut invalid = event.clone();
            invalid[key] = value;
            assert!(
                decoder.decode(&invalid).is_err(),
                "must validate {key} on cache hits too"
            );
            assert!(Arc::ptr_eq(&first, &decoder.decode(&event).unwrap()));
        }
        let mut other_surface = FrameDecoder::default();
        assert!(!Arc::ptr_eq(&first, &other_surface.decode(&event).unwrap()));
    }

    #[test]
    fn decoder_does_not_keep_discarded_pixels_alive() {
        let mut decoder = FrameDecoder::default();
        let event = frame_event(255);
        let image = decoder.decode(&event).unwrap();
        let weak = Arc::downgrade(&image);
        drop(image); // the visible snapshot was cleared on hide, close or selection change
        assert!(
            weak.upgrade().is_none(),
            "the cache must not own hidden decoded pixels"
        );
        let displayed = decoder.decode(&event).unwrap();
        decoder.clear();
        assert!(
            !Arc::ptr_eq(&displayed, &decoder.decode(&event).unwrap()),
            "clearing an inactive surface also discards its encoded PNG cache"
        );
    }

    #[test]
    #[ignore = "manual CPU microbenchmark; not a native rendering or end-to-end FPS test"]
    fn benchmark_static_frame_decode() {
        use image::ImageEncoder as _;
        use std::{hint::black_box, time::Instant};
        let pixels = image::RgbaImage::from_fn(1280, 720, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8, 255])
        });
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(pixels.as_raw(), 1280, 720, image::ExtendedColorType::Rgba8)
            .unwrap();
        let mut event = serde_json::json!({"type":"frame","width":1280,"height":720,"scale":1,
            "data":base64::engine::general_purpose::STANDARD.encode(&png)});
        const FRAMES: u64 = 30;
        let start = Instant::now();
        let mut displayed = None;
        for i in 0..FRAMES {
            event["timestamp_ms"] = i.into();
            displayed = Some(black_box(decode_event(black_box(&event)).unwrap()));
            black_box(&displayed);
        }
        let uncached = start.elapsed();
        drop(displayed);
        let mut decoder = FrameDecoder::default();
        // Include the initial decode: this is a cold cache, not just string comparisons.
        let start = Instant::now();
        let mut displayed = None;
        for i in 0..FRAMES {
            event["timestamp_ms"] = i.into();
            displayed = Some(black_box(decoder.decode(black_box(&event)).unwrap()));
            black_box(&displayed);
        }
        let cached = start.elapsed();
        drop(displayed);
        eprintln!(
            "static 1280x720 PNG ({} bytes), {FRAMES} frames: uncached={uncached:?}, cached={cached:?}, speedup={:.2}x",
            png.len(),
            uncached.as_secs_f64() / cached.as_secs_f64()
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
