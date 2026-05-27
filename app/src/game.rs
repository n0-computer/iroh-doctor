//! Port of PongGame.swift - pure logic, no IO.

use std::time::Instant;

use rand::{rngs::ThreadRng, Rng};

use crate::wire::{self, BallPayload};

pub const PADDLE_HALF_WIDTH: f32 = 0.15;
pub const MY_PADDLE_Y: f32 = 0.92;
pub const OPPONENT_PADDLE_Y: f32 = -0.92;
pub const BALL_RADIUS: f32 = 0.035;
pub const INITIAL_BALL_SPEED: f32 = 0.7;
pub const MAX_BALL_SPEED: f32 = 1.8;
pub const SPEEDUP_FACTOR: f32 = 1.05;
pub const OPPONENT_LEAD_TIME: f32 = 0.05;
pub const OPPONENT_VELOCITY_EMA: f32 = 0.3;
pub const OPPONENT_VELOCITY_CAP: f32 = 4.0;

#[derive(Debug, Clone, Copy)]
pub struct PongGame {
    pub is_authority: bool,
    pub my_paddle_x: f32,
    pub opponent_paddle_x: f32,
    pub opponent_paddle_vx: f32,
    pub ball_pos: (f32, f32),
    pub ball_vel: (f32, f32),
    pub my_score: u16,
    pub their_score: u16,

    last_clock: Option<Instant>,
    last_opponent_paddle_at: Option<Instant>,
}

impl Default for PongGame {
    fn default() -> Self {
        Self {
            is_authority: false,
            my_paddle_x: 0.0,
            opponent_paddle_x: 0.0,
            opponent_paddle_vx: 0.0,
            ball_pos: (0.0, 0.0),
            ball_vel: (0.0, 0.0),
            my_score: 0,
            their_score: 0,
            last_clock: None,
            last_opponent_paddle_at: None,
        }
    }
}

impl PongGame {
    pub fn opponent_paddle_predicted_x(&self) -> f32 {
        let predicted = self.opponent_paddle_x + self.opponent_paddle_vx * OPPONENT_LEAD_TIME;
        predicted.clamp(-1.0, 1.0)
    }

    pub fn set_my_paddle(&mut self, x: f32) {
        self.my_paddle_x = x.clamp(-1.0, 1.0);
    }

    pub fn received_opponent_paddle(&mut self, x: f32) {
        let clamped = x.clamp(-1.0, 1.0);
        let now = Instant::now();
        match self.last_opponent_paddle_at {
            Some(last) => {
                let dt = (now - last).as_secs_f32().min(0.2);
                if dt > 0.005 {
                    let raw = (clamped - self.opponent_paddle_x) / dt;
                    let capped = raw.clamp(-OPPONENT_VELOCITY_CAP, OPPONENT_VELOCITY_CAP);
                    self.opponent_paddle_vx = self.opponent_paddle_vx
                        * (1.0 - OPPONENT_VELOCITY_EMA)
                        + capped * OPPONENT_VELOCITY_EMA;
                }
            }
            None => {
                self.opponent_paddle_vx = 0.0;
            }
        }
        self.opponent_paddle_x = clamped;
        self.last_opponent_paddle_at = Some(now);
    }

    pub fn received_ball(&mut self, p: BallPayload) {
        // Authority's frame -> our frame: flip y on position and velocity.
        // Scores swap (authority's "my" is our "their").
        self.ball_pos = (p.x, -p.y);
        self.ball_vel = (p.vx, -p.vy);
        self.their_score = p.my_score;
        self.my_score = p.their_score;
    }

    pub fn reset_for_new_session(&mut self, as_authority: bool, rng: &mut ThreadRng) {
        self.is_authority = as_authority;
        self.my_score = 0;
        self.their_score = 0;
        self.opponent_paddle_x = 0.0;
        self.opponent_paddle_vx = 0.0;
        self.last_clock = None;
        self.last_opponent_paddle_at = None;
        if as_authority {
            self.reset_ball(rng.gen::<bool>(), rng);
        } else {
            self.ball_pos = (0.0, 0.0);
            self.ball_vel = (0.0, 0.0);
        }
    }

