//! Port of backend/app/services/websocket_manager.py: the WebSocket connection manager with Redis pub/sub
//! fan-out. Each connected user is subscribed to
//!   - their personal channel `messaging:user:{user_id}`,
//!   - every group channel they belong to, `messaging:channel:{channel_id}`.
//!
//! Typing indicators are ephemeral Redis messages to the room channels.
//!
//! The socket itself belongs to the axum handler: it accepts the upgrade, makes an
//! `mpsc::unbounded_channel::<String>()`, forwards everything the receiver yields to the socket as text
//! frames, and hands the sender to `connect`. When the socket closes the handler drops the receiver, so a
//! later send fails and that subscribe loop ends, as `websocket.send_text` raising ends it in Python.
//!
//! Python behaviour kept on purpose:
//! - `active` is keyed by user id: a second connection of the same user replaces the first in the
//!   registry, but the first connection's loops keep delivering to it; `disconnect` of either removes the
//!   user, and every loop of that user stops at its next message.
//! - a loop only notices a disconnect when the next message (or its subscribe confirmation) arrives.
//! - duplicate ids in `channel_ids` get one loop each (Python iterates the list, not the set).
//! - payloads are `json.dumps(payload)` (ASCII-escaped, `", "` / `": "` separators).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

/// What the handler gives the manager for one socket: text frames to send.
pub type WsSender = UnboundedSender<String>;

/// The event on a user's personal channel that says they are no longer in a group channel.
pub const CHANNEL_REMOVED_EVENT: &str = "channel_membership_removed";

/// `json.dumps(payload)`.
pub fn dumps(payload: &Value) -> String {
    use granary_compat::pyjson::{dumps, DumpOpts, PyValue};
    dumps(&PyValue::from(payload), &DumpOpts::default()).unwrap_or_default()
}

#[derive(Default)]
struct Inner {
    /// user_id -> socket, in insertion order (a Python dict).
    active: Mutex<Vec<(String, WsSender)>>,
    /// user_id -> channel ids subscribed.
    channel_subs: Mutex<HashMap<String, HashSet<String>>>,
    /// Batch E: (user_id, channel_id) -> generation. A channel loop delivers only while its generation is
    /// current; removing the subscription bumps it, so a removed member stops getting the channel at once.
    sub_gen: Mutex<HashMap<(String, String), u64>>,
    redis: tokio::sync::Mutex<Option<(redis::Client, redis::aio::MultiplexedConnection)>>,
}

/// `ConnectionManager`. Cheap to clone (shared state).
#[derive(Clone, Default)]
pub struct ConnectionManager {
    inner: Arc<Inner>,
}

impl ConnectionManager {
    pub fn new() -> ConnectionManager {
        ConnectionManager::default()
    }

    /// `_get_redis()`: a client on `settings.REDIS_URL`, made on first use.
    async fn _get_redis(
        &self,
    ) -> redis::RedisResult<(redis::Client, redis::aio::MultiplexedConnection)> {
        let mut guard = self.inner.redis.lock().await;
        if guard.is_none() {
            let client = redis::Client::open(granary_core::config::settings().REDIS_URL.as_str())?;
            let conn = client.get_multiplexed_async_connection().await?;
            *guard = Some((client, conn));
        }
        Ok(guard.clone().expect("set above"))
    }

    /// Drops the Redis client (the Python tests reset `manager._redis`).
    pub async fn reset_redis(&self) {
        *self.inner.redis.lock().await = None;
    }

    fn is_active(&self, user_id: &str) -> bool {
        self.inner
            .active
            .lock()
            .unwrap()
            .iter()
            .any(|(u, _)| u == user_id)
    }

    fn socket(&self, user_id: &str) -> Option<WsSender> {
        self.inner
            .active
            .lock()
            .unwrap()
            .iter()
            .find(|(u, _)| u == user_id)
            .map(|(_, s)| s.clone())
    }

    /// `connect(user_id, websocket, channel_ids=None)` (the handler has already accepted the socket).
    pub async fn connect(
        &self,
        user_id: &str,
        websocket: WsSender,
        channel_ids: Option<Vec<String>>,
    ) {
        let ids = channel_ids.unwrap_or_default();
        let total = {
            let mut active = self.inner.active.lock().unwrap();
            match active.iter_mut().find(|(u, _)| u == user_id) {
                Some(slot) => slot.1 = websocket.clone(), // a dict keeps the key's original position
                None => active.push((user_id.to_string(), websocket.clone())),
            }
            active.len()
        };
        self.inner
            .channel_subs
            .lock()
            .unwrap()
            .insert(user_id.to_string(), ids.iter().cloned().collect());

        self.spawn_loop(
            user_id,
            websocket.clone(),
            format!("messaging:user:{user_id}"),
        );
        for cid in &ids {
            self.spawn_loop(
                user_id,
                websocket.clone(),
                format!("messaging:channel:{cid}"),
            );
        }
        tracing::info!(user_id, total, "WS connected");
    }

