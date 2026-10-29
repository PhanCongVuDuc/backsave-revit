mod common;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use revit_downgrade_server::clock::ManualClock;
use revit_downgrade_server::config::Config;
use revit_downgrade_server::jobs::JobId;
use revit_downgrade_server::worker::spawn_workers;
use revit_downgrade_server::{AppState, app};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    app: Router,
    dir: tempfile::TempDir,
}

fn setup_with(edit: impl FnOnce(&mut Config)) -> Fixture {
    setup_in(tempfile::tempdir().unwrap(), edit)
}

/// Builds the app on an existing data directory, so files can be placed there first.
fn setup_in(dir: tempfile::TempDir, edit: impl FnOnce(&mut Config)) -> Fixture {
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    ));
    let mut state = common::test_state(dir.path(), clock);
    let mut config = (*state.config).clone();
    edit(&mut config);
    state.config = Arc::new(config);
    let app = app(state.clone());
    Fixture { state, app, dir }
}

fn setup() -> Fixture {
    setup_with(|_| {})
}

fn start_workers(f: &Fixture) {
    spawn_workers(
        1,
        f.state.store.clone(),
        f.state.engine.clone(),
        f.state.stats.clone(),
    );
}

fn peer(last: u8) -> SocketAddr {
    SocketAddr::from(([10, 0, 0, last], 4000))
}

fn with_peer(mut req: Request<Body>, last: u8) -> Request<Body> {
    req.extensions_mut().insert(ConnectInfo(peer(last)));
    req
}

async fn send(f: &Fixture, req: Request<Body>) -> axum::response::Response {
    f.app.clone().oneshot(with_peer(req, 1)).await.unwrap()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), 1 << 26)
        .await
        .unwrap()
        .to_vec()
}

