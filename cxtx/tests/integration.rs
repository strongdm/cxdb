use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::Duration;

use assert_cmd::prelude::*;
use axum::body::Body;
use axum::extract::Path as AxumPath;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::Router;
use cxdb_server::events::EventBus;
use cxdb_server::http::start_http;
use cxdb_server::metrics::{Metrics, SessionTracker};
use cxdb_server::registry::Registry;
use cxdb_server::store::Store;
use futures_util::{SinkExt, StreamExt};
use predicates::prelude::*;
use reqwest::Client;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as WsRequest, Response as WsResponse,
};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use cxtx::cxdb_http::CxdbHttpClient;
use cxtx::delivery::DeliveryHandle;
use cxtx::ledger::SessionLedgerWriter;
use cxtx::provider::ProviderKind;
use cxtx::session::SessionRuntime;

#[tokio::test(flavor = "multi_thread")]
async fn first_record_metadata_is_queryable_via_cxdb_http() {
    let _scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let session = SessionRuntime::new(
        ProviderKind::Codex,
        vec!["--help".to_string()],
        BTreeMap::new(),
    )
    .unwrap();
    let client =
        CxdbHttpClient::new(cxdb.base_url.parse().unwrap(), "cxtx-tests".to_string()).unwrap();
    let context_id = client.create_context().await.unwrap();
    client
        .append_turn(context_id, &session.session_start_turn().item)
        .await
        .unwrap();

    let contexts = client.list_contexts().await.unwrap();
    let listed = contexts["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| json_u64(&context["context_id"]) == Some(context_id))
        .cloned()
        .unwrap();
    assert_eq!(listed["client_tag"], "cxtx/codex");
    assert!(listed["title"]
        .as_str()
        .unwrap()
        .contains("cxtx/codex codex"));
    assert!(listed["labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|label| label == "interactive"));

    let provenance = client.get_provenance(context_id).await.unwrap();
    assert_eq!(provenance["provenance"]["service_name"], "cxtx");
    assert_eq!(provenance["provenance"]["on_behalf_of_source"], "cli");
}

#[tokio::test(flavor = "multi_thread")]
async fn registry_bundle_is_published_before_first_append() {
    let _scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let session = SessionRuntime::new(
        ProviderKind::Codex,
        vec!["--help".to_string()],
        BTreeMap::new(),
    )
    .unwrap();
    let client =
        CxdbHttpClient::new(cxdb.base_url.parse().unwrap(), "cxtx-tests".to_string()).unwrap();
    let context_id = client.create_context().await.unwrap();
    client
        .append_turn(context_id, &session.session_start_turn().item)
        .await
        .unwrap();

    let descriptor = cxdb
        .registry_type("cxdb.ConversationItem", 3)
        .await
        .unwrap();
    assert_eq!(descriptor["fields"]["1"]["name"], "item_type");
    assert_eq!(descriptor["fields"]["30"]["name"], "context_metadata");
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_wrapper_preserves_child_io_and_uploads_canonical_turns() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockOpenAi::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    let fixture_dir = scratch.dir.path().join("fixtures-codex");
    fs::create_dir_all(&fixture_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("codex"),
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$OPENAI_BASE_URL" > "{fixture}/openai_base_url_env.txt"
printf '%s\n' "$OPENAI_API_BASE" > "{fixture}/openai_api_base_env.txt"
printf '%s\n' "$CXTX_OPENAI_BASE_URL" > "{fixture}/openai_base_url.txt"
printf '%s\n' "$CXTX_OPENAI_API_BASE" > "{fixture}/openai_api_base.txt"
printf '%s\n' "$@" > "{fixture}/args.txt"
python3 - <<'PY'
import json
import os
import sys
import urllib.request

base = os.environ["CXTX_OPENAI_BASE_URL"].rstrip("/")
req = urllib.request.Request(
    base + "/chat/completions",
    data=json.dumps({{
        "model": "gpt-5",
        "messages": [{{"role": "user", "content": "hello"}}],
        "stream": False
    }}).encode(),
    headers={{
        "Content-Type": "application/json",
        "Authorization": "Bearer test-openai"
    }},
)
with urllib.request.urlopen(req) as resp:
    sys.stdout.write(resp.read().decode())
PY
printf 'codex-child-stderr\n' >&2
"#,
            fixture = fixture_dir.display(),
        ),
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("OPENAI_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--")
        .arg("--model")
        .arg("gpt-5");
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"id\":\"chatcmpl_123\""));
    assert!(String::from_utf8_lossy(&output.stderr).contains("codex-child-stderr"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("cxtx:"));

    assert!(fs::read_to_string(fixture_dir.join("openai_base_url.txt"))
        .unwrap()
        .contains("/v1"));
    assert!(fs::read_to_string(fixture_dir.join("openai_base_url_env.txt"))
        .unwrap()
        .contains("/v1"));
    assert_eq!(
        fs::read_to_string(fixture_dir.join("openai_api_base_env.txt")).unwrap(),
        fs::read_to_string(fixture_dir.join("openai_api_base.txt")).unwrap()
    );
    let args = fs::read_to_string(fixture_dir.join("args.txt")).unwrap();
    assert!(!args.contains("openai_base_url="));
    assert!(args.starts_with("-c\nprefer_websockets=false\n"));
    assert!(args.contains("--disable\nresponses_websockets\n"));
    assert!(args.contains("--disable\nresponses_websockets_v2\n"));
    assert!(args.ends_with("--model\ngpt-5\n"));

    let contexts = cxdb.list_contexts().await.unwrap();
    let context_id = first_context_id(&contexts);
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec!["system", "user_input", "assistant_turn", "system"]
    );
    assert_eq!(turns[1]["data"]["user_input"]["text"], "hello");
    assert_eq!(turns[2]["data"]["turn"]["text"], "hi");
    assert_eq!(turns[0]["data"]["system"]["title"], "session_start");
    assert_eq!(turns[3]["data"]["system"]["title"], "session_end");

    let ledger = find_single_ledger(scratch.dir.path());
    assert_eq!(ledger["provider_kind"], "openai");
    assert!(ledger["exchanges"][0]["request_path"]
        .as_str()
        .unwrap()
        .ends_with("request.json"));
    assert!(ledger["exchanges"][0]["response_path"]
        .as_str()
        .unwrap()
        .ends_with("response.json"));

    let recorded_requests = upstream.requests.lock().unwrap().clone();
    assert_eq!(recorded_requests.len(), 1);
    assert_eq!(recorded_requests[0]["path"], "/v1/chat/completions");
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_replay_history_suppresses_duplicate_turns() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockOpenAi::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("codex"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import urllib.request

base = os.environ["CXTX_OPENAI_BASE_URL"].rstrip("/")
headers = {
    "Content-Type": "application/json",
    "Authorization": "Bearer test-openai",
}

def post(messages):
    req = urllib.request.Request(
        base + "/chat/completions",
        data=json.dumps({
            "model": "gpt-5",
            "messages": messages,
            "stream": False
        }).encode(),
        headers=headers,
    )
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read().decode())

first = post([{"role": "user", "content": "hello"}])
assistant = first["choices"][0]["message"]["content"]
post([
    {"role": "user", "content": "hello"},
    {"role": "assistant", "content": assistant},
    {"role": "user", "content": "tell me more"},
])
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("OPENAI_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--");
    command.assert().success();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec![
            "system",
            "user_input",
            "assistant_turn",
            "user_input",
            "assistant_turn",
            "system"
        ]
    );
    assert_eq!(turns[1]["data"]["user_input"]["text"], "hello");
    assert_eq!(turns[3]["data"]["user_input"]["text"], "tell me more");
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_history_rewrite_emits_system_turn_and_resets_suffix() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockOpenAi::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("codex"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import urllib.request

base = os.environ["CXTX_OPENAI_BASE_URL"].rstrip("/")
headers = {
    "Content-Type": "application/json",
    "Authorization": "Bearer test-openai",
}

def post(messages):
    req = urllib.request.Request(
        base + "/chat/completions",
        data=json.dumps({
            "model": "gpt-5",
            "messages": messages,
            "stream": False
        }).encode(),
        headers=headers,
    )
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read().decode())

post([{"role": "user", "content": "hello"}])
post([
    {"role": "user", "content": "start over"},
    {"role": "user", "content": "new question"},
])
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("OPENAI_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--");
    command.assert().success();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec![
            "system",
            "user_input",
            "assistant_turn",
            "system",
            "user_input",
            "user_input",
            "assistant_turn",
            "system"
        ]
    );
    assert_eq!(
        turns[3]["data"]["system"]["title"],
        "history_rewrite_detected"
    );
    assert_eq!(turns[4]["data"]["user_input"]["text"], "start over");
    assert_eq!(turns[5]["data"]["user_input"]["text"], "new question");
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_tool_result_history_uploads_tool_related_turns() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockOpenAiTooling::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("codex"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import urllib.request

base = os.environ["CXTX_OPENAI_BASE_URL"].rstrip("/")
headers = {
    "Content-Type": "application/json",
    "Authorization": "Bearer test-openai",
}

def post(messages):
    req = urllib.request.Request(
        base + "/chat/completions",
        data=json.dumps({
            "model": "gpt-5",
            "messages": messages,
            "stream": False
        }).encode(),
        headers=headers,
    )
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read().decode())

