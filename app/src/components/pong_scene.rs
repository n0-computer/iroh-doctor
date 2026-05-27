use dioxus::prelude::*;

use super::{color_for_endpoint_id, opponent_color_for_state};
use crate::game::{self, PongGame};
use crate::peer::{ConnectionState, PeerCommand};
use crate::PeerHandle;

// Fixed field dimensions so we can map mouse pixel coords without measuring the DOM.
const FIELD_WIDTH: f32 = 420.0;
const FIELD_HEIGHT: f32 = 660.0;

#[component]
pub fn PongScene(
    game: Signal<PongGame>,
    endpoint_id: Signal<String>,
    conn_state: Signal<ConnectionState>,
    cmd_handle: Signal<Option<PeerHandle>>,
) -> Element {
    let g = game();
    let id = endpoint_id();
    let state = conn_state();
    let my_color = color_for_endpoint_id(&id);
    let opp_color = opponent_color_for_state(&state);

    let paddle_w_px = game::PADDLE_HALF_WIDTH * 2.0 * FIELD_WIDTH;
    let paddle_h_px = 12.0_f32;
    let ball_diameter = game::BALL_RADIUS * 2.0 * f32::min(FIELD_WIDTH, FIELD_HEIGHT);

    let my_paddle_left = (g.my_paddle_x + 1.0) * 0.5 * FIELD_WIDTH - paddle_w_px / 2.0;
    let my_paddle_top = (game::MY_PADDLE_Y + 1.0) * 0.5 * FIELD_HEIGHT - paddle_h_px / 2.0;

    let opp_predicted = g.opponent_paddle_predicted_x();
    let opp_paddle_left = (opp_predicted + 1.0) * 0.5 * FIELD_WIDTH - paddle_w_px / 2.0;
    let opp_paddle_top = (game::OPPONENT_PADDLE_Y + 1.0) * 0.5 * FIELD_HEIGHT - paddle_h_px / 2.0;

    let ball_left = (g.ball_pos.0 + 1.0) * 0.5 * FIELD_WIDTH - ball_diameter / 2.0;
    let ball_top = (g.ball_pos.1 + 1.0) * 0.5 * FIELD_HEIGHT - ball_diameter / 2.0;

    let my_score = g.my_score;
    let their_score = g.their_score;

    rsx! {
        div { class: "field-wrap",
            div {
                class: "field",
                style: "width: {FIELD_WIDTH}px; height: {FIELD_HEIGHT}px;",
                onpointermove: move |evt| {
                    let coords = evt.element_coordinates();
                    let nx = ((coords.x / FIELD_WIDTH as f64) * 2.0 - 1.0).clamp(-1.0, 1.0) as f32;
                    if let Some(handle) = cmd_handle.read().clone() {
                        let _ = handle.tx.try_send(PeerCommand::UpdateMyPaddle { x: nx });
                    }
                },

                // center line
                div { class: "center-line" }

                // opponent paddle (top)
                div {
                    class: "paddle opponent-paddle",
                    style: "left: {opp_paddle_left}px; top: {opp_paddle_top}px; width: {paddle_w_px}px; height: {paddle_h_px}px; background: {opp_color};",
                }

                // my paddle (bottom)
                div {
                    class: "paddle my-paddle",
                    style: "left: {my_paddle_left}px; top: {my_paddle_top}px; width: {paddle_w_px}px; height: {paddle_h_px}px; background: {my_color};",
                }

                // ball
                div {
                    class: "ball",
                    style: "left: {ball_left}px; top: {ball_top}px; width: {ball_diameter}px; height: {ball_diameter}px;",
                }

                // scores
                div {
                    class: "score score-top",
                    style: "color: {opp_color};",
                    "{their_score}"
                }
                div {
                    class: "score score-bottom",
                    style: "color: {my_color};",
                    "{my_score}"
                }
            }
        }
    }
}