    #[allow(dead_code)]
    pub fn session_ended(&mut self) {
        self.opponent_paddle_x = 0.0;
        self.opponent_paddle_vx = 0.0;
        self.last_opponent_paddle_at = None;
        self.ball_pos = (0.0, 0.0);
        self.ball_vel = (0.0, 0.0);
    }

    pub fn tick_from_clock(&mut self, rng: &mut ThreadRng) {
        let now = Instant::now();
        let dt = match self.last_clock {
            Some(last) if now > last => (now - last).as_secs_f32().min(0.05),
            _ => 1.0 / 60.0,
        };
        self.last_clock = Some(now);
        self.tick(dt, rng);
    }

    fn tick(&mut self, dt: f32, rng: &mut ThreadRng) {
        if !self.is_authority {
            // Non-authority: linear extrapolation from last received velocity.
            self.ball_pos.0 += self.ball_vel.0 * dt;
            self.ball_pos.1 += self.ball_vel.1 * dt;
            return;
        }
        let mut x = self.ball_pos.0 + self.ball_vel.0 * dt;
        let mut y = self.ball_pos.1 + self.ball_vel.1 * dt;

        // Side walls
        if x < -1.0 + BALL_RADIUS {
            x = -1.0 + BALL_RADIUS;
            self.ball_vel.0 = self.ball_vel.0.abs();
        } else if x > 1.0 - BALL_RADIUS {
            x = 1.0 - BALL_RADIUS;
            self.ball_vel.0 = -self.ball_vel.0.abs();
        }

        // My paddle (bottom)
        if self.ball_vel.1 > 0.0
            && y > MY_PADDLE_Y - BALL_RADIUS
            && (x - self.my_paddle_x).abs() < PADDLE_HALF_WIDTH + BALL_RADIUS
        {
            y = MY_PADDLE_Y - BALL_RADIUS;
            self.ball_vel.1 = -self.ball_vel.1.abs();
            self.apply_paddle_spin(x, self.my_paddle_x);
            self.speed_up();
        }
        // Opponent paddle (top), with lead compensation
        if self.ball_vel.1 < 0.0 && y < OPPONENT_PADDLE_Y + BALL_RADIUS {
            let lead = self.opponent_paddle_predicted_x();
            if (x - lead).abs() < PADDLE_HALF_WIDTH + BALL_RADIUS {
                y = OPPONENT_PADDLE_Y + BALL_RADIUS;
                self.ball_vel.1 = self.ball_vel.1.abs();
                self.apply_paddle_spin(x, lead);
                self.speed_up();
            }
        }

        if y > 1.0 + BALL_RADIUS {
            self.their_score = self.their_score.wrapping_add(1);
            self.reset_ball(true, rng);
            return;
        }
        if y < -1.0 - BALL_RADIUS {
            self.my_score = self.my_score.wrapping_add(1);
            self.reset_ball(false, rng);
            return;
        }
        self.ball_pos = (x, y);
    }

    pub fn ball_frame(&self) -> Option<Vec<u8>> {
        if !self.is_authority {
            return None;
        }
        Some(wire::encode_ball(
            self.ball_pos.0,
            self.ball_pos.1,
            self.ball_vel.0,
            self.ball_vel.1,
            self.my_score,
            self.their_score,
        ))
    }

    fn reset_ball(&mut self, toward_me: bool, rng: &mut ThreadRng) {
        self.ball_pos = (0.0, 0.0);
        let angle: f32 = rng.gen_range(-0.35..0.35);
        let vx = angle.sin() * INITIAL_BALL_SPEED;
        let vy = (if toward_me { 1.0 } else { -1.0 }) * angle.cos() * INITIAL_BALL_SPEED;
        self.ball_vel = (vx, vy);
    }