async fn body_json(resp: axum::response::Response) -> Value {
    serde_json::from_slice(&body_bytes(resp).await).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn post_job(body: &Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/jobs")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Content that makes the engine stub act as a file of `year` in the given mode.
fn marker_bytes(mode: &str, year: u32) -> Vec<u8> {
    let d = tempfile::tempdir().unwrap();
    let p = common::write_marker(d.path(), "rvt", mode, Some(year));
    std::fs::read(p).unwrap()
}

fn job_body(name: &str, size: usize, year: u16, target: u16) -> Value {
    json!({"name": name, "size": size, "kind": "rvt", "year": year, "target": target})
}

/// Creates a job and returns its id.
async fn create(f: &Fixture, name: &str, data: &[u8], year: u16, target: u16) -> String {
    let resp = send(f, post_job(&job_body(name, data.len(), year, target))).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v = body_json(resp).await;
    let id = v["job_id"].as_str().unwrap().to_string();
    assert_eq!(v["upload_url"], format!("/api/uploads/{id}"));
    id
}

async fn upload_all(f: &Fixture, id: &str, data: &[u8]) {
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/uploads/{id}"))
        .header("content-type", "application/offset+octet-stream")
        .header("tus-resumable", "1.0.0")
        .header("upload-offset", "0")
        .body(Body::from(data.to_vec()))
        .unwrap();
    let resp = send(f, req).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

async fn wait_for_state(f: &Fixture, id: &str, want: &str) -> Value {
    for _ in 0..200 {
        let v = body_json(send(f, get(&format!("/api/jobs/{id}"))).await).await;
        if v["state"] == want {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job never reached {want}");
}

/// Full run of one job through the stub engine; returns the id once it is done.
async fn run_to_done(f: &Fixture, name: &str, data: &[u8], year: u16, target: u16) -> String {
    let id = create(f, name, data, year, target).await;
    upload_all(f, &id, data).await;
    wait_for_state(f, &id, "done").await;
    id
}

#[tokio::test]
async fn info_returns_pairs_from_engine() {
    let mut f = setup();
    let pairs = f.state.engine.capabilities().await.unwrap();
    assert!(pairs.contains(&(2026, 2025)));
    f.state.pairs = Arc::new(pairs.clone());
    f.app = app(f.state.clone());
    let resp = send(&f, get("/api/info")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["max_bytes"], 1_073_741_824u64);
    assert_eq!(v["output_ttl_s"], 7200);
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    let got: Vec<(u16, u16)> = serde_json::from_value(v["pairs"].clone()).unwrap();
    assert_eq!(got, pairs);
}

#[tokio::test]
async fn create_upload_run_download_flow() {
    let f = setup();
    start_workers(&f);
    let data = marker_bytes("ok", 2026);
    let id = create(&f, "House.rvt", &data, 2026, 2025).await;

    let v = body_json(send(&f, get(&format!("/api/jobs/{id}"))).await).await;
    assert_eq!(v["state"], "waiting_upload");
    assert_eq!(v["position"], Value::Null);
    assert_eq!(v["result"], Value::Null);

    upload_all(&f, &id, &data).await;
    let v = wait_for_state(&f, &id, "done").await;
    assert_eq!(v["error"], Value::Null);
    assert_eq!(v["report_code"], Value::Null);
    assert_eq!(v["result"]["file_name"], "House_R2025.rvt");
    assert_eq!(v["result"]["dropped_count"], 72);
    assert!(v["expires_at"].as_str().unwrap().ends_with('Z'));

    let resp = send(&f, get(&format!("/api/jobs/{id}/file"))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "application/octet-stream");
    let len: usize = resp.headers()["content-length"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(resp.headers()["cache-control"], "no-store");
    assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    let bytes = body_bytes(resp).await;
    assert_eq!(bytes.len(), len);
    assert_eq!(bytes, data);
    assert_eq!(v["result"]["size"], len);

    let resp = send(&f, get(&format!("/api/jobs/{id}/dropped"))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let list = body_json(resp).await;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 72);
    for e in list {
        let mut keys: Vec<&str> = e.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["cascade_from", "class", "element_id", "reason"]);
    }
}

#[tokio::test]
async fn download_keeps_unicode_name() {
    let f = setup();
    start_workers(&f);
    let data = marker_bytes("ok", 2026);
    let id = run_to_done(&f, "Grüße \"naïve\" (v2).rvt", &data, 2026, 2025).await;
    let resp = send(&f, get(&format!("/api/jobs/{id}/file"))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()["content-disposition"],
        "attachment; filename=\"Gr__e _na_ve_ (v2)_R2025.rvt\"; filename*=UTF-8''Gr%C3%BC%C3%9Fe%20%22na%C3%AFve%22%20%28v2%29_R2025.rvt"
    );
}

#[tokio::test]
async fn delete_removes_files() {
    let f = setup();
    start_workers(&f);
    let data = marker_bytes("ok", 2026);
    let id = run_to_done(&f, "House.rvt", &data, 2026, 2025).await;
    let dir = f.dir.path().join("jobs").join(&id);
    assert!(dir.exists());
    let resp = send(&f, delete(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(!dir.exists());
    let resp = send(&f, get(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = send(&f, delete(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_waiting_job_removes_directory() {
    let f = setup();
    let data = marker_bytes("ok", 2026);
    let id = create(&f, "House.rvt", &data, 2026, 2025).await;
    let resp = send(&f, delete(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(!f.dir.path().join("jobs").join(&id).exists());
}

#[tokio::test]
async fn delete_running_job_cancels_engine() {
    let f = setup();
    start_workers(&f);
    let data = marker_bytes("hang", 2026);
    let id = create(&f, "House.rvt", &data, 2026, 2025).await;
    upload_all(&f, &id, &data).await;
    let job = f.state.store.get(&id.parse::<JobId>().unwrap()).unwrap();
    let pid_file = f.state.store.output_path(&job).with_extension("rvt.pid");
    let pid = common::wait_for_pid(&pid_file).await;
    assert!(common::process_alive(pid));

    let resp = send(&f, delete(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The worker notices the cancellation, stops the engine and deletes the directory.
    let dir = f.dir.path().join("jobs").join(&id);
    for _ in 0..100 {
        if !dir.exists() && !common::process_alive(pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!common::process_alive(pid));
    assert!(!dir.exists());
    let resp = send(&f, get(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_id_is_404() {
    let f = setup();
    for id in [
        "nothex",
        "ABCDEF0123456789ABCDEF0123456789",
        "0123456789abcdef",
        "..%2f..%2fetc",
    ] {
        for uri in [
            format!("/api/jobs/{id}"),
            format!("/api/jobs/{id}/file"),
            format!("/api/jobs/{id}/dropped"),
        ] {
            let resp = send(&f, get(&uri)).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(body_json(resp).await["error"]["code"], "not_found");
        }
        let resp = send(&f, delete(&format!("/api/jobs/{id}"))).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
    let unknown = JobId::random();
    let resp = send(&f, get(&format!("/api/jobs/{unknown}"))).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn file_before_done_is_409() {
    let f = setup();
    let data = marker_bytes("ok", 2026);
    let id = create(&f, "House.rvt", &data, 2026, 2025).await;
    for suffix in ["file", "dropped"] {
        let resp = send(&f, get(&format!("/api/jobs/{id}/{suffix}"))).await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(resp).await["error"]["code"], "not_ready");
    }
}

#[tokio::test]
async fn create_validates_name_and_size() {
    let f = setup();
    let cases = [
        (
            job_body("", 10, 2026, 2025),
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_name",
        ),
        (
            job_body(&format!("{}.rvt", "a".repeat(252)), 10, 2026, 2025),
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_name",
        ),
        (
            job_body("House.rvt", 0, 2026, 2025),
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_size",
        ),
        (
            job_body("House.rvt", 2_000_000_000, 2026, 2025),
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
        ),
        (
            job_body("House.rvt", 10, 2026, 2030),
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_pair",
        ),
    ];
    for (body, status, code) in cases {
        let resp = send(&f, post_job(&body)).await;
        assert_eq!(resp.status(), status, "{code}");
        assert_eq!(body_json(resp).await["error"]["code"], code);
    }
    // Exactly 255 characters is fine.
    let name = format!("{}.rvt", "é".repeat(251));
    let resp = send(&f, post_job(&job_body(&name, 10, 2026, 2025))).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    // Malformed JSON.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/jobs")
        .header("content-type", "application/json")
        .body(Body::from("{nope"))
        .unwrap();
    let resp = send(&f, req).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(resp).await["error"]["code"], "bad_request");
}

#[tokio::test]
async fn job_json_has_no_internal_fields() {
    let f = setup();
    start_workers(&f);
    let data = marker_bytes("ok", 2026);
    let id = run_to_done(&f, "House.rvt", &data, 2026, 2025).await;
    let resp = send(&f, get(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let v = body_json(resp).await;
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "elapsed_s",
            "error",
            "estimate_s",
            "expires_at",
            "kind",
            "name",
            "position",
            "report_code",
            "result",
            "size",
            "state",
            "target",
            "year"
        ]
    );
    assert_eq!(v["name"], "House.rvt");
    assert_eq!(v["kind"], "rvt");
    assert_eq!(v["year"], 2026);
    assert_eq!(v["target"], 2025);
    assert_eq!(v["size"], data.len());
    let mut rkeys: Vec<&str> = v["result"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    rkeys.sort();
    assert_eq!(rkeys, ["dropped_count", "file_name", "size"]);
    let text = v.to_string();
    assert!(!text.contains("10.0.0.1"));
    assert!(!text.contains("jobs"));
    assert!(!text.contains(&id));
}

#[tokio::test]
async fn failed_job_reports_error() {
    let f = setup();
    start_workers(&f);
    let data = marker_bytes("refuse", 2026);
    let id = create(&f, "House.rvt", &data, 2026, 2025).await;
    upload_all(&f, &id, &data).await;
    let v = wait_for_state(&f, &id, "failed").await;
    assert_eq!(v["error"]["kind"], "refused");
    assert!(v["error"]["message"].is_string());
    assert_eq!(v["result"], Value::Null);
}

#[tokio::test]
async fn queued_job_shows_position_and_estimate() {
    let f = setup();
    let size = 3 * 1024 * 1024;
    let resp = send(&f, post_job(&job_body("House.rvt", size, 2026, 2024))).await;
    let id = body_json(resp).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    // Pretend the upload finished; no worker runs, so the job stays queued.
    f.state.store.mark_uploaded(&id.parse().unwrap());
    let v = body_json(send(&f, get(&format!("/api/jobs/{id}"))).await).await;
    assert_eq!(v["state"], "queued");
    assert_eq!(v["position"], 0);
    assert_eq!(v["estimate_s"], 3 * 2);
    assert_eq!(v["elapsed_s"], 0);
}

#[tokio::test]
async fn forwarded_for_used_only_when_trusted() {
    for trusted in [false, true] {
        let f = setup_with(|c| c.trust_proxy = trusted);
        let make = |forwarded: &str| {
            let mut req = post_job(&job_body("House.rvt", 10, 2026, 2025));
            req.headers_mut()
                .insert("x-forwarded-for", forwarded.parse().unwrap());
            req
        };
        // Two waiting uploads per IP are allowed; the third from the same address is refused.
        for _ in 0..2 {
            let r = send(&f, make("203.0.113.1")).await;
            assert_eq!(r.status(), StatusCode::CREATED);
        }
        // A different forwarded address only counts as a different client when it is trusted.
        let r3 = send(&f, make("203.0.113.2")).await;
        if trusted {
            assert_eq!(r3.status(), StatusCode::CREATED);
        } else {
            assert_eq!(r3.status(), StatusCode::TOO_MANY_REQUESTS);
        }
    }
}

#[tokio::test]
async fn spa_fallback_serves_index() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<html>home</html>").unwrap();
    std::fs::write(web.path().join("app.js"), "console.log(1)").unwrap();
    let path = web.path().to_path_buf();
    let f = setup_with(move |c| c.web_dir = Some(path));
    for uri in ["/", "/convert/steps", "/index.html"] {
        let resp = send(&f, get(uri)).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        assert_eq!(body_bytes(resp).await, b"<html>home</html>");
    }
    let resp = send(&f, get("/app.js")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp).await, b"console.log(1)");
}

#[tokio::test]
async fn api_unknown_route_is_json_404() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<html></html>").unwrap();
    let path = web.path().to_path_buf();
    for f in [setup(), setup_with(move |c| c.web_dir = Some(path))] {
        for uri in ["/api/nope", "/api/jobs/x/y/z", "/api"] {
            let resp = send(&f, get(uri)).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(body_json(resp).await["error"]["code"], "not_found");
        }
    }
}

#[tokio::test]
async fn disk_full_upload_fails_job_and_closes_upload() {
    let f = setup();
    let data = marker_bytes("ok", 2026);
    let id = create(&f, "House.rvt", &data, 2026, 2025).await;
    // A directory at the input path makes storing the upload fail.
    let job = f.state.store.get(&id.parse().unwrap()).unwrap();
    std::fs::create_dir_all(f.state.store.input_path(&job)).unwrap();
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/uploads/{id}"))
        .header("content-type", "application/offset+octet-stream")
        .header("tus-resumable", "1.0.0")
        .header("upload-offset", "0")
        .body(Body::from(data.clone()))
        .unwrap();
    let resp = send(&f, req).await;
    assert_eq!(resp.status(), StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(body_json(resp).await["error"]["code"], "storage_error");
    let head = Request::builder()
        .method(Method::HEAD)
        .uri(format!("/api/uploads/{id}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&f, head).await.status(), StatusCode::CONFLICT);
    let v = body_json(send(&f, get(&format!("/api/jobs/{id}"))).await).await;
    assert_eq!(v["state"], "failed");
    assert_eq!(v["error"]["kind"], "disk_full");
}

#[tokio::test]
async fn full_disk_budget_is_503_server_full() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    ));
    let mut state = common::test_state(dir.path(), clock.clone());
    state.store = Arc::new(
        revit_downgrade_server::jobs::JobStore::new(dir.path().to_path_buf(), clock)
            .with_disk_budget(100),
    );
    let f = Fixture {
        app: app(state.clone()),
        state,
        dir,
    };
    let r = send(&f, post_job(&job_body("A.rvt", 50, 2026, 2025))).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let r = with_peer(post_job(&job_body("B.rvt", 1, 2026, 2025)), 2);
    let r = f.app.clone().oneshot(r).await.unwrap();
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.headers()["retry-after"], "600");
    assert_eq!(body_json(r).await["error"]["code"], "server_full");
}

#[tokio::test]
async fn forwarded_for_honored_only_from_trusted_proxy_list() {
    for (list, honored) in [("10.0.0.1", true), ("10.9.0.0/16", false)] {
        let f = setup_with(|c| {
            c.trust_proxy = true;
            c.trusted_proxies = vec![list.parse().unwrap()];
        });
        let make = |forwarded: &str| {
            let mut req = post_job(&job_body("House.rvt", 10, 2026, 2025));
            req.headers_mut()
                .insert("x-forwarded-for", forwarded.parse().unwrap());
            req
        };
        // The test peer is 10.0.0.1.
        for _ in 0..2 {
            assert_eq!(
                send(&f, make("203.0.113.1")).await.status(),
                StatusCode::CREATED
            );
        }
        let r3 = send(&f, make("203.0.113.2")).await;
        let want = if honored {
            StatusCode::CREATED
        } else {
            StatusCode::TOO_MANY_REQUESTS
        };
        assert_eq!(r3.status(), want, "{list}");
    }
}

#[tokio::test]
async fn responses_carry_security_headers() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<html>home</html>").unwrap();
    let path = web.path().to_path_buf();
    let f = setup_with(move |c| c.web_dir = Some(path));
    for uri in ["/", "/job/abc", "/api/info"] {
        let resp = send(&f, get(uri)).await;
        let h = resp.headers();
        assert_eq!(
            h["content-security-policy"],
            "default-src 'self'; script-src 'self' https://static.cloudflareinsights.com; connect-src 'self' https://cloudflareinsights.com; img-src 'self' data:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
            "{uri}"
        );
        assert_eq!(h["x-frame-options"], "DENY");
        assert_eq!(h["referrer-policy"], "no-referrer");
    }
}

fn post_report(id: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/jobs/{id}/report"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"consent":true}"#))
        .unwrap()
}

/// Sends a request as the given client behind the (trusted) proxy.
async fn send_from(f: &Fixture, client: &str, mut req: Request<Body>) -> axum::response::Response {
    req.headers_mut()
        .insert("x-forwarded-for", client.parse().unwrap());
    send(f, req).await
}

/// Runs one job to its final state, as the given client.
async fn run_from(f: &Fixture, client: &str, mode: &str, want: &str) -> String {
    let data = marker_bytes(mode, 2026);
    let resp = send_from(
        f,
        client,
        post_job(&job_body("House.rvt", data.len(), 2026, 2025)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let id = body_json(resp).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    upload_all(f, &id, &data).await;
    wait_for_state(f, &id, want).await;
    id
}

async fn stats(f: &Fixture) -> Value {
    let resp = send(f, get("/api/stats")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp).await
}

/// Waits (up to 5 s) until `/api/stats` shows `field == want`: the count is written just after the
/// job reaches its final state.
async fn wait_for_stat(f: &Fixture, field: &str, want: u64) -> Value {
    for _ in 0..100 {
        let v = stats(f).await;
        if v[field] == want {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{field} never reached {want}: {}", stats(f).await);
}

#[tokio::test]
async fn stats_new_server_is_zero_and_cacheable() {
    let f = setup();
    let resp = send(&f, get("/api/stats")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["cache-control"], "public, max-age=60");
    let since = f.state.clock.now().format("%Y-%m-%d").to_string();
    assert_eq!(
        body_json(resp).await,
        json!({"since": since, "people": 0, "succeeded": 0, "failed": 0, "contributed": 0})
    );
}

#[tokio::test]
async fn stats_count_outcomes_and_reports() {
    let f = setup();
    start_workers(&f);
    run_from(&f, "203.0.113.1", "ok", "done").await;
    let done = run_from(&f, "203.0.113.1", "ok", "done").await;
    let failed = run_from(&f, "203.0.113.1", "fail", "failed").await;
    wait_for_stat(&f, "succeeded", 2).await;
    let v = wait_for_stat(&f, "failed", 1).await;
    assert_eq!(v["contributed"], 0);
    assert_eq!(v["people"], 1);

    let resp = send(&f, post_report(&failed)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(stats(&f).await["contributed"], 1);

    // Refused reports are not contributions.
    let resp = send(&f, post_report(&failed)).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let resp = send(&f, post_report(&done)).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let v = stats(&f).await;
    assert_eq!(
        (&v["succeeded"], &v["failed"], &v["contributed"]),
        (&json!(2), &json!(1), &json!(1))
    );
}

#[tokio::test]
async fn stats_skip_cancelled_job() {
    let f = setup();
    start_workers(&f);
    let data = marker_bytes("hang", 2026);
    let id = create(&f, "House.rvt", &data, 2026, 2025).await;
    upload_all(&f, &id, &data).await;
    wait_for_state(&f, &id, "running").await;
    let resp = send(&f, delete(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let dir = f.dir.path().join("jobs").join(&id);
    for _ in 0..100 {
        if !dir.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!dir.exists());
    let v = stats(&f).await;
    assert_eq!((&v["succeeded"], &v["failed"]), (&json!(0), &json!(0)));
    assert_eq!(v["people"], 0);
}

#[tokio::test]
async fn stats_people_by_forwarded_address() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    // IPv4 A converts 2 files, IPv4 B 1 file (it fails and is reported), and two addresses of
    // one IPv6 /64 network convert one file each.
    run_from(&f, "203.0.113.1", "ok", "done").await;
    run_from(&f, "203.0.113.1", "ok", "done").await;
    let failed = run_from(&f, "203.0.113.2", "fail", "failed").await;
    run_from(&f, "2001:db8:1:2::1", "ok", "done").await;
    run_from(&f, "2001:db8:1:2::2", "ok", "done").await;
    wait_for_stat(&f, "failed", 1).await;
    let resp = send_from(&f, "203.0.113.2", post_report(&failed)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v = wait_for_stat(&f, "succeeded", 4).await;
    assert_eq!(
        (&v["people"], &v["failed"], &v["contributed"]),
        (&json!(3), &json!(1), &json!(1))
    );
    let text = std::fs::read_to_string(f.dir.path().join("stats").join("stats.json")).unwrap();
    assert!(!text.contains("203.0.113"));
    assert!(!text.contains("2001:db8"));
}

#[tokio::test]
async fn stats_corrupt_file_is_503_and_downgrades_still_work() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("stats")).unwrap();
    let file = dir.path().join("stats").join("stats.json");
    std::fs::write(&file, "{").unwrap();
    let f = setup_in(dir, |_| {});
    start_workers(&f);

    let resp = send(&f, get("/api/stats")).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "stats_unavailable");
    assert_eq!(v["error"]["message"], "Usage statistics are not available.");

    let data = marker_bytes("ok", 2026);
    run_to_done(&f, "House.rvt", &data, 2026, 2025).await;
    let resp = send(&f, get("/api/stats")).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(std::fs::read(&file).unwrap(), b"{");
}

#[tokio::test]
async fn stats_survive_restarts() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    run_from(&f, "203.0.113.1", "ok", "done").await;
    let failed = run_from(&f, "203.0.113.2", "fail", "failed").await;
    wait_for_stat(&f, "failed", 1).await;
    let resp = send(&f, post_report(&failed)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let before = wait_for_stat(&f, "succeeded", 1).await;
    assert_eq!(before["contributed"], 1);
    assert_eq!(before["people"], 2);

    // Restart three times on the same data directory, the way the server starts.
    let mut f = f;
    for _ in 0..3 {
        let Fixture { dir, .. } = f;
        revit_downgrade_server::cleanup::wipe_jobs_dir(dir.path()).unwrap();
        f = setup_in(dir, |c| c.trust_proxy = true);
        assert_eq!(stats(&f).await, before);
    }

    // A client counted before the restarts: only the succeeded total moves, so the key held too.
    start_workers(&f);
    run_from(&f, "203.0.113.1", "ok", "done").await;
    let after = wait_for_stat(&f, "succeeded", 2).await;
    assert_eq!(after["failed"], before["failed"]);
    assert_eq!(after["contributed"], before["contributed"]);
    assert_eq!(after["people"], before["people"]);
    assert_eq!(after["since"], before["since"]);
}

// "Does not open in Revit" reports.

fn post_open_report(id: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/jobs/{id}/open-report"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn open_body(size: usize, note: Option<&str>) -> String {
    match note {
        Some(n) => json!({ "consent": true, "size": size, "note": n }),
        None => json!({ "consent": true, "size": size }),
    }
    .to_string()
}

fn patch_open_report(id: &str, offset: usize, bytes: &[u8]) -> Request<Body> {
    Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/jobs/{id}/open-report/upload"))
        .header("content-type", "application/offset+octet-stream")
        .header("tus-resumable", "1.0.0")
        .header("upload-offset", offset.to_string())
        .body(Body::from(bytes.to_vec()))
        .unwrap()
}

fn head_open_report(id: &str) -> Request<Body> {
    Request::builder()
        .method(Method::HEAD)
        .uri(format!("/api/jobs/{id}/open-report/upload"))
        .header("tus-resumable", "1.0.0")
        .body(Body::empty())
        .unwrap()
}

fn finish_open_report(id: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/jobs/{id}/open-report/finish"))
        .body(Body::empty())
        .unwrap()
}

/// A done job of `client` and the bytes of its original.
async fn done_from(f: &Fixture, client: &str) -> (String, Vec<u8>) {
    let id = run_from(f, client, "ok", "done").await;
    (id, marker_bytes("ok", 2026))
}

/// Opens the report, sends the whole original and finishes; returns the report code.
async fn report_unopenable(f: &Fixture, client: &str, id: &str, data: &[u8]) -> String {
    let resp = send_from(
        f,
        client,
        post_open_report(id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send_from(f, client, patch_open_report(id, 0, data)).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = send_from(f, client, finish_open_report(id)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    body_json(resp).await["report_code"]
        .as_str()
        .unwrap()
        .to_string()
}

fn job_of(f: &Fixture, id: &str) -> revit_downgrade_server::jobs::Job {
    f.state.store.get(&id.parse().unwrap()).unwrap()
}

fn report_codes(f: &Fixture) -> Vec<String> {
    match std::fs::read_dir(f.dir.path().join("reports")) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

async fn error_code(resp: axum::response::Response) -> (StatusCode, Value) {
    let status = resp.status();
    (status, body_json(resp).await["error"]["code"].clone())
}

#[tokio::test]
async fn open_report_full_flow_makes_a_report_and_moves_the_counts() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.10";
    let (id, data) = done_from(&f, client).await;
    let before = wait_for_stat(&f, "succeeded", 1).await;
    assert_eq!(
        (&before["failed"], &before["contributed"]),
        (&json!(0), &json!(0))
    );

    let note = "Revit says the file is corrupt";
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), Some(note))),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(resp.headers()["cache-control"], "no-store");
    assert_eq!(
        body_json(resp).await["upload_url"],
        format!("/api/jobs/{id}/open-report/upload")
    );

    let half = data.len() / 2;
    let resp = send_from(&f, client, patch_open_report(&id, 0, &data[..half])).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(resp.headers()["upload-offset"], half.to_string().as_str());
    let resp = send_from(&f, client, head_open_report(&id)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["upload-offset"], half.to_string().as_str());
    assert_eq!(
        resp.headers()["upload-length"],
        data.len().to_string().as_str()
    );
    assert_eq!(resp.headers()["tus-resumable"], "1.0.0");
    let resp = send_from(&f, client, patch_open_report(&id, half, &data[half..])).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let code = body_json(resp).await["report_code"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(code.starts_with("R-") && code.len() == 8, "{code}");

    let dir = f.dir.path().join("reports").join(&code);
    assert_eq!(std::fs::read(dir.join("input.rvt")).unwrap(), data);
    assert_eq!(std::fs::read(dir.join("output.rvt")).unwrap(), data);
    let meta: Value =
        serde_json::from_slice(&std::fs::read(dir.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["report_type"], "does_not_open");
    assert_eq!(meta["kind"], "rvt");
    assert_eq!(meta["size"], data.len());
    assert_eq!(meta["year"], 2026);
    assert_eq!(meta["target"], 2025);
    assert_eq!(meta["note"], note);
    assert!(meta["done_at"].is_string() && meta["reported_at"].is_string());
    let text = std::fs::read_to_string(dir.join("meta.json")).unwrap();
    assert!(!text.contains("House") && !text.contains(client));

    let v = body_json(send(&f, get(&format!("/api/jobs/{id}"))).await).await;
    assert_eq!(v["state"], "done");
    assert_eq!(v["report_code"], code.as_str());
    let resp = send(&f, get(&format!("/api/jobs/{id}/file"))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp).await, data);
    assert!(!f.state.store.report_input_path(&job_of(&f, &id)).exists());

    let after = stats(&f).await;
    assert_eq!(
        (
            &after["succeeded"],
            &after["failed"],
            &after["contributed"],
            &after["people"]
        ),
        (&json!(0), &json!(1), &json!(1), &before["people"])
    );
}

#[tokio::test]
async fn open_report_start_refusals() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.11";
    let (id, data) = done_from(&f, client).await;
    let size = data.len();

    for body in [
        json!({ "size": size }).to_string(),
        json!({ "consent": false, "size": size }).to_string(),
        json!({ "consent": "true", "size": size }).to_string(),
        "not json".to_string(),
    ] {
        let (status, code) =
            error_code(send_from(&f, client, post_open_report(&id, &body)).await).await;
        assert_eq!(
            (status, code),
            (StatusCode::UNPROCESSABLE_ENTITY, json!("consent_required")),
            "{body}"
        );
    }
    let long = "é".repeat(501);
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(size, Some(&long))),
    )
    .await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::UNPROCESSABLE_ENTITY, json!("note_too_long"))
    );
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(size + 1, None)),
    )
    .await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::CONFLICT, json!("size_mismatch"))
    );
    assert!(
        job_of(&f, &id).open_report.is_none(),
        "nothing opened, no byte taken"
    );

    let failed = run_from(&f, client, "fail", "failed").await;
    let resp = send_from(
        &f,
        client,
        post_open_report(&failed, &open_body(size, None)),
    )
    .await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::CONFLICT, json!("not_done"))
    );
    for unknown in ["zz".to_string(), "0".repeat(32)] {
        let resp = send_from(
            &f,
            client,
            post_open_report(&unknown, &open_body(size, None)),
        )
        .await;
        assert_eq!(
            error_code(resp).await,
            (StatusCode::NOT_FOUND, json!("not_found"))
        );
    }

    // 500 characters is the most a note may have; blank notes are dropped.
    let longest = format!("  {}  ", "é".repeat(500));
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(size, Some(&longest))),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        job_of(&f, &id).open_report.unwrap().note,
        Some("é".repeat(500))
    );
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(size, Some("   "))),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(job_of(&f, &id).open_report.unwrap().note, None);
    assert!(report_codes(&f).is_empty());
}

#[tokio::test]
async fn open_report_start_again_resumes_the_upload() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.12";
    let (id, data) = done_from(&f, client).await;
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send_from(&f, client, patch_open_report(&id, 0, &data[..5])).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), Some("x"))),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        body_json(resp).await["upload_url"],
        format!("/api/jobs/{id}/open-report/upload")
    );
    let resp = send_from(&f, client, head_open_report(&id)).await;
    assert_eq!(resp.headers()["upload-offset"], "5");
}

#[tokio::test]
async fn open_report_counts_toward_uploads_in_progress() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.13";
    let (id, data) = done_from(&f, client).await;
    for _ in 0..2 {
        let resp = send_from(&f, client, post_job(&job_body("A.rvt", 10, 2026, 2025))).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::TOO_MANY_REQUESTS, json!("uploads_in_progress"))
    );
}