first = post([{"role": "user", "content": "use tool"}])
tool_call = first["choices"][0]["message"]["tool_calls"][0]
post([
    {"role": "user", "content": "use tool"},
    {"role": "assistant", "content": "", "tool_calls": [tool_call]},
    {"role": "tool", "tool_call_id": tool_call["id"], "content": "lookup done"},
])
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("OPENAI_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--");
    command.assert().success();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec![
            "system",
            "user_input",
            "assistant_turn",
            "tool_result",
            "assistant_turn",
            "system"
        ]
    );
    assert_eq!(turns[2]["data"]["turn"]["tool_calls"][0]["name"], "lookup");
    assert_eq!(turns[3]["data"]["tool_result"]["content"], "lookup done");
    assert_eq!(turns[4]["data"]["turn"]["text"], "tool complete");
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_responses_bootstrap_history_starts_with_real_prompt() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockOpenAi::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("codex"),
        r##"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import sys
import urllib.request

base = os.environ["CXTX_OPENAI_BASE_URL"].rstrip("/")
req = urllib.request.Request(
    base + "/responses",
    data=json.dumps({
        "model": "gpt-5.4",
        "input": [
            {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "<permissions instructions>"}]},
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "# AGENTS.md instructions for /repo\n<environment_context>\n  <cwd>/repo</cwd>\n</environment_context>"}]},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "previous answer"}]},
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "real prompt"}]}
        ]
    }).encode(),
    headers={
        "Content-Type": "application/json",
        "Authorization": "Bearer test-openai"
    },
)
with urllib.request.urlopen(req) as resp:
    sys.stdout.write(resp.read().decode())
