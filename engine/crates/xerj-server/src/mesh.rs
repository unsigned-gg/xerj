//! Joined Cortex mesh presence (`service-xerj`) for `xerj-server`.
//!
//! When `CORTEX_MESH_URL` (or `CORTEX_API`) and `CORTEX_MESH_CRED_DIR` are
//! configured with `peer_id` and `token`, `xerj-server` heartbeats
//! `/v1/mesh/heartbeat`, drains `/v1/mesh/recv`, acknowledges messages via
//! `/v1/mesh/ack`, and emits a rate-limited `kind: "ack"` receipt containing
//! live engine health (`status`, `index_count`, `total_docs`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use xerj_engine::Engine;

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const RECEIPT_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct MeshCredentials {
    pub api_url: String,
    pub peer_id: String,
    pub token: String,
    pub role: String,
    pub interval: Duration,
}

#[derive(Serialize)]
struct HeartbeatReq<'a> {
    peer_id: &'a str,
    status: &'a str,
    project: &'a str,
}

#[derive(Serialize)]
struct RecvReq<'a> {
    peer_id: &'a str,
    limit: usize,
}

#[derive(Deserialize)]
struct RecvResp {
    #[serde(default)]
    messages: Vec<InboxMsg>,
}

#[derive(Deserialize)]
struct InboxMsg {
    id: String,
    from: String,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    kind: String,
}

#[derive(Serialize)]
struct AckReq<'a> {
    peer_id: &'a str,
    message_ids: Vec<&'a str>,
}

#[derive(Serialize)]
struct SendReq<'a> {
    from_peer_id: &'a str,
    to: &'a str,
    kind: &'a str,
    subject: String,
    body: String,
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn load_credentials_from(
    api_url: Option<&str>,
    cred_dir: Option<&Path>,
    role: Option<&str>,
) -> Option<MeshCredentials> {
    let api_url = api_url?.trim().trim_end_matches('/').to_string();
    if api_url.is_empty() {
        return None;
    }
    let dir = cred_dir?;
    let peer_id = read_trimmed(&dir.join("peer_id"))?;
    let token = read_trimmed(&dir.join("token"))?;
    let hb_secs: u64 = read_trimmed(&dir.join("heartbeat_secs"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let interval = Duration::from_secs((hb_secs / 2).clamp(15, 60));
    let role = role
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("service-xerj")
        .to_string();
    Some(MeshCredentials {
        api_url,
        peer_id,
        token,
        role,
        interval,
    })
}

pub fn spawn_from_env(engine: Arc<Engine>) -> Option<JoinHandle<()>> {
    let api_url = std::env::var("CORTEX_MESH_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("CORTEX_API")
                .ok()
                .filter(|s| !s.trim().is_empty())
        });
    let cred_dir = std::env::var("CORTEX_MESH_CRED_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from);
    let role = std::env::var("CORTEX_MESH_ROLE").ok();
    let creds = load_credentials_from(api_url.as_deref(), cred_dir.as_deref(), role.as_deref())?;
    let http = Client::builder().timeout(HTTP_TIMEOUT).build().ok()?;
    tracing::info!(
        role = %creds.role,
        peer_id = %creds.peer_id,
        api_url = %creds.api_url,
        interval_secs = creds.interval.as_secs(),
        "xerj cortex mesh presence enabled"
    );
    Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(creds.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut cooldowns: HashMap<String, Instant> = HashMap::new();
        loop {
            ticker.tick().await;
            tick_once(&http, &creds, &engine, &mut cooldowns).await;
        }
    }))
}

async fn tick_once(
    http: &Client,
    creds: &MeshCredentials,
    engine: &Engine,
    cooldowns: &mut HashMap<String, Instant>,
) {
    let hb = HeartbeatReq {
        peer_id: &creds.peer_id,
        status: "idle",
        project: "unsigned-gg/xerj",
    };
    let _ = http
        .post(format!("{}/v1/mesh/heartbeat", creds.api_url))
        .bearer_auth(&creds.token)
        .json(&hb)
        .send()
        .await;

    let recv = RecvReq {
        peer_id: &creds.peer_id,
        limit: 20,
    };
    let Ok(resp) = http
        .post(format!("{}/v1/mesh/recv", creds.api_url))
        .bearer_auth(&creds.token)
        .json(&recv)
        .send()
        .await
    else {
        return;
    };
    if !resp.status().is_success() {
        return;
    }
    let Ok(inbox) = resp.json::<RecvResp>().await else {
        return;
    };
    if inbox.messages.is_empty() {
        return;
    }

    let ids: Vec<&str> = inbox.messages.iter().map(|m| m.id.as_str()).collect();
    let ack = AckReq {
        peer_id: &creds.peer_id,
        message_ids: ids,
    };
    let _ = http
        .post(format!("{}/v1/mesh/ack", creds.api_url))
        .bearer_auth(&creds.token)
        .json(&ack)
        .send()
        .await;

    let health = engine.health().await;
    let now = Instant::now();
    for msg in &inbox.messages {
        if msg.kind != "msg" || msg.from == creds.role || msg.from == creds.peer_id {
            continue;
        }
        if cooldowns
            .get(&msg.from)
            .is_some_and(|prev| now.duration_since(*prev) < RECEIPT_COOLDOWN)
        {
            continue;
        }
        cooldowns.insert(msg.from.clone(), now);
        let subj = msg.subject.as_deref().unwrap_or("(no subject)");
        let receipt = SendReq {
            from_peer_id: &creds.peer_id,
            to: &msg.from,
            kind: "ack",
            subject: format!("ack: {subj}"),
            body: format!(
                "{} received mesh message `{}` (status={}, indices={}, total_docs={})",
                creds.role, msg.id, health.status, health.index_count, health.total_docs
            ),
        };
        let _ = http
            .post(format!("{}/v1/mesh/send", creds.api_url))
            .bearer_auth(&creds.token)
            .json(&receipt)
            .send()
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_xerj_mesh_credentials_and_defaults_role() {
        let dir = std::env::temp_dir().join(format!("xerj-mesh-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("peer_id"), "peer-xerj-0\n").unwrap();
        std::fs::write(dir.join("token"), "tok-secret\n").unwrap();
        std::fs::write(dir.join("heartbeat_secs"), "60\n").unwrap();

        let creds = load_credentials_from(Some("http://127.0.0.1:8788/"), Some(&dir), None)
            .expect("credentials");
        assert_eq!(creds.api_url, "http://127.0.0.1:8788");
        assert_eq!(creds.peer_id, "peer-xerj-0");
        assert_eq!(creds.role, "service-xerj");
        assert_eq!(creds.interval, Duration::from_secs(30));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