    /// `disconnect(user_id)`.
    pub fn disconnect(&self, user_id: &str) {
        let total = {
            let mut active = self.inner.active.lock().unwrap();
            active.retain(|(u, _)| u != user_id);
            active.len()
        };
        self.inner.channel_subs.lock().unwrap().remove(user_id);
        tracing::info!(user_id, total, "WS disconnected");
    }

    /// `add_channel_subscription(user_id, channel_id)`: subscribes an already-connected user to a channel.
    pub async fn add_channel_subscription(&self, user_id: &str, channel_id: &str) {
        let Some(ws) = self.socket(user_id) else {
            return;
        };
        let added = self
            .inner
            .channel_subs
            .lock()
            .unwrap()
            .entry(user_id.to_string())
            .or_default()
            .insert(channel_id.to_string());
        if added {
            self.spawn_loop(user_id, ws, format!("messaging:channel:{channel_id}"));
        }
    }

    fn generation(&self, user_id: &str, channel_id: &str) -> u64 {
        *self
            .inner
            .sub_gen
            .lock()
            .unwrap()
            .get(&(user_id.to_string(), channel_id.to_string()))
            .unwrap_or(&0)
    }

    /// Batch E: a member removed from a channel (or who left it) stops receiving its live messages at once,
    /// not at their next reconnect. Every subscribe loop of that user on that channel ends at its next message,
    /// without delivering it.
    pub fn remove_channel_subscription(&self, user_id: &str, channel_id: &str) {
        if let Some(subs) = self.inner.channel_subs.lock().unwrap().get_mut(user_id) {
            subs.remove(channel_id);
        }
        *self
            .inner
            .sub_gen
            .lock()
            .unwrap()
            .entry((user_id.to_string(), channel_id.to_string()))
            .or_insert(0) += 1;
    }

    /// Whether `user_id` is (still) subscribed to `channel_id` in this process.
    pub fn is_subscribed(&self, user_id: &str, channel_id: &str) -> bool {
        self.inner
            .channel_subs
            .lock()
            .unwrap()
            .get(user_id)
            .is_some_and(|s| s.contains(channel_id))
    }

    /// Batch E: tells every API process (each keeps its own sockets) that `user_id` left `channel_id`:
    /// the user's personal loop sees this event and drops the channel there too. The browser gets it as
    /// well, so it can close the channel.
    pub async fn announce_channel_removal(
        &self,
        user_id: &str,
        channel_id: &str,
    ) -> redis::RedisResult<()> {
        self.remove_channel_subscription(user_id, channel_id);
        self.publish(
            user_id,
            &json!({"type": CHANNEL_REMOVED_EVENT, "channel_id": channel_id}),
        )
        .await
    }

    fn channel_gen(&self, user_id: &str, redis_channel: &str) -> Option<(String, u64)> {
        redis_channel
            .strip_prefix("messaging:channel:")
            .map(|cid| (cid.to_string(), self.generation(user_id, cid)))
    }

    fn spawn_loop(&self, user_id: &str, websocket: WsSender, redis_channel: String) {
        let this = self.clone();
        let user_id = user_id.to_string();
        // The generation is taken now, not when the task starts: a removal in between must still win.
        let gen = self.channel_gen(&user_id, &redis_channel);
        tokio::spawn(async move {
            this.subscribe_loop_at(&user_id, websocket, &redis_channel, gen)
                .await
        });
    }

    /// `_subscribe_loop(user_id, websocket, redis_channel)`: listens to one Redis channel and pushes its
    /// messages to the socket.
    pub async fn _subscribe_loop(&self, user_id: &str, websocket: WsSender, redis_channel: &str) {
        let gen = self.channel_gen(user_id, redis_channel);
        self.subscribe_loop_at(user_id, websocket, redis_channel, gen)
            .await
    }

