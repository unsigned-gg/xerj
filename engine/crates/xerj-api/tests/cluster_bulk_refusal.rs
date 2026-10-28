//! A `_bulk` the embedding passage budget refuses must survive the cluster
//! path as a 429 with a wait, not as an empty 200 or per-item 500s.
//!
//! This file is its own test binary, so seeding the process-wide throughput
//! and parking passages in the process ledger cannot reach other tests.

use std::time::Duration;

use serde_json::{json, Value};
use xerj_cluster::{ShardHandler, ShardOp};

fn state() -> (xerj_api::state::AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.path().to_string_lossy().into_owned();
    let metrics = xerj_common::metrics::Metrics::new().unwrap();
    let engine = xerj_engine::Engine::new(config.clone()).unwrap();
    engine
        .create_index("refusal-docs", xerj_common::Schema::empty())
        .unwrap();
    engine
        .put_index_mapping(
            "refusal-docs",
            json!({"properties": {"sem": {"type": "semantic_text"}}}),
        )
        .unwrap();
    (xerj_api::state::AppState::new(config, engine, metrics), dir)
}

/// A 60-passage budget at 1 passage per second, with 1000 passages already
/// queued by someone else, so any semantic bulk is over budget. Dropping the
/// guard returns the parked passages.
fn busy_embedder() -> xerj_engine::admission::PassageGuard {
    xerj_ai::embedder::record_neural_window(1, Duration::from_secs(1));
    xerj_engine::admission::process_ledger()
        .admit(1000, 0.0, 60)
        .unwrap()
}

fn big_bulk() -> String {
    let text = "word ".repeat(448 * 4 / 5);
    format!("{{\"index\":{{\"_index\":\"refusal-docs\",\"_id\":\"1\"}}}}\n{{\"sem\":\"{text}\"}}\n")
}

/// A primary that refuses a forwarded bulk answers 429 and says how long to
/// wait, not an empty 200.
#[tokio::test]
async fn a_primary_that_refuses_a_bulk_answers_429_with_a_retry_after() {
    let (state, _dir) = state();
    let queued = busy_embedder();
    let handler = xerj_api::cluster_shard::ApiShardHandler::new(state);
    let reply = handler
        .handle(ShardOp::Bulk {
            body: big_bulk(),
            default_index: None,
            require_alias: None,
            default_pipeline: None,
            default_source_req_json: None,
        })
        .await;
    drop(queued);
    assert_eq!(reply.status, 429, "{}", reply.body);
    let body: Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(body["error"]["type"], "xerj_bulk_admission_exception");
    let secs = body["error"]["retry_after_secs"].as_u64().unwrap();
    assert!((1..=60).contains(&secs), "{secs}");
}

/// When every group of a cluster bulk is refused nothing was written, so the
/// accepting node refuses the whole request, with its Retry-After.
#[tokio::test]
async fn a_cluster_bulk_refused_by_every_group_is_refused_whole() {
    let (state, _dir) = state();
    let queued = busy_embedder();
    let (result, wire) = xerj_api::cluster_shard::bulk_on_primaries(
        &state,
        None,
        &big_bulk(),
        xerj_engine::bulk::BulkOpts::default(),
    )
    .await;
    drop(queued);
    let refusal = result.refusal.expect("whole-request refusal");
    assert!((1..=60).contains(&refusal.retry_after_secs));
    assert!(result.items.is_empty() && result.errors && wire.is_empty());
}