PY
"##,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("OPENAI_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--");
    command.assert().success();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec!["system", "assistant_turn", "user_input", "assistant_turn", "system"]
    );
    assert_eq!(turns[1]["data"]["turn"]["text"], "previous answer");
    assert_eq!(turns[2]["data"]["user_input"]["text"], "real prompt");
    assert_eq!(turns[3]["data"]["turn"]["text"], "clean answer");

    let recorded_requests = upstream.requests.lock().unwrap().clone();
    assert_eq!(recorded_requests.len(), 1);
    assert_eq!(recorded_requests[0]["path"], "/v1/responses");
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_proxy_uploads_canonical_turns_into_cxdb() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockOpenAiWebsocket::start().await.unwrap();
    let session = SessionRuntime::new(
        ProviderKind::Codex,
        vec!["interactive".to_string()],
        BTreeMap::new(),
    )
    .unwrap();
    let ledger = SessionLedgerWriter::create(&session).await.unwrap();
    let proxy = cxtx::proxy::ProxyServer::start(
        ProviderKind::Codex,
        upstream.base_url.parse().unwrap(),
        session.clone(),
        ledger.clone(),
    )
    .await
    .unwrap();
    let delivery = DeliveryHandle::start(
        cxdb.base_url.parse().unwrap(),
        session.clone(),
        ledger.clone(),
        "cxtx-tests".to_string(),
    )
    .await
    .unwrap();
    proxy.set_delivery(delivery.clone()).await;
    delivery.enqueue_create_context().await.unwrap();
    delivery.enqueue_turn(session.session_start_turn()).await.unwrap();

    let mut proxy_url = proxy.proxy_base_url();
    proxy_url.set_scheme("ws").unwrap();
    proxy_url.set_path("/v1/responses");

    let (mut socket, _) = tokio_tungstenite::connect_async(proxy_url.as_str())
        .await
        .unwrap();
    socket
        .send(WsMessage::Text(
            r#"{
                "type":"response.create",
                "model":"gpt-5.4",
                "input":[
                    {"type":"message","role":"developer","content":[{"type":"input_text","text":"mode"}]},
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"websocket hello"}]}
                ]
            }"#
            .to_string(),
        ))
        .await
        .unwrap();

    while let Some(message) = socket.next().await {
        let message = message.unwrap();
        match message {
            WsMessage::Text(text) if text.contains("\"response.completed\"") => break,
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    let _ = socket.close(None).await;

    delivery
        .enqueue_turn(session.session_end_turn(0, true))
        .await
        .unwrap();
    proxy.shutdown().await.unwrap();
    delivery.shutdown().await.unwrap();
    ledger.finalize().await.unwrap();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec!["system", "user_input", "assistant_turn", "system"]
    );
    assert_eq!(turns[1]["data"]["user_input"]["text"], "websocket hello");
    assert_eq!(turns[2]["data"]["turn"]["text"], "hello from websocket");

    let ledger_json = find_single_ledger(scratch.dir.path());
    assert_eq!(ledger_json["exchanges"][0]["status_code"], 101);
    assert!(ledger_json["exchanges"][0]["stream_path"]
        .as_str()
        .unwrap()
        .ends_with("stream.ndjson"));
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_wrapper_streams_to_child_and_uploads_canonical_turns() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockClaude::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    let fixture_dir = scratch.dir.path().join("fixtures-claude");
    fs::create_dir_all(&fixture_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("claude"),
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$ANTHROPIC_BASE_URL" > "{fixture}/anthropic_base_url.txt"
printf '%s\n' "$CLAUDE_BASE_URL" > "{fixture}/claude_base_url.txt"
printf '%s\n' "$@" > "{fixture}/args.txt"
python3 - <<'PY'
import json
import os
import sys
import urllib.request

base = os.environ["ANTHROPIC_BASE_URL"].rstrip("/")
req = urllib.request.Request(
    base + "/v1/messages",
    data=json.dumps({{
        "model": "claude-3-7-sonnet-20250219",
        "max_tokens": 16,
        "stream": True,
        "messages": [{{"role": "user", "content": "hello"}}]
    }}).encode(),
    headers={{
        "Content-Type": "application/json",
        "x-api-key": "test-anthropic",
        "anthropic-version": "2023-06-01",
        "Accept": "text/event-stream"
    }},
)
with urllib.request.urlopen(req) as resp:
    for raw in resp:
        sys.stdout.write(raw.decode())
PY
printf 'claude-child-stderr\n' >&2
"#,
            fixture = fixture_dir.display(),
        ),
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("ANTHROPIC_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("claude")
        .arg("--")
        .arg("--print")
        .arg("stream");
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("event: message_start"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("claude-child-stderr"));

    let anthropic_base = fs::read_to_string(fixture_dir.join("anthropic_base_url.txt")).unwrap();
    assert!(anthropic_base.starts_with("http://127.0.0.1:"));
    assert!(!anthropic_base.contains("/v1"));

    let contexts = cxdb.list_contexts().await.unwrap();
    let context_id = first_context_id(&contexts);
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec!["system", "user_input", "assistant_turn", "system"]
    );
    assert_eq!(turns[2]["data"]["turn"]["text"], "hello from claude");

    let ledger = find_single_ledger(scratch.dir.path());
    assert!(ledger["exchanges"][0]["stream_path"]
        .as_str()
        .unwrap()
        .ends_with("stream.ndjson"));
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_replay_history_suppresses_duplicate_turns() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockClaudeJson::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("claude"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import urllib.request

base = os.environ["ANTHROPIC_BASE_URL"].rstrip("/")
headers = {
    "Content-Type": "application/json",
    "x-api-key": "test-anthropic",
    "anthropic-version": "2023-06-01",
}

def post(messages):
    req = urllib.request.Request(
        base + "/v1/messages",
        data=json.dumps({
            "model": "claude-3-7-sonnet-20250219",
            "max_tokens": 16,
            "stream": False,
            "messages": messages,
        }).encode(),
        headers=headers,
    )
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read().decode())

first = post([{"role": "user", "content": "hello"}])
assistant = first["content"][0]["text"]
post([
    {"role": "user", "content": "hello"},
    {"role": "assistant", "content": assistant},
    {"role": "user", "content": "tell me more"},
])
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("ANTHROPIC_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("claude")
        .arg("--");
    command.assert().success();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec![
            "system",
            "user_input",
            "assistant_turn",
            "user_input",
            "assistant_turn",
            "system"
        ]
    );
    assert_eq!(turns[1]["data"]["user_input"]["text"], "hello");
    assert_eq!(turns[2]["data"]["turn"]["text"], "hello from claude");
    assert_eq!(turns[3]["data"]["user_input"]["text"], "tell me more");
    assert_eq!(turns[4]["data"]["turn"]["text"], "more from claude");
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_history_rewrite_emits_system_turn_and_resets_suffix() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockClaudeJson::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("claude"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import urllib.request

base = os.environ["ANTHROPIC_BASE_URL"].rstrip("/")
headers = {
    "Content-Type": "application/json",
    "x-api-key": "test-anthropic",
    "anthropic-version": "2023-06-01",
}

def post(messages):
    req = urllib.request.Request(
        base + "/v1/messages",
        data=json.dumps({
            "model": "claude-3-7-sonnet-20250219",
            "max_tokens": 16,
            "stream": False,
            "messages": messages,
        }).encode(),
        headers=headers,
    )
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read().decode())

post([{"role": "user", "content": "hello"}])
post([
    {"role": "user", "content": "start over"},
    {"role": "user", "content": "new question"},
])
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("ANTHROPIC_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("claude")
        .arg("--");
    command.assert().success();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec![
            "system",
            "user_input",
            "assistant_turn",
            "system",
            "user_input",
            "user_input",
            "assistant_turn",
            "system"
        ]
    );
    assert_eq!(
        turns[3]["data"]["system"]["title"],
        "history_rewrite_detected"
    );
    assert_eq!(turns[4]["data"]["user_input"]["text"], "start over");
    assert_eq!(turns[5]["data"]["user_input"]["text"], "new question");
    assert_eq!(turns[6]["data"]["turn"]["text"], "fresh answer");
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_tool_result_history_uploads_tool_related_turns() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockClaudeTooling::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("claude"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import urllib.request

base = os.environ["ANTHROPIC_BASE_URL"].rstrip("/")
headers = {
    "Content-Type": "application/json",
    "x-api-key": "test-anthropic",
    "anthropic-version": "2023-06-01",
    "Accept": "text/event-stream",
}

def post(messages):
    req = urllib.request.Request(
        base + "/v1/messages",
        data=json.dumps({
            "model": "claude-3-7-sonnet-20250219",
            "max_tokens": 16,
            "stream": True,
            "messages": messages,
        }).encode(),
        headers=headers,
    )
    with urllib.request.urlopen(req) as resp:
        for _ in resp:
            pass

post([{"role": "user", "content": "use tool"}])
post([
    {"role": "user", "content": "use tool"},
    {"role": "assistant", "content": [{"type": "tool_use", "id": "call_1", "name": "lookup", "input": {"q": "use tool"}}]},
    {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": [{"type": "text", "text": "done"}]}]},
])
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("ANTHROPIC_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("claude")
        .arg("--");
    command.assert().success();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(
        item_types,
        vec![
            "system",
            "user_input",
            "assistant_turn",
            "tool_result",
            "assistant_turn",
            "system"
        ]
    );
    assert_eq!(turns[2]["data"]["turn"]["tool_calls"][0]["name"], "lookup");
    assert_eq!(turns[3]["data"]["tool_result"]["content"], "done");
    assert_eq!(turns[4]["data"]["turn"]["text"], "tool complete");

    let ledger = find_single_ledger(scratch.dir.path());
    assert!(ledger["exchanges"][0]["stream_path"]
        .as_str()
        .unwrap()
        .ends_with("stream.ndjson"));
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_upstream_error_response_emits_system_turn() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockOpenAiUpstreamError::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("codex"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import sys
import urllib.error
import urllib.request

base = os.environ["CXTX_OPENAI_BASE_URL"].rstrip("/")
req = urllib.request.Request(
    base + "/chat/completions",
    data=json.dumps({
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "hello"}],
        "stream": False
    }).encode(),
    headers={
        "Content-Type": "application/json",
        "Authorization": "Bearer test-openai"
    },
)
try:
    with urllib.request.urlopen(req) as resp:
        sys.stdout.write(resp.read().decode())
except urllib.error.HTTPError as err:
    sys.stdout.write(err.read().decode())
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("OPENAI_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--");
    command.assert().success();

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(item_types, vec!["system", "user_input", "system", "system"]);
    assert_eq!(
        turns[2]["data"]["system"]["title"],
        "provider_error_response"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_malformed_json_response_emits_system_error_turn() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockOpenAiMalformed::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("codex"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import sys
import urllib.request

base = os.environ["CXTX_OPENAI_BASE_URL"].rstrip("/")
req = urllib.request.Request(
    base + "/chat/completions",
    data=json.dumps({
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "hello"}],
        "stream": False
    }).encode(),
    headers={
        "Content-Type": "application/json",
        "Authorization": "Bearer test-openai"
    },
)
with urllib.request.urlopen(req) as resp:
    sys.stdout.write(resp.read().decode())
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("OPENAI_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--");
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "not-json");

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(item_types, vec!["system", "user_input", "system", "system"]);
    assert_eq!(turns[2]["data"]["system"]["title"], "response_parse_error");
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_malformed_stream_emits_system_error_turn() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let upstream = MockClaudeMalformed::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("claude"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import sys
import urllib.request

base = os.environ["ANTHROPIC_BASE_URL"].rstrip("/")
req = urllib.request.Request(
    base + "/v1/messages",
    data=json.dumps({
        "model": "claude-3-7-sonnet-20250219",
        "max_tokens": 16,
        "stream": True,
        "messages": [{"role": "user", "content": "hello"}]
    }).encode(),
    headers={
        "Content-Type": "application/json",
        "x-api-key": "test-anthropic",
        "anthropic-version": "2023-06-01",
        "Accept": "text/event-stream"
    },
)
with urllib.request.urlopen(req) as resp:
    for raw in resp:
        sys.stdout.write(raw.decode())
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("ANTHROPIC_BASE_URL", upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("claude")
        .arg("--");
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("event: content_block_start"));

    let context_id = first_context_id(&cxdb.list_contexts().await.unwrap());
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(item_types, vec!["system", "user_input", "system", "system"]);
    assert_eq!(turns[2]["data"]["system"]["title"], "stream_parse_error");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_child_binary_does_not_create_cxdb_context() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", scratch.dir.path())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--")
        .arg("--help");
    command
        .assert()
        .failure()
        .stderr(predicate::str::contains("failed to launch codex"));

    let contexts = cxdb.list_contexts().await.unwrap();
    assert_eq!(contexts["count"], 0);

    let ledger = find_single_ledger(scratch.dir.path());
    assert_eq!(ledger["delivery_state"], "child_launch_failed");
    assert_eq!(ledger["cxdb_context_id"], Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_cxdb_url_fails_before_session_bootstrap() {
    let scratch = ScratchRoot::new().unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .arg("--url")
        .arg("not-a-url")
        .arg("codex")
        .arg("--")
        .arg("--help");
    command
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid CXDB URL: not-a-url"));

    assert!(!scratch.dir.path().join(".scratch").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_delivery_recovers_when_cxdb_appears_later() {
    let _scratch = ScratchRoot::new().unwrap();
    let port = free_port();
    let base_url = format!("http://127.0.0.1:{port}");
    let session = SessionRuntime::new(
        ProviderKind::Codex,
        vec!["--help".to_string()],
        BTreeMap::new(),
    )
    .unwrap();
    let ledger = SessionLedgerWriter::create(&session).await.unwrap();
    let delivery = DeliveryHandle::start(
        base_url.parse().unwrap(),
        session.clone(),
        ledger.clone(),
        "cxtx/test".to_string(),
    )
    .await
    .unwrap();
    delivery.enqueue_create_context().await.unwrap();
    delivery
        .enqueue_turn(session.session_start_turn())
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(400)).await;
    let cxdb = TestCxdb::start_on_port(port).await.unwrap();
    delivery
        .enqueue_turn(session.session_end_turn(0, true))
        .await
        .unwrap();
    delivery.shutdown().await.unwrap();

    let contexts = cxdb.list_contexts().await.unwrap();
    assert_eq!(contexts["count"], 1);
    let ledger_json = serde_json::from_slice::<Value>(&fs::read(ledger.path()).unwrap()).unwrap();
    assert_eq!(ledger_json["delivery_state"], "healthy");
    assert!(ledger_json["cxdb_context_id"].as_u64().unwrap() > 0);
    assert!(ledger_json["appended_sequences"]
        .as_array()
        .unwrap()
        .iter()
        .any(|sequence| sequence == 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_delivery_recovers_from_mid_session_append_failure_in_order() {
    let _scratch = ScratchRoot::new().unwrap();
    let fake = FakeRecoveringCxdb::start().await.unwrap();
    let session = SessionRuntime::new(
        ProviderKind::Codex,
        vec!["--help".to_string()],
        BTreeMap::new(),
    )
    .unwrap();
    let ledger = SessionLedgerWriter::create(&session).await.unwrap();
    let delivery = DeliveryHandle::start(
        fake.base_url.parse().unwrap(),
        session.clone(),
        ledger.clone(),
        "cxtx/test".to_string(),
    )
    .await
    .unwrap();
    delivery.enqueue_create_context().await.unwrap();
    delivery
        .enqueue_turn(session.session_start_turn())
        .await
        .unwrap();
    delivery
        .enqueue_turn(session.session_end_turn(0, true))
        .await
        .unwrap();
    delivery.shutdown().await.unwrap();

    let state = fake.state.lock().unwrap().clone();
    assert!(state.registry_ready);
    assert_eq!(state.create_calls, 1);
    assert_eq!(
        state.stored_item_types,
        vec!["system", "system", "system", "system"]
    );
    assert_eq!(
        state.stored_system_titles,
        vec![
            "session_start",
            "session_end",
            "ingest_degraded",
            "ingest_recovered"
        ]
    );

    let ledger_json = serde_json::from_slice::<Value>(&fs::read(ledger.path()).unwrap()).unwrap();
    assert_eq!(ledger_json["delivery_state"], "healthy");
    assert_eq!(ledger_json["appended_sequences"], json!([1, 2, 3, 4]));
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_drain_records_remaining_queue_state() {
    let _scratch = ScratchRoot::new().unwrap();
    let port = free_port();
    let base_url = format!("http://127.0.0.1:{port}");
    let session = SessionRuntime::new(
        ProviderKind::Codex,
        vec!["--help".to_string()],
        BTreeMap::new(),
    )
    .unwrap();
    let ledger = SessionLedgerWriter::create(&session).await.unwrap();
    let delivery = DeliveryHandle::start(
        base_url.parse().unwrap(),
        session.clone(),
        ledger.clone(),
        "cxtx/test".to_string(),
    )
    .await
    .unwrap();
    delivery.enqueue_create_context().await.unwrap();
    delivery
        .enqueue_turn(session.session_start_turn())
        .await
        .unwrap();
    delivery.shutdown().await.unwrap();

    let ledger_json = serde_json::from_slice::<Value>(&fs::read(ledger.path()).unwrap()).unwrap();
    assert_eq!(ledger_json["delivery_state"], "degraded");
    assert!(ledger_json["queue_depth"].as_u64().unwrap() > 0);
    assert!(ledger_json["last_delivery_error"]
        .as_str()
        .unwrap()
        .contains("shutdown drain deadline reached"));
    assert_eq!(ledger_json["cxdb_context_id"], Value::Null);
    assert!(ledger_json["appended_sequences"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn child_bypassing_env_overrides_produces_only_lifecycle_turns() {
    let scratch = ScratchRoot::new().unwrap();
    let cxdb = TestCxdb::start().await.unwrap();
    let direct_upstream = MockOpenAi::start().await.unwrap();
    let fake_bin_dir = scratch.dir.path().join("bin");
    fs::create_dir_all(&fake_bin_dir).unwrap();
    write_executable(
        &fake_bin_dir.join("codex"),
        r#"#!/bin/sh
set -eu
python3 - <<'PY'
import json
import os
import sys
import urllib.request

base = os.environ["BYPASS_OPENAI_BASE_URL"].rstrip("/")
req = urllib.request.Request(
    base + "/chat/completions",
    data=json.dumps({
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "hello"}],
        "stream": False
    }).encode(),
    headers={
        "Content-Type": "application/json",
        "Authorization": "Bearer test-openai"
    },
)
with urllib.request.urlopen(req) as resp:
    sys.stdout.write(resp.read().decode())
PY
"#,
    )
    .unwrap();

    let mut command = std::process::Command::cargo_bin("cxtx").unwrap();
    command
        .current_dir(scratch.dir.path())
        .env("PATH", prepend_path(&fake_bin_dir))
        .env("OPENAI_BASE_URL", "http://ignored-by-wrapper.invalid/v1")
        .env("BYPASS_OPENAI_BASE_URL", direct_upstream.base_url.as_str())
        .arg("--url")
        .arg(&cxdb.base_url)
        .arg("codex")
        .arg("--");
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"id\":\"chatcmpl_123\""));

    let contexts = cxdb.list_contexts().await.unwrap();
    let context_id = first_context_id(&contexts);
    let turns = cxdb.turns(context_id).await.unwrap();
    let item_types = turn_item_types(&turns);
    assert_eq!(item_types, vec!["system", "system"]);

    let recorded_requests = direct_upstream.requests.lock().unwrap().clone();
    assert_eq!(recorded_requests.len(), 1);
    assert_eq!(recorded_requests[0]["path"], "/v1/chat/completions");
}

struct ScratchRoot {
    dir: TempDir,
    previous: PathBuf,
    _guard: MutexGuard<'static, ()>,
}

impl ScratchRoot {
    fn new() -> anyhow::Result<Self> {
        static CWD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = CWD_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let previous = std::env::current_dir()?;
        std::env::set_current_dir(dir.path())?;
        Ok(Self {
            dir,
            previous,
            _guard: guard,
        })
    }
}

impl Drop for ScratchRoot {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

struct TestCxdb {
    base_url: String,
}

impl TestCxdb {
    async fn start() -> anyhow::Result<Self> {
        Self::start_on_port(free_port()).await
    }

    async fn start_on_port(port: u16) -> anyhow::Result<Self> {
        let tempdir = tempfile::tempdir()?;
        let data_dir = tempdir.path().to_path_buf();
        std::mem::forget(tempdir);
        let bind_addr = format!("127.0.0.1:{port}");
        let store = Arc::new(RwLock::new(Store::open(&data_dir)?));
        let registry = Arc::new(Mutex::new(Registry::open(&data_dir.join("registry"))?));
        let metrics = Arc::new(Metrics::new(data_dir.clone()));
        let session_tracker = Arc::new(SessionTracker::new());
        let event_bus = Arc::new(EventBus::new());
        start_http(
            bind_addr.clone(),
            store,
            registry,
            metrics,
            session_tracker,
            event_bus,
        )?;
        wait_for_http(&format!("http://{bind_addr}/healthz")).await?;
        Ok(Self {
            base_url: format!("http://{bind_addr}"),
        })
    }

    async fn list_contexts(&self) -> anyhow::Result<Value> {
        Client::new()
            .get(format!(
                "{}/v1/contexts?include_provenance=1",
                self.base_url
            ))
            .send()
            .await?
            .json()
            .await
            .map_err(Into::into)
    }

    async fn turns(&self, context_id: u64) -> anyhow::Result<Vec<Value>> {
        let response: Value = Client::new()
            .get(format!(
                "{}/v1/contexts/{context_id}/turns?view=typed&limit=64",
                self.base_url
            ))
            .send()
            .await?
            .json()
            .await?;
        let mut turns = response["turns"]
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unexpected turns response: {response}"))?;
        turns.sort_by_key(|turn| turn["depth"].as_u64().unwrap());
        Ok(turns)
    }

    async fn registry_type(&self, type_id: &str, version: u64) -> anyhow::Result<Value> {
        Client::new()
            .get(format!(
                "{}/v1/registry/types/{type_id}/versions/{version}",
                self.base_url
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }
}

struct MockOpenAi {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    _shutdown: oneshot::Sender<()>,
}

impl MockOpenAi {
    async fn start() -> anyhow::Result<Self> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_openai_handler))
            .route("/v1/responses", post(mock_openai_responses_handler))
            .with_state(requests.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}/v1"),
            requests,
            _shutdown: shutdown_tx,
        })
    }
}

struct MockOpenAiWebsocket {
    base_url: String,
    _shutdown: oneshot::Sender<()>,
}

impl MockOpenAiWebsocket {
    async fn start() -> anyhow::Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            tokio::select! {
                _ = async {
                    let (socket, _) = listener.accept().await.unwrap();
                    let callback = |_: &WsRequest, response: WsResponse| Ok(response);
                    let mut socket = tokio_tungstenite::accept_hdr_async(socket, callback)
                        .await
                        .unwrap();
                    let _request = socket.next().await.unwrap().unwrap();
                    socket
                        .send(WsMessage::Text(
                            r#"{"type":"response.output_text.delta","delta":"hello from websocket"}"#
                                .to_string(),
                        ))
                        .await
                        .unwrap();
                    socket
                        .send(WsMessage::Text(
                            r#"{
                                "type":"response.completed",
                                "response":{
                                    "model":"gpt-5.4",
                                    "status":"completed",
                                    "output":[
                                        {
                                            "type":"message",
                                            "role":"assistant",
                                            "content":[{"type":"output_text","text":"hello from websocket"}]
                                        }
                                    ]
                                }
                            }"#
                            .to_string(),
                        ))
                        .await
                        .unwrap();
                    let _ = socket.close(None).await;
                } => {}
                _ = &mut shutdown_rx => {}
            }
        });
        Ok(Self {
            base_url: format!("ws://{addr}/v1"),
            _shutdown: shutdown_tx,
        })
    }
}

struct MockOpenAiMalformed {
    base_url: String,
    _shutdown: oneshot::Sender<()>,
}

struct MockOpenAiTooling {
    base_url: String,
    _shutdown: oneshot::Sender<()>,
}

impl MockOpenAiTooling {
    async fn start() -> anyhow::Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new().route("/v1/chat/completions", post(mock_openai_tooling_handler));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}/v1"),
            _shutdown: shutdown_tx,
        })
    }
}