#[tokio::test]
async fn open_report_needs_disk_budget_for_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    ));
    let data = marker_bytes("ok", 2026);
    let mut state = common::test_state(dir.path(), clock.clone());
    let mut config = (*state.config).clone();
    config.trust_proxy = true;
    state.config = Arc::new(config);
    // Room for one job while it runs (input + output), and later for its output plus the
    // original sent again, but not for anything more.
    state.store = Arc::new(
        revit_downgrade_server::jobs::JobStore::new(dir.path().to_path_buf(), clock)
            .with_disk_budget(2 * data.len() as u64),
    );
    let f = Fixture {
        app: app(state.clone()),
        state,
        dir,
    };
    start_workers(&f);
    let (id, _) = done_from(&f, "203.0.113.14").await;
    // Another client's waiting upload takes 2 bytes.
    let resp = send_from(
        &f,
        "203.0.113.15",
        post_job(&job_body("B.rvt", 1, 2026, 2025)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send_from(
        &f,
        "203.0.113.14",
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.headers()["retry-after"], "600");
    assert_eq!(
        error_code(resp).await,
        (StatusCode::SERVICE_UNAVAILABLE, json!("server_full"))
    );
}

#[tokio::test]
async fn open_report_finish_refusals() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.16";
    let (id, data) = done_from(&f, client).await;
    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::NOT_FOUND, json!("not_found"))
    );

    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send_from(&f, client, patch_open_report(&id, 0, &data[..3])).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::CONFLICT, json!("upload_incomplete"))
    );
    let resp = send_from(&f, client, patch_open_report(&id, 3, &data[3..])).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // A file on disk of another size than counted (should never happen) is refused, and the
    // upload is closed so that the original can be sent again from the start.
    let path = f.state.store.report_input_path(&job_of(&f, &id));
    std::fs::write(&path, [&data[..], b"extra"].concat()).unwrap();
    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::CONFLICT, json!("size_mismatch"))
    );
    assert!(job_of(&f, &id).open_report.is_none());
    assert!(!path.exists());
    assert_eq!(stats(&f).await["contributed"], 0);
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send_from(&f, client, patch_open_report(&id, 0, &data)).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Saving fails (the reports directory is a file): the upload stays open for a retry.
    let reports = f.dir.path().join("reports");
    let _ = std::fs::remove_dir_all(&reports);
    std::fs::write(&reports, b"not a directory").unwrap();
    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::INSUFFICIENT_STORAGE, json!("storage_error"))
    );
    assert!(job_of(&f, &id).open_report.is_some());
    assert!(path.is_file());
    assert_eq!(stats(&f).await["contributed"], 0);
    std::fs::remove_file(&reports).unwrap();

    let (a, b) = tokio::join!(
        send_from(&f, client, finish_open_report(&id)),
        send_from(&f, client, finish_open_report(&id)),
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    assert_eq!(report_codes(&f).len(), 1);
    let v = stats(&f).await;
    assert_eq!(
        (&v["succeeded"], &v["failed"], &v["contributed"]),
        (&json!(0), &json!(1), &json!(1))
    );

    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::CONFLICT, json!("already_reported"))
    );
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::CONFLICT, json!("already_reported"))
    );
    let resp = send_from(&f, client, patch_open_report(&id, data.len(), b"x")).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::CONFLICT, json!("upload_closed"))
    );
}

