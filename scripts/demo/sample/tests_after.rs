mod common;

use axum::http::StatusCode;
use common::{signed_event, TestApp};

#[tokio::test]
async fn rejects_a_bad_signature() {
    let app = TestApp::spawn().await;
    let res = app.post_webhook(b"{}", "t=1,v1=deadbeef").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn credits_a_succeeded_payment() {
    let app = TestApp::spawn().await;
    let (body, sig) = signed_event("payment_intent.succeeded", 4_200);
    assert_eq!(app.post_webhook(&body, &sig).await.status(), StatusCode::OK);
    assert_eq!(app.balance_cents().await, 4_200);
}

#[tokio::test]
async fn a_redelivered_event_is_applied_once() {
    let app = TestApp::spawn().await;
    let (body, sig) = signed_event("payment_intent.succeeded", 4_200);
    for _ in 0..3 {
        assert_eq!(app.post_webhook(&body, &sig).await.status(), StatusCode::OK);
    }
    assert_eq!(app.balance_cents().await, 4_200);
    assert_eq!(app.ledger_entries().await, 1);
}

#[tokio::test]
async fn concurrent_deliveries_are_applied_once() {
    let app = TestApp::spawn().await;
    let (body, sig) = signed_event("payment_intent.succeeded", 1_500);
    let sends = (0..16).map(|_| app.post_webhook(&body, &sig));
    let statuses = futures::future::join_all(sends).await;
    assert!(statuses.iter().all(|r| r.status() == StatusCode::OK));
    assert_eq!(app.balance_cents().await, 1_500);
}