struct MockOpenAiUpstreamError {
    base_url: String,
    _shutdown: oneshot::Sender<()>,
}

impl MockOpenAiUpstreamError {
    async fn start() -> anyhow::Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(mock_openai_upstream_error_handler),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}/v1"),
            _shutdown: shutdown_tx,
        })
    }
}

impl MockOpenAiMalformed {
    async fn start() -> anyhow::Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new().route("/v1/chat/completions", post(mock_openai_malformed_handler));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}/v1"),
            _shutdown: shutdown_tx,
        })
    }
}

struct MockClaude {
    base_url: String,
    _shutdown: oneshot::Sender<()>,
}

impl MockClaude {
    async fn start() -> anyhow::Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new().route("/v1/messages", post(mock_claude_handler));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            _shutdown: shutdown_tx,
        })
    }
}

struct MockClaudeJson {
    base_url: String,
    _shutdown: oneshot::Sender<()>,
}

impl MockClaudeJson {
    async fn start() -> anyhow::Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new().route("/v1/messages", post(mock_claude_json_handler));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            _shutdown: shutdown_tx,
        })
    }
}

struct MockClaudeTooling {
    base_url: String,
    _shutdown: oneshot::Sender<()>,
}

impl MockClaudeTooling {
    async fn start() -> anyhow::Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new().route("/v1/messages", post(mock_claude_tooling_handler));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            _shutdown: shutdown_tx,
        })
    }
}

