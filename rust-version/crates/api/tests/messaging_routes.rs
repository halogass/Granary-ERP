//! routers/messaging.py tests that write rows directly (ported from backend/tests with the same names and
//! assertions). The HTTP-only tests are in compat-tests/http-suite/tests/test_messaging_*.py.

mod support;

use serde_json::{json, Value};
use support::{u, UserSpec};
use uuid::Uuid;

const M: &str = "/api/v1/messaging";

fn app() -> &'static support::TestApp {
    support::app("messaging_routes")
}

/// backend/tests/test_audit_high_b2.py::test_dm_thread_is_only_for_its_two_people
#[tokio::test]
async fn test_dm_thread_is_only_for_its_two_people() {
    let app = app();
    let a = app.make_user(UserSpec::new()).await;
    let b = app.make_user(UserSpec::new()).await;
    let outsider = app.make_user(UserSpec::new()).await;
    let (sa, sb) = (a.id.to_string(), b.id.to_string());
    let tid = if sa < sb {
        format!("{sa}_{sb}")
    } else {
        format!("{sb}_{sa}")
    };
    let mut db = app.db().await;
    let root = Uuid::new_v4();
    let insert = "INSERT INTO direct_messages (id, thread_id, sender_id, receiver_id, content, thread_root_id, \
                  is_deleted_by_sender, is_deleted_by_receiver, is_read, created_at, updated_at) \
                  VALUES ($1, $2, $3, $4, $5, $6, $7, false, false, now() + make_interval(secs => $8), now())";
    // (the ORM's Python-side defaults: the flags are false, not NULL)
    for (id, s, r, content, root_id, deleted, at) in [
        (root, a.id, b.id, "dm root", None, false, 0.0),
        (
            Uuid::new_v4(),
            b.id,
            a.id,
            "dm reply",
            Some(root),
            false,
            1.0,
        ),
        (
            Uuid::new_v4(),
            a.id,
            b.id,
            "deleted reply",
            Some(root),
            true,
            2.0,
        ),
    ] {
        sqlx::query(insert)
            .bind(id)
            .bind(&tid)
            .bind(s)
            .bind(r)
            .bind(content)
            .bind(root_id)
            .bind(deleted)
            .bind(at)
            .execute(&mut db)
            .await
            .unwrap();
    }
    let path = format!("{M}/thread/{root}?source=dm");
    assert_eq!(app.get(&outsider.token, &path).await.status, 404);
    let res = app.get(&a.token, &path).await;
    assert_eq!(res.status, 200);
    let contents = |v: Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(contents(res.json()), vec!["dm reply"]);
    let res = app.get(&b.token, &path).await;
    assert_eq!(contents(res.json()), vec!["dm reply", "deleted reply"]);
}

/// backend/tests/test_catalog_pricing_a.py::test_mention_search_leaves_out_retired_products
#[tokio::test]
async fn test_mention_search_leaves_out_retired_products() {
    let app = app();
    let sa = app.superadmin().await;
    let tag = u("x");
    let live_name = format!("Pytest mention live-{tag}");
    let gone_name = format!("Pytest mention gone-{tag}");
    let mut db = app.db().await;
    let mut ids = Vec::new();
    for (name, orderable) in [(&live_name, true), (&gone_name, false)] {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO product_catalog (id, sku, name, product_type, sale_model, price_metric, pricing_mode, \
             is_whitelabel, is_active, is_orderable, is_deleted) \
             VALUES ($1, $2, $3, 'hardware', 'one_time', 'unit', 'list', false, true, $4, false)",
        )
        .bind(id)
        .bind(u("PYT-SKU"))
        .bind(name)
        .bind(orderable)
        .execute(&mut db)
        .await
        .unwrap();
        ids.push(id.to_string());
    }
    let q = tag.rsplit('-').next().unwrap();
    let res = app
        .get(
            &sa.token,
            &format!("{M}/mention/entities?q={q}&types=product"),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let got: Vec<String> = res
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert!(got.contains(&ids[0]) && !got.contains(&ids[1]));
    assert_eq!(
        res.json()[0],
        json!({"type": "product", "id": ids[0], "name": live_name, "sub": res.json()[0]["sub"]})
    );
}

