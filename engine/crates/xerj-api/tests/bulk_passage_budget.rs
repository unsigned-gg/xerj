//! `_bulk` on a `semantic_text` index is refused whole with HTTP 429 and a
//! `Retry-After` header when its estimated passages exceed
//! `embedding.bulk_passage_budget_secs` times the measured embedding
//! throughput. Smaller bulks are admitted.
//!
//! This file is its own test binary, so seeding the process-wide throughput
//! below cannot reach any other test.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn app(budget_secs: u64) -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.path().to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    config.embedding.bulk_passage_budget_secs = budget_secs;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);
    (xerj_api::router::build_es_compat_router(state), dir)
}

async fn create_index(app: &axum::Router) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/budget")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"mappings": {"properties": {"sem": {"type": "semantic_text"}}}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
}

/// One index action whose document is about `bytes` bytes long.
fn bulk_body(bytes: usize) -> String {
    let text = "word ".repeat(bytes / 5);
    format!("{{\"index\":{{\"_index\":\"budget\",\"_id\":\"1\"}}}}\n{{\"sem\":\"{text}\"}}\n")
}

async fn bulk(app: &axum::Router, body: String) -> (StatusCode, Option<String>, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        retry_after,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// 1 passage per second measured, 60 s budget: 60 passages, about 26,880 bytes.
fn seed_one_passage_per_second() {
    xerj_ai::embedder::record_neural_window(1, std::time::Duration::from_secs(1));
    assert!((xerj_ai::embedder::neural_passages_per_second() - 1.0).abs() < 1e-9);
}

#[tokio::test]
async fn bulk_over_the_passage_budget_is_429_with_retry_after_and_a_small_one_is_admitted() {
    seed_one_passage_per_second();
    let (app, _dir) = app(60).await;
    create_index(&app).await;
    // One passage already queued by another request keeps the rule live: an
    // idle node admits any single request, however large.
    let _in_flight = xerj_engine::admission::process_ledger()
        .admit(1, 1.0, 60)
        .expect("idle ledger admits");

    // 1 + about 100 passages against a budget of 60: 41 over at 1/s is 41 s.
    let (status, retry_after, body) = bulk(&app, bulk_body(448 * 100)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    let secs: u64 = retry_after.expect("Retry-After").parse().expect("integer");
    assert!((1..=60).contains(&secs), "{secs}");
    assert_eq!(body["status"], 429);
    let reason = body["error"]["reason"].as_str().unwrap();
    assert!(reason.contains("embedding passage budget"), "{reason}");
    assert!(!reason.contains("circuit breaker"), "{reason}");
    assert!(body.get("items").is_none());

    // Nothing from the refused request was written.
    let (status, _, got) = bulk(&app, bulk_body(448 * 5)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["errors"], false, "{got}");
    assert_eq!(got["items"][0]["index"]["status"], 201, "{got}");
}

#[tokio::test]
async fn a_zero_budget_turns_the_rule_off() {
    seed_one_passage_per_second();
    let (app, _dir) = app(0).await;
    create_index(&app).await;
    let (status, retry_after, got) = bulk(&app, bulk_body(448 * 100)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert!(retry_after.is_none());
    assert_eq!(got["errors"], false, "{got}");
}