struct MockClaudeMalformed {
    base_url: String,
    _shutdown: oneshot::Sender<()>,
}

impl MockClaudeMalformed {
    async fn start() -> anyhow::Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new().route("/v1/messages", post(mock_claude_malformed_handler));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            _shutdown: shutdown_tx,
        })
    }
}

#[derive(Clone, Default)]
struct FakeRecoveringCxdbState {
    registry_ready: bool,
    create_calls: usize,
    append_calls: usize,
    stored_item_types: Vec<String>,
    stored_system_titles: Vec<String>,
}

struct FakeRecoveringCxdb {
    base_url: String,
    state: Arc<Mutex<FakeRecoveringCxdbState>>,
    _shutdown: oneshot::Sender<()>,
}

impl FakeRecoveringCxdb {
    async fn start() -> anyhow::Result<Self> {
        let state = Arc::new(Mutex::new(FakeRecoveringCxdbState::default()));
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new()
            .route(
                "/v1/registry/types/cxdb.ConversationItem/versions/3",
                get(fake_registry_type_handler),
            )
            .route(
                "/v1/registry/bundles/:bundle_id",
                put(fake_registry_bundle_handler),
            )
            .route("/v1/contexts/create", post(fake_create_context_handler))
            .route(
                "/v1/contexts/:context_id/append",
                post(fake_append_turn_handler),
            )
            .with_state(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            state,
            _shutdown: shutdown_tx,
        })
    }
}