#[tokio::test]
async fn open_report_input_lost_at_finish_reopens_the_upload() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.17";
    let (id, data) = done_from(&f, client).await;
    let open_and_send = || async {
        let resp = send_from(
            &f,
            client,
            post_open_report(&id, &open_body(data.len(), None)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED, "a fresh upload");
        let resp = send_from(&f, client, patch_open_report(&id, 0, &data)).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    };
    open_and_send().await;
    // The job is still done: losing the file is a storage error, never "expired".
    std::fs::remove_file(f.state.store.report_input_path(&job_of(&f, &id))).unwrap();
    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::INSUFFICIENT_STORAGE, json!("storage_error"))
    );
    assert!(job_of(&f, &id).open_report.is_none());
    assert!(report_codes(&f).is_empty());
    assert_eq!(stats(&f).await["contributed"], 0);

    // The retry starts from offset 0 and makes the report.
    open_and_send().await;
    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(report_codes(&f).len(), 1);
}

#[tokio::test]
async fn open_report_finish_on_an_expired_job_is_410() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.21";
    let (id, data) = done_from(&f, client).await;
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send_from(&f, client, patch_open_report(&id, 0, &data)).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let job_id = job_of(&f, &id).id;
    f.state.store.expire(&job_id, f.state.clock.now());
    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(error_code(resp).await, (StatusCode::GONE, json!("expired")));
    assert!(report_codes(&f).is_empty());
    assert_eq!(stats(&f).await["contributed"], 0);
}