    fn apply_paddle_spin(&mut self, ball_x: f32, paddle_x: f32) {
        let offset = (ball_x - paddle_x) / (PADDLE_HALF_WIDTH + BALL_RADIUS);
        let clamped = offset.clamp(-1.0, 1.0);
        let speed = (self.ball_vel.0 * self.ball_vel.0 + self.ball_vel.1 * self.ball_vel.1).sqrt();
        let new_angle = clamped * 0.9;
        let sign_y: f32 = if self.ball_vel.1 >= 0.0 { 1.0 } else { -1.0 };
        self.ball_vel = (new_angle.sin() * speed, sign_y * new_angle.cos() * speed);
    }

    fn speed_up(&mut self) {
        let speed = (self.ball_vel.0 * self.ball_vel.0 + self.ball_vel.1 * self.ball_vel.1).sqrt();
        if speed < 0.0001 {
            return;
        }
        let next = MAX_BALL_SPEED.min(speed * SPEEDUP_FACTOR);
        let scale = next / speed;
        self.ball_vel = (self.ball_vel.0 * scale, self.ball_vel.1 * scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_my_paddle_clamps_to_unit_range() {
        let mut g = PongGame::default();
        g.set_my_paddle(2.0);
        assert_eq!(g.my_paddle_x, 1.0);
        g.set_my_paddle(-2.0);
        assert_eq!(g.my_paddle_x, -1.0);
        g.set_my_paddle(0.5);
        assert_eq!(g.my_paddle_x, 0.5);
    }

    #[test]
    fn opponent_predicted_x_clamps_to_unit_range() {
        let mut g = PongGame {
            opponent_paddle_x: 0.99,
            opponent_paddle_vx: 10.0,
            ..PongGame::default()
        };
        // Without clamp this would project well past 1.0.
        assert!(g.opponent_paddle_predicted_x() <= 1.0);
        g.opponent_paddle_x = -0.99;
        g.opponent_paddle_vx = -10.0;
        assert!(g.opponent_paddle_predicted_x() >= -1.0);
    }

    #[test]
    fn opponent_predicted_x_uses_lead_time() {
        let g = PongGame {
            opponent_paddle_x: 0.1,
            opponent_paddle_vx: 1.0,
            ..PongGame::default()
        };
        // 0.1 + 1.0 * OPPONENT_LEAD_TIME = 0.15.
        let predicted = g.opponent_paddle_predicted_x();
        assert!((predicted - 0.15).abs() < 1e-6, "got {predicted}");
    }

    #[test]
    fn received_ball_flips_y_and_swaps_scores() {
        let mut g = PongGame {
            my_score: 1,
            their_score: 0,
            ..PongGame::default()
        };
        g.received_ball(BallPayload {
            x: 0.5,
            y: 0.25,
            vx: 0.3,
            vy: 0.4,
            my_score: 7,
            their_score: 9,
        });
        // x unchanged, y flipped: the authority's frame has the opposite
        // perspective.
        assert_eq!(g.ball_pos, (0.5, -0.25));
        assert_eq!(g.ball_vel, (0.3, -0.4));
        // Score-swap: authority's "my" becomes our "their".
        assert_eq!(g.their_score, 7);
        assert_eq!(g.my_score, 9);
    }

    #[test]
    fn ball_frame_returns_none_for_non_authority() {
        let g = PongGame::default();
        assert!(g.ball_frame().is_none());
    }

    #[test]
    fn ball_frame_some_for_authority() {
        let g = PongGame {
            is_authority: true,
            ..PongGame::default()
        };
        let frame = g.ball_frame().expect("authority must produce a frame");
        // wire encoding is tag + 20 bytes body.
        assert_eq!(frame.len(), 21);
        assert_eq!(frame[0], wire::TAG_BALL);
    }

    #[test]
    fn received_opponent_paddle_clamps_input() {
        let mut g = PongGame::default();
        g.received_opponent_paddle(2.5);
        assert_eq!(g.opponent_paddle_x, 1.0);
        g.received_opponent_paddle(-2.5);
        assert_eq!(g.opponent_paddle_x, -1.0);
    }
}