async fn mock_openai_handler(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    request: axum::http::Request<Body>,
) -> impl IntoResponse {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json_body: Value = serde_json::from_slice(&body).unwrap();
    requests.lock().unwrap().push(json!({
        "path": parts.uri.path(),
        "body": json_body.clone(),
    }));

    let last_user = json_body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("hello");
    let response_text = if last_user == "tell me more" {
        "more detail"
    } else {
        "hi"
    };

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("x-request-id", "req_openai_123"),
        ],
        Body::from(
            json!({
                "id": "chatcmpl_123",
                "model": "gpt-5",
                "choices": [{"message": {"role": "assistant", "content": response_text}}]
            })
            .to_string(),
        ),
    )
}

async fn mock_openai_responses_handler(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    request: axum::http::Request<Body>,
) -> impl IntoResponse {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json_body: Value = serde_json::from_slice(&body).unwrap();
    requests.lock().unwrap().push(json!({
        "path": parts.uri.path(),
        "body": json_body.clone(),
    }));

    let response_text = match openai_input_last_user_text(&json_body) {
        Some("real prompt") => "clean answer",
        Some("websocket hello") => "hello from websocket",
        _ => "hi",
    };

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("x-request-id", "req_openai_responses_123"),
        ],
        Body::from(
            json!({
                "id": "resp_123",
                "model": "gpt-5.4",
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": response_text}]
                }]
            })
            .to_string(),
        ),
    )
}