/// Two messages sharing a thread's latest created_at list the thread once, with the higher id (port
/// findings batch 3), and a memo whose acknowledgements hold a non-object is a 500 on acknowledge (Python `a.get`).
#[tokio::test]
async fn conversations_and_memo_quirks() {
    let app = app();
    let a = app.make_user(UserSpec::new().name("Pyt Quirk")).await;
    let b = app.make_user(UserSpec::new()).await;
    let (sa, sb) = (a.id.to_string(), b.id.to_string());
    let tid = if sa < sb {
        format!("{sa}_{sb}")
    } else {
        format!("{sb}_{sa}")
    };
    let mut db = app.db().await;
    let mut ids = [Uuid::new_v4(), Uuid::new_v4()];
    ids.sort();
    for (id, content) in ids.iter().zip(["same instant low", "same instant high"]) {
        sqlx::query(
            "INSERT INTO direct_messages (id, thread_id, sender_id, receiver_id, content, is_read, created_at, \
             updated_at) VALUES ($1, $2, $3, $4, $5, false, '2026-01-01T00:00:00Z', now())",
        )
        .bind(id)
        .bind(&tid)
        .bind(a.id)
        .bind(b.id)
        .bind(content)
        .execute(&mut db)
        .await
        .unwrap();
    }
    let convs = app
        .get(&b.token, &format!("{M}/conversations"))
        .await
        .json();
    let mine: Vec<&Value> = convs
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["thread_id"] == json!(tid))
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["last_message"], json!("same instant high"));
    assert_eq!(mine[0]["unread_count"], json!(2));

    let memo = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO memos (id, memo_number, subject, memo_type, body, sender_id, sender_name, sender_signed_at, \
         recipient_id, status, acknowledgements) VALUES ($1, $2, 's', 'information', 'b', $3, 'x', now(), $4, \
         'sent', '[\"odd\"]'::jsonb)",
    )
    .bind(memo)
    .bind(u("PYT/MEMO"))
    .bind(a.id)
    .bind(b.id)
    .execute(&mut db)
    .await
    .unwrap();
    let res = app
        .post(
            &b.token,
            &format!("{M}/memos/{memo}/acknowledge"),
            json!(null),
        )
        .await;
    assert_eq!(res.status, 500);
    assert_eq!(
        res.json(),
        json!({"detail": "Internal server error", "code": "INTERNAL_ERROR"})
    );
}

/// A memo sent to a channel or a DM shows up in the message list with its `memo_id`, so the memo card can
/// load the memo (without it the card stayed a skeleton forever), and a channel member can read that memo.
#[tokio::test]
async fn memo_messages_carry_memo_id() {
    let app = app();
    let a = app.make_user(UserSpec::new()).await;
    let b = app.make_user(UserSpec::new()).await;
    let ch = app
        .post(
            &a.token,
            &format!("{M}/channels"),
            json!({"name": u("memo-card"), "type": "public"}),
        )
        .await;
    assert_eq!(ch.status, 201, "{}", ch.text());
    let cid = ch.json()["id"].as_str().unwrap().to_string();
    let inv = app
        .post(
            &a.token,
            &format!("{M}/channels/{cid}/invite"),
            json!({"user_id": b.id.to_string()}),
        )
        .await;
    assert!(inv.status < 300, "{}", inv.text());

    let sent = app
        .post(
            &a.token,
            &format!("{M}/memos"),
            json!({"subject": "Q2 reports", "memo_type": "directive", "body": "Due Friday", "channel_id": cid}),
        )
        .await;
    assert!(sent.status < 300, "{}", sent.text());
    let msgs = app
        .get(&b.token, &format!("{M}/channels/{cid}/messages"))
        .await
        .json();
    let card = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["message_type"] == json!("memo"))
        .expect("the memo message")
        .clone();
    let memo_id = card["memo_id"]
        .as_str()
        .expect("memo_id on the channel message");
    let memo = app.get(&b.token, &format!("{M}/memos/{memo_id}")).await;
    assert_eq!(memo.status, 200, "{}", memo.text());
    assert_eq!(memo.json()["memo_type"], json!("directive"));

    let dm = app
        .post(
            &a.token,
            &format!("{M}/memos"),
            json!({"subject": "Leave", "memo_type": "information", "body": "Noted", "recipient_id": b.id.to_string()}),
        )
        .await;
    assert!(dm.status < 300, "{}", dm.text());
    let msgs = app
        .get(&b.token, &format!("{M}/messages/{}", a.id))
        .await
        .json();
    let card = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["message_type"] == json!("memo"))
        .expect("the DM memo message")
        .clone();
    let memo_id = card["memo_id"].as_str().expect("memo_id on the DM");
    assert_eq!(
        app.get(&b.token, &format!("{M}/memos/{memo_id}"))
            .await
            .status,
        200
    );
}

