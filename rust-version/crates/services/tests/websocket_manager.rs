//! services/websocket_manager.rs on the test Redis: registry, subscribe loops, payload format.

mod support;

use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use granary_services::websocket_manager::{dumps, ConnectionManager};

fn init() {
    std::env::set_var("REDIS_URL", "redis://127.0.0.1:56380/7");
    support::init();
    if std::env::var_os("DATABASE_URL").is_none() {
        std::env::set_var(
            "DATABASE_URL",
            "postgres://granary:granarytest@127.0.0.1:55433/postgres",
        ); // settings need one; unused
    }
}

async fn wait_subscribed(channel: &str, n: i64) {
    let mut conn = granary_core::redis_client::get_redis()
        .conn()
        .await
        .unwrap();
    for _ in 0..200 {
        let v: (String, i64) = redis::cmd("PUBSUB")
            .arg("NUMSUB")
            .arg(channel)
            .query_async(&mut conn)
            .await
            .unwrap();
        if v.1 >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{channel} never reached {n} subscribers");
}

async fn recv(rx: &mut UnboundedReceiver<String>) -> Option<String> {
    tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .ok()
        .flatten()
}

fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn payload_is_python_json_dumps() {
    assert_eq!(
        dumps(
            &json!({"type": "message", "content": "héllo \"x\"", "n": 1, "f": 1.5, "ok": true, "x": null, "l": [1, 2]})
        ),
        r#"{"type": "message", "content": "h\u00e9llo \"x\"", "n": 1, "f": 1.5, "ok": true, "x": null, "l": [1, 2]}"#
    );
}

#[tokio::test]
async fn publish_reaches_connected_sockets() {
    init();
    let m = ConnectionManager::new();
    let (alice, bob, ch) = (uid(), uid(), uid());
    let (tx_a, mut rx_a) = unbounded_channel();
    let (tx_b, mut rx_b) = unbounded_channel();
    m.connect(&alice, tx_a, Some(vec![ch.clone()])).await;
    m.connect(&bob, tx_b, None).await;
    wait_subscribed(&format!("messaging:user:{alice}"), 1).await;
    wait_subscribed(&format!("messaging:channel:{ch}"), 1).await;
    wait_subscribed(&format!("messaging:user:{bob}"), 1).await;
    assert!(m.is_online(&alice) && m.is_online(&bob) && !m.is_online(&uid()));
    assert_eq!(m.active_user_ids(), vec![alice.clone(), bob.clone()]);

    m.publish(&alice, &json!({"type": "dm", "text": "é"}))
        .await
        .unwrap();
    assert_eq!(
        recv(&mut rx_a).await.as_deref(),
        Some(r#"{"type": "dm", "text": "\u00e9"}"#)
    );
    m.publish_to_channel(&ch, &json!({"type": "channel_message"}))
        .await
        .unwrap();
    assert_eq!(
        recv(&mut rx_a).await.as_deref(),
        Some(r#"{"type": "channel_message"}"#)
    );
    assert_eq!(recv(&mut rx_b).await, None);

    m.publish_to_dm(&alice, &bob, &json!({"type": "dm"}))
        .await
        .unwrap();
    assert_eq!(recv(&mut rx_a).await.as_deref(), Some(r#"{"type": "dm"}"#));
    assert_eq!(recv(&mut rx_b).await.as_deref(), Some(r#"{"type": "dm"}"#));

    // typing: channel rooms go to the channel, dm rooms to both users, others nowhere
    m.broadcast_typing(&format!("channel:{ch}"), &bob, "Bob", true)
        .await
        .unwrap();
    let want = format!(
        r#"{{"type": "typing", "room": "channel:{ch}", "typer_id": "{bob}", "typer_name": "Bob", "is_typing": true}}"#
    );
    assert_eq!(recv(&mut rx_a).await, Some(want));
    m.broadcast_typing(&format!("dm:{alice}"), &bob, "Bob", false)
        .await
        .unwrap();
    let want = format!(
        r#"{{"type": "typing", "room": "dm:{alice}", "typer_id": "{bob}", "typer_name": "Bob", "is_typing": false}}"#
    );
    assert_eq!(recv(&mut rx_a).await, Some(want.clone()));
    assert_eq!(recv(&mut rx_b).await, Some(want));
    m.broadcast_typing("lobby", &bob, "Bob", true)
        .await
        .unwrap();
    assert_eq!(recv(&mut rx_a).await, None);

    // add_channel_subscription: only for connected users, once per channel
    let ch2 = uid();
    m.add_channel_subscription(&bob, &ch2).await;
    m.add_channel_subscription(&bob, &ch2).await;
    m.add_channel_subscription(&uid(), &ch2).await;
    wait_subscribed(&format!("messaging:channel:{ch2}"), 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    m.publish_to_channel(&ch2, &json!({"n": 2})).await.unwrap();
    assert_eq!(recv(&mut rx_b).await.as_deref(), Some(r#"{"n": 2}"#));
    assert_eq!(recv(&mut rx_b).await, None); // one loop only

    // disconnect: the loop stops at the next message and unsubscribes
    m.disconnect(&alice);
    assert!(!m.is_online(&alice));
    assert_eq!(m.active_user_ids(), vec![bob.clone()]);
    m.publish(&alice, &json!({"after": true})).await.unwrap();
    assert_eq!(recv(&mut rx_a).await, None);
    let mut conn = granary_core::redis_client::get_redis()
        .conn()
        .await
        .unwrap();
    for _ in 0..100 {
        let v: (String, i64) = redis::cmd("PUBSUB")
            .arg("NUMSUB")
            .arg(format!("messaging:user:{alice}"))
            .query_async(&mut conn)
            .await
            .unwrap();
        if v.1 == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let v: (String, i64) = redis::cmd("PUBSUB")
        .arg("NUMSUB")
        .arg(format!("messaging:user:{alice}"))
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(v.1, 0);
    m.disconnect(&bob);
}

/// Python keys the registry by user: a second connection replaces the first in `active` (keeping its
/// position) and both sockets keep receiving until one disconnects, which removes the user.
#[tokio::test]
async fn reconnect_semantics() {
    init();
    let m = ConnectionManager::new();
    let (u, other) = (uid(), uid());
    let (tx1, mut rx1) = unbounded_channel();
    let (tx2, mut rx2) = unbounded_channel();
    let (tx3, _rx3) = unbounded_channel();
    m.connect(&u, tx1, None).await;
    m.connect(&other, tx3, None).await;
    m.connect(&u, tx2, None).await;
    assert_eq!(m.active_user_ids(), vec![u.clone(), other.clone()]);
    wait_subscribed(&format!("messaging:user:{u}"), 2).await;
    m.publish(&u, &json!({"a": 1})).await.unwrap();
    assert!(recv(&mut rx1).await.is_some());
    assert!(recv(&mut rx2).await.is_some());
    // a closed socket ends its own loop only
    drop(rx1);
    m.publish(&u, &json!({"a": 2})).await.unwrap();
    assert!(recv(&mut rx2).await.is_some());
    m.disconnect(&u);
    m.publish(&u, &json!({"a": 3})).await.unwrap();
    assert_eq!(recv(&mut rx2).await, None);
    m.disconnect(&other);
}
