//! Pure callback/UI boundary. No CEF pointers or GUI objects cross it.
use std::sync::Mutex;

#[derive(Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub serial: u64,
}

#[derive(Default)]
struct State {
    latest: Option<Frame>,
    closed: bool,
    serial: u64,
    replaced: u64,
}

pub struct Mailbox {
    state: Mutex<State>,
    pub wake: async_channel::Receiver<()>,
    sender: async_channel::Sender<()>,
}

impl Default for Mailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Mailbox {
    pub fn new() -> Self {
        let (sender, wake) = async_channel::bounded(1);
        Self {
            state: Mutex::new(State::default()),
            sender,
            wake,
        }
    }

    /// Copy inside the callback, under one lock so concurrent producers cannot
    /// allocate unbounded full frames. Poison/invalid/late callbacks fail closed.
    pub fn publish(&self, width: i32, height: i32, bytes: &[u8]) -> bool {
        let Some(len) = frame_len(width, height) else {
            return false;
        };
        if bytes.len() != len {
            return false;
        }
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.closed {
            return false;
        }
        state.serial += 1;
        let frame = Frame {
            width: width as u32,
            height: height as u32,
            bgra: bytes.to_vec(),
            serial: state.serial,
        };
        if state.latest.replace(frame).is_some() {
            state.replaced += 1;
        }
        drop(state);
        let _ = self.sender.try_send(());
        true
    }

    pub fn take(&self) -> Option<Frame> {
        self.state.lock().ok()?.latest.take()
    }

    pub fn counts(&self) -> (u64, u64) {
        self.state
            .lock()
            .map(|s| (s.serial, s.replaced))
            .unwrap_or_default()
    }

    pub fn close(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.closed = true;
            s.latest = None;
        }
        self.sender.close();
    }
}

pub fn frame_len(width: i32, height: i32) -> Option<usize> {
    if !(1..=4096).contains(&width) || !(1..=4096).contains(&height) {
        return None;
    }
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub width: i32,
    pub height: i32,
    pub scale: f32,
}
impl Default for Geometry {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            scale: 1.0,
        }
    }
}
impl Geometry {
    pub fn local_point(self, x: f32, y: f32, origin_x: f32, origin_y: f32) -> (i32, i32) {
        ((x - origin_x).round() as i32, (y - origin_y).round() as i32)
    }
    pub fn accepts(self, width: u32, height: u32) -> bool {
        (width as i64 - (self.width as f32 * self.scale).round() as i64).abs() <= 1
            && (height as i64 - (self.height as f32 * self.scale).round() as i64).abs() <= 1
    }
}

/// Minimal logical US-layout proof; no physical scan-code or IME claims.
pub fn logical_key(key: &str) -> Option<i32> {
    if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() {
        return Some(key.as_bytes()[0].to_ascii_uppercase() as i32);
    }
    Some(match key {
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "backspace" => 8,
        "tab" => 9,
        "enter" => 13,
        "escape" => 27,
        "space" => 32,
        "delete" => 46,
        "home" => 36,
        "end" => 35,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_frames_are_rejected_and_latest_is_bounded() {
        let mailbox = Mailbox::new();
        assert_eq!(frame_len(0, 1), None);
        assert_eq!(frame_len(4097, 1), None);
        assert_eq!(frame_len(i32::MAX, i32::MAX), None);
        assert!(!mailbox.publish(1, 1, &[0; 3]));
        assert!(mailbox.publish(1, 1, &[1; 4]));
        assert!(mailbox.publish(1, 1, &[2; 4]));
        assert_eq!(mailbox.take().unwrap().bgra, vec![2; 4]);
        assert!(mailbox.take().is_none());
        assert_eq!(mailbox.wake.len(), 1);
        mailbox.close();
        assert!(!mailbox.publish(1, 1, &[3; 4]));
    }
    #[test]
    fn scaled_geometry_rejects_stale_frames() {
        let geometry = Geometry {
            width: 100,
            height: 50,
            scale: 2.,
        };
        assert!(geometry.accepts(200, 100));
        assert!(!geometry.accepts(100, 50));
        assert_eq!(geometry.local_point(40., 70., 10., 20.), (30, 50));
    }
}
