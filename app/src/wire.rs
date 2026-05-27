//! Same wire format as the Swift HelloIroh app (WireFormat.swift):
//!
//! - ALPN: `iroh-helloiroh-pong/0`
//! - Paddle frame: `tag=0 (u8), x (f32 LE)` - 5 bytes, both peers
//! - Ball frame:   `tag=1 (u8), x f32, y f32, vx f32, vy f32, myScore u16, theirScore u16` - 21 bytes, authority only

pub const ALPN: &[u8] = b"iroh-helloiroh-pong/0";

pub const TAG_PADDLE: u8 = 0;
pub const TAG_BALL: u8 = 1;

pub fn encode_paddle(x: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.push(TAG_PADDLE);
    out.extend_from_slice(&x.to_bits().to_le_bytes());
    out
}

pub fn decode_paddle_body(data: &[u8]) -> Option<f32> {
    if data.len() != 4 {
        return None;
    }
    let bits = u32::from_le_bytes(data[0..4].try_into().ok()?);
    Some(f32::from_bits(bits))
}

pub fn encode_ball(x: f32, y: f32, vx: f32, vy: f32, my_score: u16, their_score: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(21);
    out.push(TAG_BALL);
    out.extend_from_slice(&x.to_bits().to_le_bytes());
    out.extend_from_slice(&y.to_bits().to_le_bytes());
    out.extend_from_slice(&vx.to_bits().to_le_bytes());
    out.extend_from_slice(&vy.to_bits().to_le_bytes());
    out.extend_from_slice(&my_score.to_le_bytes());
    out.extend_from_slice(&their_score.to_le_bytes());
    out
}

#[derive(Debug, Clone, Copy)]
pub struct BallPayload {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub my_score: u16,
    pub their_score: u16,
}

pub fn decode_ball_body(data: &[u8]) -> Option<BallPayload> {
    if data.len() != 20 {
        return None;
    }
    let x = f32::from_bits(u32::from_le_bytes(data[0..4].try_into().ok()?));
    let y = f32::from_bits(u32::from_le_bytes(data[4..8].try_into().ok()?));
    let vx = f32::from_bits(u32::from_le_bytes(data[8..12].try_into().ok()?));
    let vy = f32::from_bits(u32::from_le_bytes(data[12..16].try_into().ok()?));
    let my_score = u16::from_le_bytes(data[16..18].try_into().ok()?);
    let their_score = u16::from_le_bytes(data[18..20].try_into().ok()?);
    Some(BallPayload {
        x,
        y,
        vx,
        vy,
        my_score,
        their_score,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paddle_roundtrip() {
        let f = encode_paddle(0.5);
        assert_eq!(f.len(), 5);
        assert_eq!(f[0], TAG_PADDLE);
        assert_eq!(decode_paddle_body(&f[1..]), Some(0.5));
    }

    #[test]
    fn ball_roundtrip() {
        let f = encode_ball(0.1, -0.2, 0.3, -0.4, 3, 5);
        assert_eq!(f.len(), 21);
        assert_eq!(f[0], TAG_BALL);
        let p = decode_ball_body(&f[1..]).unwrap();
        assert_eq!(p.x, 0.1);
        assert_eq!(p.y, -0.2);
        assert_eq!(p.vx, 0.3);
        assert_eq!(p.vy, -0.4);
        assert_eq!(p.my_score, 3);
        assert_eq!(p.their_score, 5);
    }
}