#[tokio::test]
async fn open_report_allowed_while_the_client_has_a_conversion_running() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.22";
    let (id, data) = done_from(&f, client).await;
    // A second conversion of the same client is running: it blocks a new job, not the report.
    let running = run_from(&f, client, "hang", "running").await;
    let resp = send_from(&f, client, post_job(&job_body("B.rvt", 10, 2026, 2025))).await;
    assert_eq!(error_code(resp).await.1, json!("active_job"));
    let code = report_unopenable(&f, client, &id, &data).await;
    assert!(report_codes(&f).contains(&code));
    let resp = send_from(&f, client, delete(&format!("/api/jobs/{running}"))).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn open_report_does_not_count_toward_the_hourly_limit() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.18";
    let (id, data) = done_from(&f, client).await;
    report_unopenable(&f, client, &id, &data).await;
    // One job so far this hour: four more are allowed.
    for _ in 0..4 {
        let resp = send_from(&f, client, post_job(&job_body("A.rvt", 10, 2026, 2025))).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let new = body_json(resp).await["job_id"]
            .as_str()
            .unwrap()
            .to_string();
        let resp = send_from(&f, client, delete(&format!("/api/jobs/{new}"))).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
    let resp = send_from(&f, client, post_job(&job_body("A.rvt", 10, 2026, 2025))).await;
    assert_eq!(error_code(resp).await.1, json!("hourly_limit"));
}