async fn mock_openai_malformed_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("x-request-id", "req_openai_malformed_123"),
        ],
        Body::from("not-json"),
    )
}

async fn mock_openai_tooling_handler(request: axum::http::Request<Body>) -> impl IntoResponse {
    let (_, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json_body: Value = serde_json::from_slice(&body).unwrap();
    let messages = json_body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let has_tool_result = messages
        .iter()
        .any(|message| message["role"] == "tool" && message["content"] == "lookup done");

    let response = if has_tool_result {
        json!({
            "id": "chatcmpl_tool_2",
            "model": "gpt-5",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "tool complete"
                }
            }]
        })
    } else {
        json!({
            "id": "chatcmpl_tool_1",
            "model": "gpt-5",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{\"q\":\"use tool\"}"
                        }
                    }]
                }
            }]
        })
    };

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("x-request-id", "req_openai_tool_123"),
        ],
        Body::from(response.to_string()),
    )
}

async fn mock_openai_upstream_error_handler() -> impl IntoResponse {
    (
        StatusCode::BAD_GATEWAY,
        [
            ("content-type", "application/json"),
            ("x-request-id", "req_openai_error_123"),
        ],
        Body::from(json!({"error": "upstream failed"}).to_string()),
    )
}

async fn mock_claude_handler() -> impl IntoResponse {
    let body = Body::from_stream(async_stream::stream! {
        for chunk in [
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"model\":\"claude-3-7-sonnet-20250219\"}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"hello \"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"from claude\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ] {
            yield Ok::<_, std::io::Error>(bytes::Bytes::from(chunk));
        }
    });
    (
        StatusCode::OK,
        [
            ("content-type", "text/event-stream"),
            ("request-id", "req_claude_123"),
        ],
        body,
    )
}

