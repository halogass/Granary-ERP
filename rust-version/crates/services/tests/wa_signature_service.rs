//! Ports of backend/tests/test_wa_signature.py at the service level. The Python file's rule tests
//! (short_name, sign, strip_signature, message_key) are below verbatim; the HTTP tests (/whatsapp/send,
//! /send-media, /forward, /messages, /auth/me, /users) exercise router code that calls these functions, so
//! what they check about this module (the signed text, the "sent by" log across the @lid switch, the
//! superadmin's verbatim text, the scheduled-message signature) is checked here directly.

mod support;

use granary_db::rows::auth::User;
use granary_services::wa_signature::{self, message_key, short_name, sign, strip_signature};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

const CHAT: &str = "8181234567890@c.us";

async fn user(
    pool: &PgPool,
    name: &str,
    superadmin: bool,
    sig_name: Option<&str>,
    custom: Option<&str>,
) -> User {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, \
         wa_signature_name, wa_signature_custom) VALUES ($1, $2, $3, 'x', true, $4, false, 'internal', $5, $6)",
    )
    .bind(id)
    .bind(format!("pytest.wasig.{}@granary.test", &Uuid::new_v4().simple().to_string()[..8]))
    .bind(name)
    .bind(superadmin)
    .bind(sig_name)
    .bind(custom)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn company(pool: &PgPool) -> String {
    let mut c = pool.acquire().await.unwrap();
    granary_services::email_marketing::get_company_info(&mut c)
        .await
        .unwrap()
        .company_name
}

#[test]
fn test_short_name() {
    for (full, expected) in [
        ("Bob Sanders Wilson", "Bob S. W."),
        ("Bob", "Bob"),
        ("  bob   sanders  ", "bob S."),
        ("Bob Sanders, S.T.", "Bob S."),
        ("", ""),
    ] {
        assert_eq!(short_name(Some(full)), expected);
    }
}

#[test]
fn test_sign_adds_the_sign_off_once() {
    let sig = "_\u{2014} Bob S. W., Acme Ltd_";
    let once = sign(Some("Hello Mr"), sig);
    assert_eq!(once, format!("Hello Mr\n\n{sig}"));
    assert_eq!(sign(Some(&once), sig), once);
    assert_eq!(sign(Some(""), sig), sig);
    assert_eq!(sign(None, sig), sig);
}

#[test]
fn test_an_imitated_sign_off_is_stripped_from_ai_drafts() {
    assert_eq!(
        strip_signature(Some(
            "Sure Mr, we will send it shortly.\n\n_\u{2014} Rachel A., Acme Ltd_"
        )),
        "Sure Mr, we will send it shortly."
    );
    assert_eq!(
        strip_signature(Some("No sign-off here")),
        "No sign-off here"
    );
}

#[test]
fn test_the_message_key_survives_the_contact_id_form_changing() {
    assert_eq!(
        message_key(Some(&format!("true_{CHAT}_3EB0ABC"))).as_deref(),
        Some("3EB0ABC")
    );
    assert_eq!(
        message_key(Some("true_123456789@lid_3EB0ABC")).as_deref(),
        Some("3EB0ABC")
    );
    assert_eq!(
        message_key(Some("true_120363@g.us_3EB0XYZ_81811@c.us")).as_deref(),
        Some("3EB0XYZ")
    );
}