#[tokio::test]
async fn open_report_saved_while_statistics_are_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("stats")).unwrap();
    let file = dir.path().join("stats").join("stats.json");
    std::fs::write(&file, "{").unwrap();
    let f = setup_in(dir, |c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.19";
    let (id, data) = done_from(&f, client).await;
    let code = report_unopenable(&f, client, &id, &data).await;
    assert!(
        f.dir
            .path()
            .join("reports")
            .join(&code)
            .join("input.rvt")
            .is_file()
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"{");
}

#[tokio::test]
async fn open_report_upload_closed_while_a_report_is_being_written() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.20";
    let (id, data) = done_from(&f, client).await;
    let resp = send_from(&f, client, patch_open_report(&id, 0, &data)).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::NOT_FOUND, json!("not_found"))
    );
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    // A finish in progress holds the report claim.
    let claimed = f.state.store.claim_report(
        &id.parse().unwrap(),
        revit_downgrade_server::jobs::ReportKind::DoesNotOpen,
    );
    assert!(matches!(
        claimed,
        revit_downgrade_server::jobs::ReportClaim::Claimed(_)
    ));
    let resp = send_from(&f, client, patch_open_report(&id, 0, &data)).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::CONFLICT, json!("upload_closed"))
    );
}

#[tokio::test]
async fn open_report_storage_error_keeps_the_job_done() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.21";
    let (id, data) = done_from(&f, client).await;
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send_from(&f, client, patch_open_report(&id, 0, &data[..4])).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let path = f.state.store.report_input_path(&job_of(&f, &id));
    assert!(path.is_file());
    // A directory in place of the partial file makes the next write fail.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    let resp = send_from(&f, client, patch_open_report(&id, 4, &data[4..])).await;
    assert_eq!(
        error_code(resp).await,
        (StatusCode::INSUFFICIENT_STORAGE, json!("storage_error"))
    );
    let job = job_of(&f, &id);
    assert_eq!(job.state, revit_downgrade_server::model::JobState::Done);
    assert!(job.open_report.is_none());
    assert!(job.error.is_none());
    let resp = send(&f, get(&format!("/api/jobs/{id}/file"))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    // The user starts again.
    std::fs::remove_dir_all(&path).unwrap();
    let code = report_unopenable(&f, client, &id, &data).await;
    assert!(code.starts_with("R-"));
}