    async fn subscribe_loop_at(
        &self,
        user_id: &str,
        websocket: WsSender,
        redis_channel: &str,
        channel_gen: Option<(String, u64)>,
    ) {
        let run = async {
            let (client, _) = self._get_redis().await?;
            let mut pubsub = client.get_async_pubsub().await?;
            pubsub.subscribe(redis_channel).await?;
            // Python's listen() yields the subscribe confirmation first, and checks `active` on it.
            if !self.is_active(user_id) {
                return Ok::<_, redis::RedisError>(());
            }
            let mut stream = pubsub.on_message();
            while let Some(msg) = stream.next().await {
                if !self.is_active(user_id) {
                    break;
                }
                // A channel loop stops once the user is no longer subscribed (removed or left).
                if let Some((cid, gen)) = &channel_gen {
                    if self.generation(user_id, cid) != *gen {
                        break;
                    }
                }
                let data: String = match msg.get_payload() {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if channel_gen.is_none() && data.contains(CHANNEL_REMOVED_EVENT) {
                    if let Ok(v) = serde_json::from_str::<Value>(&data) {
                        if v["type"] == CHANNEL_REMOVED_EVENT {
                            if let Some(cid) = v["channel_id"].as_str() {
                                self.remove_channel_subscription(user_id, cid);
                            }
                        }
                    }
                }
                if websocket.send(data).is_err() {
                    break;
                }
            }
            Ok(())
        };
        if let Err(e) = run.await {
            tracing::error!(user_id, channel = redis_channel, error = %e, "subscribe_loop error");
        }
        // Dropping the pub/sub connection unsubscribes and closes it.
    }

    async fn publish_raw(&self, channel: &str, data: &str) -> redis::RedisResult<()> {
        let (_, mut conn) = self._get_redis().await?;
        let _: i64 = redis::cmd("PUBLISH")
            .arg(channel)
            .arg(data)
            .query_async(&mut conn)
            .await?;
        Ok(())
    }

    /// `publish(user_id, payload)`: to the user's personal channel.
    pub async fn publish(&self, user_id: &str, payload: &Value) -> redis::RedisResult<()> {
        self.publish_raw(&format!("messaging:user:{user_id}"), &dumps(payload))
            .await
    }

    /// `publish_to_channel(channel_id, payload)`: to a group channel.
    pub async fn publish_to_channel(
        &self,
        channel_id: &str,
        payload: &Value,
    ) -> redis::RedisResult<()> {
        self.publish_raw(&format!("messaging:channel:{channel_id}"), &dumps(payload))
            .await
    }

    /// `publish_to_dm(sender_id, recipient_id, payload)`: to both participants (recipient first).
    pub async fn publish_to_dm(
        &self,
        sender_id: &str,
        recipient_id: &str,
        payload: &Value,
    ) -> redis::RedisResult<()> {
        let data = dumps(payload);
        self.publish_raw(&format!("messaging:user:{recipient_id}"), &data)
            .await?;
        self.publish_raw(&format!("messaging:user:{sender_id}"), &data)
            .await
    }

    /// `broadcast_typing(room, typer_id, typer_name, is_typing)`; room is `"dm:{other_user_id}"` or
    /// `"channel:{channel_id}"` (anything else is ignored).
    pub async fn broadcast_typing(
        &self,
        room: &str,
        typer_id: &str,
        typer_name: &str,
        is_typing: bool,
    ) -> redis::RedisResult<()> {
        let payload = dumps(&json!({
            "type": "typing",
            "room": room,
            "typer_id": typer_id,
            "typer_name": typer_name,
            "is_typing": is_typing,
        }));
        // Python opens the client before looking at the room.
        self._get_redis().await?;
        if let Some(channel_id) = room.strip_prefix("channel:") {
            self.publish_raw(&format!("messaging:channel:{channel_id}"), &payload)
                .await?;
        } else if let Some(other_id) = room.strip_prefix("dm:") {
            self.publish_raw(&format!("messaging:user:{other_id}"), &payload)
                .await?;
            self.publish_raw(&format!("messaging:user:{typer_id}"), &payload)
                .await?;
        }
        Ok(())
    }

    /// `is_online(user_id)`.
    pub fn is_online(&self, user_id: &str) -> bool {
        self.is_active(user_id)
    }

    /// `list(manager.active.keys())` (insertion order).
    pub fn active_user_ids(&self) -> Vec<String> {
        self.inner
            .active
            .lock()
            .unwrap()
            .iter()
            .map(|(u, _)| u.clone())
            .collect()
    }
}

/// The module-level singleton `manager`.
#[allow(non_upper_case_globals)]
pub static manager: once_cell::sync::Lazy<ConnectionManager> =
    once_cell::sync::Lazy::new(ConnectionManager::new);
