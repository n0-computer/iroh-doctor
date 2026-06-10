//! Gossip command helpers: joining a topic and deriving a topic id
//! from a friendly string or a 64-hex id.

use std::str::FromStr;
use std::sync::Arc;

use iroh::EndpointId;
use iroh_gossip::net::Gossip;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{instrument, warn};

use super::*;

#[instrument(
    skip(gossip, recv_handle, sender_slot, events_tx, bootstrap),
    fields(topic_input)
)]
pub(crate) async fn join_gossip(
    gossip: Gossip,
    recv_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    sender_slot: Arc<Mutex<Option<iroh_gossip::api::GossipSender>>>,
    topic_input: String,
    bootstrap: Vec<String>,
    events_tx: mpsc::Sender<GossipEventUi>,
) -> Result<String, String> {
    use n0_future::StreamExt;

    let topic_id = parse_topic_id(&topic_input);

    let mut bootstrap_ids = Vec::with_capacity(bootstrap.len());
    for raw in bootstrap {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let id = EndpointId::from_str(trimmed)
            .map_err(|e| format!("invalid bootstrap endpoint id `{trimmed}`: {e}"))?;
        bootstrap_ids.push(id);
    }

    {
        let mut h = recv_handle.lock().await;
        if let Some(prev) = h.take() {
            prev.abort();
        }
    }
    {
        let mut s = sender_slot.lock().await;
        s.take();
    }

    let topic = gossip
        .subscribe_and_join(topic_id, bootstrap_ids)
        .await
        .map_err(|e| format!("subscribe_and_join: {e:#}"))?;
    let (sender, mut receiver) = topic.split();
    {
        let mut s = sender_slot.lock().await;
        *s = Some(sender);
    }

    let handle = tokio::spawn(async move {
        while let Some(item) = receiver.next().await {
            use iroh_gossip::api::Event;
            let event = match item {
                Ok(e) => e,
                Err(err) => {
                    warn!(?err, "gossip receive error");
                    break;
                }
            };
            let ui = match event {
                Event::NeighborUp(peer) => GossipEventUi::NeighborUp {
                    peer: peer.to_string(),
                },
                Event::NeighborDown(peer) => GossipEventUi::NeighborDown {
                    peer: peer.to_string(),
                },
                Event::Received(msg) => GossipEventUi::Message {
                    from: msg.delivered_from.to_string(),
                    body: String::from_utf8_lossy(&msg.content).into_owned(),
                },
                Event::Lagged => GossipEventUi::Lagged,
            };
            if events_tx.try_send(ui).is_err() {
                warn!("gossip events_tx full, dropping event");
            }
        }
    });

    let mut h = recv_handle.lock().await;
    *h = Some(handle);

    Ok(topic_id.to_string())
}

fn parse_topic_id(input: &str) -> iroh_gossip::proto::TopicId {
    let trimmed = input.trim();
    if trimmed.len() == 64 {
        if let Ok(id) = trimmed.parse::<iroh_gossip::proto::TopicId>() {
            return id;
        }
    }
    // Hash the user-supplied string with BLAKE3 so any friendly name maps
    // deterministically to a topic both peers can compute.
    let hash = blake3::hash(trimmed.as_bytes());
    iroh_gossip::proto::TopicId::from_bytes(*hash.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_topic_id_64_hex_roundtrips() {
        let hex: String = "ab".repeat(32);
        let topic = parse_topic_id(&hex);
        assert_eq!(topic.to_string(), hex);
    }

    #[test]
    fn parse_topic_id_friendly_string_hashes() {
        let a = parse_topic_id("hello-iroh-pong");
        let b = parse_topic_id("hello-iroh-pong");
        let c = parse_topic_id("different-topic");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn parse_topic_id_trims_whitespace_before_hashing() {
        let a = parse_topic_id("  test  ");
        let b = parse_topic_id("test");
        assert_eq!(a, b);
    }

    #[test]
    fn parse_topic_id_64_non_hex_falls_back_to_hash() {
        // 64 chars but with 'z' (not a hex digit) - must not be parsed as
        // hex; falls through to the BLAKE3 fallback.
        let s = "z".repeat(64);
        let from_blake = parse_topic_id(&s);
        // Same input through the hash branch must be deterministic.
        let again = parse_topic_id(&s);
        assert_eq!(from_blake, again);
        // And it must differ from a different input.
        let other = parse_topic_id("something-else");
        assert_ne!(from_blake, other);
    }
}
