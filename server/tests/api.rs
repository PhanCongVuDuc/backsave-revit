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
    let dir = tempfile::tempdir().unwrap();
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
    spawn_workers(1, f.state.store.clone(), f.state.engine.clone());
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
    assert_eq!(v["output_ttl_s"], 3600);
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
        revit_downgrade_server::jobs::JobStore::new(dir.path().to_path_buf(), clock).with_disk_budget(100),
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
            "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
            "{uri}"
        );
        assert_eq!(h["x-frame-options"], "DENY");
        assert_eq!(h["referrer-policy"], "no-referrer");
    }
}
