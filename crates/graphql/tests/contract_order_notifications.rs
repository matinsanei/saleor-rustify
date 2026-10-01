//! Order-notification contract: `notify_order` delivers a real message
//! through a stub SMTP server (proves recipient + subject + order facts),
//! and never fails when the mailer is down (celery parity).
//! Mirrors `saleor/order/tests/test_notifications.py` trigger coverage
//! (confirmation path end-to-end; other triggers share the helper).

use saleor_rustify_db::database_url;
use sea_orm::DatabaseConnection;
use std::sync::{Arc, Mutex};

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

/// Minimal SMTP stub: 220 → 250/250/250 → 354 → 250 → 221, captures DATA.
async fn stub_smtp(captured: Arc<Mutex<Vec<String>>>) -> u16 {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                break;
            };
            let captured = captured.clone();
            tokio::spawn(async move {
                let (rh, mut wh) = sock.into_split();
                let mut rd = BufReader::new(rh);
                wh.write_all(b"220 stub\r\n").await.unwrap();
                let mut line = String::new();
                loop {
                    line.clear();
                    if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                        break;
                    }
                    let cmd = line.trim_end().to_ascii_uppercase();
                    if cmd.starts_with("EHLO") || cmd.starts_with("MAIL ") || cmd.starts_with("RCPT ") {
                        wh.write_all(b"250 ok\r\n").await.unwrap();
                    } else if cmd == "DATA" {
                        wh.write_all(b"354 go\r\n").await.unwrap();
                        let mut data = String::new();
                        loop {
                            line.clear();
                            rd.read_line(&mut line).await.unwrap();
                            if line.trim_end() == "." {
                                break;
                            }
                            data.push_str(&line);
                        }
                        captured.lock().unwrap().push(data);
                        wh.write_all(b"250 queued\r\n").await.unwrap();
                    } else if cmd == "QUIT" {
                        wh.write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    } else {
                        wh.write_all(b"250 ok\r\n").await.unwrap();
                    }
                }
            });
        }
    });
    port
}

#[tokio::test]
async fn order_notifications_end_to_end() {
    let db = db().await;
    // A seed order with a real email.
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
    let oid: Option<uuid::Uuid> = saleor_rustify_db::entities::order_order::Entity::find()
        .select_only()
        .column(saleor_rustify_db::entities::order_order::Column::Id)
        .filter(saleor_rustify_db::entities::order_order::Column::UserEmail.ne(""))
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .flatten();
    let oid = oid.expect("seed order with email must exist");
    let want_email: String = saleor_rustify_db::entities::order_order::Entity::find_by_id(oid)
        .select_only()
        .column(saleor_rustify_db::entities::order_order::Column::UserEmail)
        .into_tuple::<String>()
        .one(&db)
        .await
        .unwrap()
        .unwrap();

    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let port = stub_smtp(captured.clone()).await;
    std::env::set_var("RUSTIFY_SMTP_HOST", "127.0.0.1");
    std::env::set_var("RUSTIFY_SMTP_PORT", port.to_string());
    std::env::set_var("RUSTIFY_MAIL_FROM", "shop@example.com");

    saleor_rustify_graphql::mail::notify_order(
        &db,
        oid,
        saleor_rustify_graphql::mail::OrderNotice::Confirmation,
    )
    .await;
    // Other kinds must at least not blow up (same helper, different copy).
    saleor_rustify_graphql::mail::notify_order(
        &db,
        oid,
        saleor_rustify_graphql::mail::OrderNotice::FulfillmentCreated {
            tracking: "TRK1".into(),
        },
    )
    .await;

    std::env::remove_var("RUSTIFY_SMTP_HOST");
    std::env::remove_var("RUSTIFY_SMTP_PORT");
    std::env::remove_var("RUSTIFY_MAIL_FROM");

    let msgs = captured.lock().unwrap().clone();
    assert_eq!(msgs.len(), 2, "two notices delivered");
    assert!(msgs[0].contains(&format!("To: {want_email}")), "{}", msgs[0]);
    assert!(msgs[0].contains("Order #"), "{}", msgs[0]);
    assert!(msgs[0].contains("confirmed"), "{}", msgs[0]);
    assert!(msgs[1].contains("TRK1"), "{}", msgs[1]);

    // Mailer down → swallowed, caller unaffected.
    std::env::set_var("RUSTIFY_SMTP_HOST", "127.0.0.1");
    std::env::set_var("RUSTIFY_SMTP_PORT", "9");
    saleor_rustify_graphql::mail::notify_order(
        &db,
        oid,
        saleor_rustify_graphql::mail::OrderNotice::Canceled,
    )
    .await;
    std::env::remove_var("RUSTIFY_SMTP_HOST");
    std::env::remove_var("RUSTIFY_SMTP_PORT");
}