/// `GET /messaging/people`: any internal user (no permissions at all) gets the messaging directory of active
/// internal users with only id/name/avatar/department/position; portal and inactive accounts are not in it,
/// a portal account cannot read it, and the admin users list stays 403 for staff.
#[tokio::test]
async fn people_directory_for_staff() {
    let app = app();
    let staff = app.make_user(UserSpec::new().name(&u("Dir Staff"))).await;
    let colleague = app
        .make_user(UserSpec::new().name(&u("Dir Colleague")))
        .await;
    let gone = app.make_user(UserSpec::new()).await;
    let partner = app.make_user(UserSpec::new().portal("partner")).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(gone.id)
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET department = 'Sales' WHERE id = $1")
        .bind(colleague.id)
        .execute(&mut db)
        .await
        .unwrap();

    assert_eq!(
        app.get(&staff.token, "/api/v1/users?limit=200")
            .await
            .status,
        403
    );
    let res = app.get(&staff.token, &format!("{M}/people")).await;
    assert_eq!(res.status, 200, "{}", res.text());
    let rows = res.json();
    let rows = rows.as_array().unwrap();
    let find = |id: Uuid| rows.iter().find(|r| r["id"] == json!(id.to_string()));
    let c = find(colleague.id).expect("an active colleague is listed");
    assert_eq!(c["department"], json!("Sales"));
    let mut keys: Vec<&String> = c.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["avatar_url", "department", "id", "name", "position"]
    );
    assert!(find(staff.id).is_some());
    assert!(find(gone.id).is_none(), "inactive users are not listed");
    assert!(find(partner.id).is_none(), "portal accounts are not listed");

    assert_eq!(
        app.get(&partner.token, &format!("{M}/people")).await.status,
        403
    );
}