#[tokio::test]
async fn open_report_counts_survive_a_restart() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.23";
    let (id, data) = done_from(&f, client).await;
    run_from(&f, client, "fail", "failed").await;
    wait_for_stat(&f, "failed", 1).await;
    let before = wait_for_stat(&f, "succeeded", 1).await;
    report_unopenable(&f, client, &id, &data).await;
    let after = stats(&f).await;
    assert_eq!(
        (
            &after["succeeded"],
            &after["failed"],
            &after["contributed"],
            &after["people"]
        ),
        (&json!(0), &json!(2), &json!(1), &before["people"])
    );

    let Fixture { dir, .. } = f;
    revit_downgrade_server::cleanup::wipe_jobs_dir(dir.path()).unwrap();
    let f = setup_in(dir, |c| c.trust_proxy = true);
    assert_eq!(stats(&f).await, after);
}

#[tokio::test]
async fn refused_open_reports_leave_the_counts_alone() {
    let f = setup_with(|c| c.trust_proxy = true);
    start_workers(&f);
    let client = "203.0.113.24";
    let (id, data) = done_from(&f, client).await;
    let (expiring, _) = done_from(&f, client).await;
    let before = wait_for_stat(&f, "succeeded", 2).await;

    // Not the original: refused before any byte is taken.
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len() + 7, None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(stats(&f).await, before);

    // Already reported.
    report_unopenable(&f, client, &id, &data).await;
    let reported = stats(&f).await;
    let resp = send_from(&f, client, finish_open_report(&id)).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let resp = send_from(
        &f,
        client,
        post_open_report(&id, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(stats(&f).await, reported);

    // Expired, with its upload already sent.
    let resp = send_from(
        &f,
        client,
        post_open_report(&expiring, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send_from(&f, client, patch_open_report(&expiring, 0, &data)).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        f.state
            .store
            .expire(&expiring.parse().unwrap(), f.state.clock.now())
    );
    let resp = send_from(&f, client, finish_open_report(&expiring)).await;
    assert_eq!(resp.status(), StatusCode::GONE);
    let resp = send_from(
        &f,
        client,
        post_open_report(&expiring, &open_body(data.len(), None)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::GONE);
    assert_eq!(stats(&f).await, reported);
    assert_eq!(report_codes(&f).len(), 1);
}