/// test_a_reply_is_signed_and_its_sender_logged + test_the_inbox_shows_who_sent_our_messages +
/// test_a_name_override_is_used + test_the_superadmins_custom_text_is_used_verbatim +
/// test_scheduled_messages_are_signed_with_whoever_scheduled_them.
#[tokio::test]
async fn signing_and_the_sent_by_log() {
    let pool = support::fresh_db("s6_wa_signature").await;
    let co = company(&pool).await;
    let bob = user(&pool, "Bob Sanders Wilson", false, None, None).await;
    let mut conn = pool.acquire().await.unwrap();

    // The reply is signed with the short name and the company.
    let sig = wa_signature::signature_for(&mut conn, Some(&bob))
        .await
        .unwrap();
    assert_eq!(
        sign(Some("Hello Mr Andy"), &sig),
        format!("Hello Mr Andy\n\n_\u{2014} Bob S. W., {co}_")
    );
    assert_eq!(wa_signature::display_name(&bob), "Bob S. W.");

    // Logged by key; the inbox later sees the same message under the @lid id.
    let key = Uuid::new_v4().simple().to_string()[..16].to_uppercase();
    wa_signature::record_sent(
        &mut conn,
        Some(&format!("true_{CHAT}_{key}")),
        CHAT,
        Some(&bob),
    )
    .await
    .unwrap();
    wa_signature::record_sent(
        &mut conn,
        Some(&format!("true_{CHAT}_{key}")),
        CHAT,
        Some(&bob),
    )
    .await
    .unwrap(); // once
    let (uid, name, chat): (Option<Uuid>, String, Option<String>) = sqlx::query_as(
        "SELECT user_id, sender_name, chat_id FROM wa_sent_messages WHERE message_key = $1",
    )
    .bind(&key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (uid, name.as_str(), chat.as_deref()),
        (Some(bob.id), "Bob S. W.", Some(CHAT))
    );
    let ours = format!("true_987654@lid_{key}");
    let by = wa_signature::sent_by(
        &mut conn,
        &[Some(&ours), Some("false_987654@lid_OTHER"), None, Some("")],
    )
    .await
    .unwrap();
    assert_eq!(by.len(), 1);
    assert_eq!(by.get(&ours).map(String::as_str), Some("Bob S. W."));
    assert!(wa_signature::sent_by(&mut conn, &[None])
        .await
        .unwrap()
        .is_empty());

    // No id: nothing logged. No user: "System".
    wa_signature::record_sent(&mut conn, None, CHAT, Some(&bob))
        .await
        .unwrap();
    wa_signature::record_sent(&mut conn, Some("true_x_ANON1"), CHAT, None)
        .await
        .unwrap();
    let anon: (Option<Uuid>, String) = sqlx::query_as(
        "SELECT user_id, sender_name FROM wa_sent_messages WHERE message_key = 'ANON1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(anon, (None, "System".to_string()));

    // A name override.
    let rob = user(&pool, "Robert James Smith", false, Some("Rob S."), None).await;
    assert!(sign(
        Some("Sure"),
        &wa_signature::signature_for(&mut conn, Some(&rob))
            .await
            .unwrap()
    )
    .ends_with(&format!("_\u{2014} Rob S., {co}_")));

    // The superadmin's own text is used verbatim; anyone else's custom text is ignored.
    let owner = user(
        &pool,
        "David Doe",
        true,
        None,
        Some("David | Owner, Acme Ltd"),
    )
    .await;
    assert_eq!(
        sign(
            Some("Hello"),
            &wa_signature::signature_for(&mut conn, Some(&owner))
                .await
                .unwrap()
        ),
        "Hello\n\nDavid | Owner, Acme Ltd"
    );
    let staff = user(&pool, "Sys Admin", false, None, Some("Hijacked")).await;
    assert_eq!(
        wa_signature::signature_for(&mut conn, Some(&staff))
            .await
            .unwrap(),
        format!("_\u{2014} Sys A., {co}_")
    );

    // Scheduled messages: signed with whoever scheduled them; none means the company alone.
    let rachel = user(&pool, "Rachel Amy", false, None, None).await;
    let signed = sign(
        Some("Reminder: invoice due tomorrow"),
        &wa_signature::signature_for(&mut conn, Some(&rachel))
            .await
            .unwrap(),
    );
    assert!(signed.ends_with(&format!("_\u{2014} Rachel A., {co}_")));
    assert_eq!(
        wa_signature::signature_for(&mut conn, None).await.unwrap(),
        format!("_\u{2014} {co}_")
    );

    // record_sent commits like Python's db.commit(): inside a caller's transaction the row survives the
    // caller rolling back afterwards.
    let mut tx = conn.begin().await.unwrap();
    wa_signature::record_sent(&mut tx, Some("true_x_INTX1"), CHAT, Some(&bob))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM wa_sent_messages WHERE message_key = 'INTX1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 1);
}