async fn mock_claude_json_handler(request: axum::http::Request<Body>) -> impl IntoResponse {
    let (_, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json_body: Value = serde_json::from_slice(&body).unwrap();
    let last_user = anthropic_last_user_text(&json_body).unwrap_or("hello");
    let response_text = match last_user {
        "tell me more" => "more from claude",
        "new question" => "fresh answer",
        _ => "hello from claude",
    };

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("request-id", "req_claude_json_123"),
        ],
        Body::from(
            json!({
                "id": "msg_json_123",
                "model": "claude-3-7-sonnet-20250219",
                "content": [{"type": "text", "text": response_text}],
                "stop_reason": "end_turn"
            })
            .to_string(),
        ),
    )
}

async fn mock_claude_tooling_handler(request: axum::http::Request<Body>) -> impl IntoResponse {
    let (_, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json_body: Value = serde_json::from_slice(&body).unwrap();
    let has_tool_result = json_body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|block| block["type"] == "tool_result")
        });

    let chunks = if has_tool_result {
        vec![
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_tool_2\",\"model\":\"claude-3-7-sonnet-20250219\"}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"tool complete\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]
    } else {
        vec![
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_tool_1\",\"model\":\"claude-3-7-sonnet-20250219\"}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"lookup\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"q\\\":\\\"use tool\\\"}\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]
    };
    let body = Body::from_stream(async_stream::stream! {
        for chunk in chunks {
            yield Ok::<_, std::io::Error>(bytes::Bytes::from(chunk));
        }
    });

    (
        StatusCode::OK,
        [
            ("content-type", "text/event-stream"),
            ("request-id", "req_claude_tool_123"),
        ],
        body,
    )
}

async fn mock_claude_malformed_handler() -> impl IntoResponse {
    let body = Body::from_stream(async_stream::stream! {
        for chunk in [
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"model\":\"claude-3-7-sonnet-20250219\"}}\n\n",
            "event: content_block_start\ndata: {not-json}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ] {
            yield Ok::<_, std::io::Error>(bytes::Bytes::from(chunk));
        }
    });
    (
        StatusCode::OK,
        [
            ("content-type", "text/event-stream"),
            ("request-id", "req_claude_malformed_123"),
        ],
        body,
    )
}

async fn fake_registry_type_handler(
    State(state): State<Arc<Mutex<FakeRecoveringCxdbState>>>,
) -> impl IntoResponse {
    let state = state.lock().unwrap();
    if state.registry_ready {
        (
            StatusCode::OK,
            Body::from(json!({"type_id": "cxdb.ConversationItem", "version": 3}).to_string()),
        )
            .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn fake_registry_bundle_handler(
    State(state): State<Arc<Mutex<FakeRecoveringCxdbState>>>,
    AxumPath(_bundle_id): AxumPath<String>,
) -> impl IntoResponse {
    state.lock().unwrap().registry_ready = true;
    (StatusCode::OK, Body::from(json!({"ok": true}).to_string()))
}

async fn fake_create_context_handler(
    State(state): State<Arc<Mutex<FakeRecoveringCxdbState>>>,
) -> impl IntoResponse {
    state.lock().unwrap().create_calls += 1;
    (
        StatusCode::OK,
        Body::from(json!({"context_id": "41", "head_turn_id": "0", "head_depth": 0}).to_string()),
    )
}

async fn fake_append_turn_handler(
    State(state): State<Arc<Mutex<FakeRecoveringCxdbState>>>,
    AxumPath(_context_id): AxumPath<String>,
    request: axum::http::Request<Body>,
) -> impl IntoResponse {
    let (_, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json_body: Value = serde_json::from_slice(&body).unwrap();
    let mut state = state.lock().unwrap();
    state.append_calls += 1;
    if state.append_calls == 1 {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Body::from("temporary append failure"),
        )
            .into_response();
    }
    state.stored_item_types.push(
        json_body["data"]["item_type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    if let Some(title) = json_body["data"]["system"]["title"].as_str() {
        state.stored_system_titles.push(title.to_string());
    }
    (
        StatusCode::OK,
        Body::from(json!({"turn_id": state.append_calls.to_string()}).to_string()),
    )
        .into_response()
}

fn turn_item_types(turns: &[Value]) -> Vec<&str> {
    turns
        .iter()
        .map(|turn| {
            turn["data"]["item_type"]
                .as_str()
                .unwrap_or_else(|| panic!("missing item_type in typed turn: {turn}"))
        })
        .collect()
}

fn openai_input_last_user_text(payload: &Value) -> Option<&str> {
    payload["input"]
        .as_array()
        .and_then(|items| {
            items.iter().rev().find_map(|item| {
                (item["role"] == "user")
                    .then_some(item["content"].as_array())
                    .flatten()
                    .and_then(|content| content.last())
                    .and_then(|part| part["text"].as_str())
            })
        })
}

fn first_context_id(contexts: &Value) -> u64 {
    json_u64(&contexts["contexts"][0]["context_id"]).unwrap()
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn anthropic_last_user_text(payload: &Value) -> Option<&str> {
    payload["messages"]
        .as_array()?
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .and_then(|message| match &message["content"] {
            Value::String(value) => Some(value.as_str()),
            Value::Array(blocks) => blocks
                .iter()
                .rev()
                .find(|block| block["type"] == "text")
                .and_then(|block| block["text"].as_str()),
            _ => None,
        })
}

fn wait_for_http(url: &str) -> impl std::future::Future<Output = anyhow::Result<()>> + '_ {
    async move {
        for _ in 0..40 {
            match Client::new().get(url).send().await {
                Ok(response) if response.status().is_success() => return Ok(()),
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        anyhow::bail!("server at {url} did not become ready")
    }
}

fn write_executable(path: &Path, contents: &str) -> anyhow::Result<()> {
    fs::write(path, contents)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn prepend_path(dir: &Path) -> String {
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{existing}", dir.display())
}

fn find_single_ledger(root: &Path) -> Value {
    let sessions_dir = root.join(".scratch").join("cxtx").join("sessions");
    let entries = fs::read_dir(sessions_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    serde_json::from_slice(&fs::read(entries[0].join("ledger.json")).unwrap()).unwrap()
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}