/// 20 Nov 2026: a private channel's owner invites and removes members from the screen; nobody else can, an
/// owner is never removed, and the removed member loses the channel.
#[tokio::test]
async fn private_channel_owner_invites_and_removes_members() {
    let app = app();
    let owner = app.make_user(UserSpec::new()).await;
    let b = app.make_user(UserSpec::new()).await;
    let c = app.make_user(UserSpec::new()).await;
    let ch = app
        .post(
            &owner.token,
            &format!("{M}/channels"),
            json!({"name": u("priv-members"), "type": "private"}),
        )
        .await;
    assert!(ch.status < 300, "{}", ch.text());
    let cid = ch.json()["id"].as_str().unwrap().to_string();
    let invite = |token: String, who: String| {
        let path = format!("{M}/channels/{cid}/invite");
        async move { app.post(&token, &path, json!({"user_id": who})).await }
    };
    // a plain member cannot invite into a private channel
    assert_eq!(
        invite(owner.token.clone(), b.id.to_string()).await.status,
        201
    );
    assert_eq!(invite(b.token.clone(), c.id.to_string()).await.status, 403);
    assert_eq!(
        invite(owner.token.clone(), c.id.to_string()).await.status,
        201
    );
    let members = app
        .get(&c.token, &format!("{M}/channels/{cid}"))
        .await
        .json()["members"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(members, 3);

    // only the owner removes, and never an owner
    let rm = |who: Uuid| format!("{M}/channels/{cid}/members/{who}");
    assert_eq!(app.delete(&b.token, &rm(c.id)).await.status, 403);
    assert_eq!(app.delete(&owner.token, &rm(owner.id)).await.status, 400);
    let res = app.delete(&owner.token, &rm(c.id)).await;
    assert_eq!(res.status, 204, "{}", res.text());
    assert_eq!(app.delete(&owner.token, &rm(c.id)).await.status, 404);
    assert_eq!(
        app.get(&c.token, &format!("{M}/channels/{cid}"))
            .await
            .status,
        403
    );
    let left = app
        .get(&owner.token, &format!("{M}/channels/{cid}"))
        .await
        .json();
    let ids: Vec<&str> = left["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user_id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&c.id.to_string().as_str()));
    assert!(ids.contains(&b.id.to_string().as_str()));
}

/// Batch E: who may invite, and whom. Members invite to a public channel (someone outside it cannot add
/// people to it), owners to a private one; only an active staff user can be invited, never a partner or
/// vendor portal account or a deactivated user.
#[tokio::test]
async fn channel_invites_need_a_member_and_an_active_staff_invitee() {
    let app = app();
    let owner = app.make_user(UserSpec::new()).await;
    let member = app.make_user(UserSpec::new()).await;
    let partner = app.make_user(UserSpec::new().portal("partner")).await;
    let vendor = app.make_user(UserSpec::new().portal("vendor")).await;
    let gone = app.make_user(UserSpec::new()).await;
    let mut db = app.db().await;
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(gone.id)
        .execute(&mut db)
        .await
        .unwrap();
    let ch = app
        .post(
            &owner.token,
            &format!("{M}/channels"),
            json!({"name": u("pub-invites"), "type": "public"}),
        )
        .await;
    assert_eq!(ch.status, 201, "{}", ch.text());
    let cid = ch.json()["id"].as_str().unwrap().to_string();
    // A new public channel takes in every active staff user: these two join the company afterwards.
    let outsider = app.make_user(UserSpec::new()).await;
    let newcomer = app.make_user(UserSpec::new()).await;
    let path = format!("{M}/channels/{cid}/invite");
    let invite = |token: String, who: Uuid| {
        let path = path.clone();
        async move {
            app.post(&token, &path, json!({"user_id": who.to_string()}))
                .await
        }
    };

    assert_eq!(invite(owner.token.clone(), member.id).await.status, 201);
    // someone outside the channel cannot add people to it
    let res = invite(outsider.token.clone(), newcomer.id).await;
    assert_eq!(res.status, 403, "{}", res.text());
    // a member can
    let res = invite(member.token.clone(), newcomer.id).await;
    assert_eq!(res.status, 201, "{}", res.text());
    // never a portal account or a deactivated user
    for who in [partner.id, vendor.id, gone.id, Uuid::new_v4()] {
        let res = invite(owner.token.clone(), who).await;
        assert_eq!(res.status, 400, "{who}: {}", res.text());
    }
    let members: Vec<String> = app
        .get(&owner.token, &format!("{M}/channels/{cid}"))
        .await
        .json()["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user_id"].as_str().unwrap().to_string())
        .collect();
    assert!(members.contains(&newcomer.id.to_string()));
    for who in [partner.id, vendor.id, gone.id, outsider.id] {
        assert!(!members.contains(&who.to_string()), "{who}");
    }
}

/// Batch E: the @mention search offers staff only, never a partner or vendor portal account.
#[tokio::test]
async fn mention_search_leaves_out_portal_accounts() {
    let app = app();
    let staff = app.make_user(UserSpec::new()).await;
    let tag = u("Mentionable");
    let colleague = app
        .make_user(UserSpec::new().name(&format!("{tag} Staff")))
        .await;
    let partner = app
        .make_user(
            UserSpec::new()
                .name(&format!("{tag} Partner"))
                .portal("partner"),
        )
        .await;
    let vendor = app
        .make_user(
            UserSpec::new()
                .name(&format!("{tag} Vendor"))
                .portal("vendor"),
        )
        .await;
    let res = app
        .get(&staff.token, &format!("{M}/mention/users?q={tag}&limit=50"))
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let ids: Vec<String> = res
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&colleague.id.to_string()), "{ids:?}");
    assert!(!ids.contains(&partner.id.to_string()), "{ids:?}");
    assert!(!ids.contains(&vendor.id.to_string()), "{ids:?}");
}
