use anyhow::{Context, Result, bail};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::env::{SITE_ORIGIN, Target, ThreadSpec, seed};
use crate::server::{Socket, free_port};

const THREADS: [ThreadSpec; 2] = [
    ThreadSpec {
        name: "regional",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "regional_no_doubles",
        allow_double_posts: false,
    },
];
const ACCOUNTS: usize = 5;
const MODERATOR: usize = 3;
const WORKER_READY: Duration = Duration::from_secs(240);

pub struct Options {
    pub socket_bin: PathBuf,
    pub worker_dir: PathBuf,
    pub suite_dir: PathBuf,
    pub frontend: PathBuf,
    pub scenarios: Option<String>,
    pub out: PathBuf,
}

struct Worker {
    child: Child,
    dir: PathBuf,
    port: u16,
    args: Vec<String>,
    log: PathBuf,
}

impl Worker {
    async fn start(dir: &Path, port: u16, args: Vec<String>, log: PathBuf) -> Result<Worker> {
        let child = spawn_worker(dir, &args, &log)?;
        let mut w = Worker {
            child,
            dir: dir.to_owned(),
            port,
            args,
            log,
        };
        w.wait_ready().await?;
        Ok(w)
    }

    async fn restart(&mut self) -> Result<()> {
        stop_tree(&mut self.child).await;
        wait_port_free(self.port).await;
        self.child = spawn_worker(&self.dir, &self.args, &self.log)?;
        self.wait_ready().await
    }

    async fn wait_ready(&mut self) -> Result<()> {
        let http = reqwest::Client::new();
        let started = Instant::now();
        while started.elapsed() < WORKER_READY {
            if let Some(status) = self.child.try_wait()? {
                bail!("worker exited during startup ({status}); see {}", self.log.display());
            }
            let health = http
                .get(format!("http://127.0.0.1:{}/health", self.port))
                .timeout(Duration::from_secs(2))
                .send()
                .await;
            if health.is_ok_and(|r| r.status().is_success()) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        bail!("worker not ready after {WORKER_READY:?}; see {}", self.log.display())
    }
}

fn spawn_worker(dir: &Path, args: &[String], log: &Path) -> Result<Child> {
    let out = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    let wrangler = dir.join("node_modules/wrangler/bin/wrangler.js");
    if !wrangler.exists() {
        bail!("{} is missing; run npm ci in {}", wrangler.display(), dir.display());
    }
    Ok(Command::new("node")
        .arg(wrangler)
        .args(args)
        .current_dir(dir)
        .env("WRANGLER_SEND_METRICS", "false")
        .env("CI", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out.try_clone()?))
        .stderr(Stdio::from(out))
        .kill_on_drop(true)
        .spawn()?)
}

async fn stop_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        let pid = pid.to_string();
        let stopped = if cfg!(windows) {
            Command::new("taskkill")
                .args(["/PID", &pid, "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
        } else {
            Command::new("kill").args(["-TERM", &pid]).status().await
        };
        if stopped.is_ok()
            && tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .is_ok()
        {
            return;
        }
    }
    let _ = child.kill().await;
}

async fn wait_port_free(port: u16) {
    for _ in 0..60 {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

struct Lab {
    socket: Socket,
    worker: Worker,
}

type Shared = Arc<Mutex<Lab>>;

fn done(outcome: Result<()>) -> (StatusCode, String) {
    match outcome {
        Ok(()) => (StatusCode::OK, "ok".into()),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

#[derive(serde::Deserialize, Default)]
struct Restart {
    #[serde(default)]
    graceful: bool,
}

async fn socket_restart(State(lab): State<Shared>, body: Option<Json<Restart>>) -> (StatusCode, String) {
    let graceful = body.is_some_and(|Json(r)| r.graceful);
    let mut lab = lab.lock().await;
    if !graceful {
        return done(lab.socket.kill_and_restart().await);
    }
    let stopped = lab.socket.stop_gracefully().await;
    done(match stopped {
        Ok(()) => lab.socket.start_again().await,
        Err(e) => Err(e),
    })
}

async fn socket_stop(State(lab): State<Shared>) -> (StatusCode, String) {
    done(lab.lock().await.socket.stop().await)
}

async fn socket_start(State(lab): State<Shared>) -> (StatusCode, String) {
    done(lab.lock().await.socket.start_again().await)
}

async fn socket_env(
    State(lab): State<Shared>,
    Json(env): Json<HashMap<String, Option<String>>>,
) -> (StatusCode, String) {
    let mut lab = lab.lock().await;
    for (k, v) in env {
        lab.socket.set_env(&k, v);
    }
    done(lab.socket.kill_and_restart().await)
}

async fn worker_restart(State(lab): State<Shared>) -> (StatusCode, String) {
    done(lab.lock().await.worker.restart().await)
}

fn file_url(path: &Path) -> Result<String> {
    let full = std::fs::canonicalize(path).with_context(|| format!("{} is missing", path.display()))?;
    let text = full.to_string_lossy().replace('\\', "/");
    let text = text.trim_start_matches("//?/");
    Ok(if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    })
}

fn secret() -> String {
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

pub async fn run(o: Options) -> Result<()> {
    let client_lib = file_url(&o.frontend.join("lib/regional.ts"))?;
    let target = Target::from_env()?;
    target.reset().await?;
    let pool = target.pool().await?;
    let accounts = seed(&pool, &target, &THREADS, ACCOUNTS).await?;
    sqlx::query("UPDATE counters SET roles = ? WHERE uuid = ?")
        .bind(r#"["counter","mod","admin"]"#)
        .bind(&accounts[MODERATOR].uuid)
        .execute(&pool)
        .await?;

    if o.out.exists() {
        std::fs::remove_dir_all(&o.out)?;
    }
    std::fs::create_dir_all(&o.out)?;
    let shared = secret();
    let token = secret();
    let worker_port = free_port()?;
    let socket_port = free_port()?;
    let threads: Vec<&str> = THREADS.iter().map(|t| t.name).collect();
    let extra = [
        ("DO_THREADS", threads.join(",")),
        ("DO_WORKER_URL", format!("http://127.0.0.1:{worker_port}")),
        ("DO_SHARED_SECRET", shared.clone()),
        ("DO_HEALTH_SECS", "5".into()),
        ("DO_STALL_SECS", "4".into()),
        ("DO_MOVE_COOLDOWN_SECS", "2".into()),
        ("DO_MOVE_LULL_MS", "0".into()),
        ("DO_AUTO_COOLDOWN_SECS", "3".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let socket = Socket::start_with(
        &o.socket_bin,
        &target,
        &o.out.join("socket"),
        true,
        socket_port,
        true,
        extra,
    )
    .await
    .with_context(|| format!("starting {}", o.socket_bin.display()))?
    .under_ipc_launcher()
    .await
    .context("restarting the socket under the IPC launcher")?;
    let worker_args = vec![
        "dev".into(),
        "--ip".into(),
        "127.0.0.1".into(),
        "--port".into(),
        worker_port.to_string(),
        "--persist-to".into(),
        std::path::absolute(o.out.join("worker-state"))?
            .to_string_lossy()
            .into_owned(),
        "--show-interactive-dev-session=false".into(),
        "--var".into(),
        format!("INGEST_URL:http://127.0.0.1:{socket_port}/do/ingest"),
        "--var".into(),
        format!("DO_SHARED_SECRET:{shared}"),
        "--var".into(),
        format!("PROBE_SECRET:{token}"),
        "--var".into(),
        "ENABLED:on".into(),
    ];
    println!("starting the worker locally (builds the WebAssembly judge first)");
    let worker = Worker::start(
        &std::path::absolute(&o.worker_dir)?,
        worker_port,
        worker_args,
        std::path::absolute(o.out.join("worker.log"))?,
    )
    .await?;

    let nest_port = socket.nest_port;
    let lab: Shared = Arc::new(Mutex::new(Lab { socket, worker }));
    let control = Router::new()
        .route("/socket/restart", post(socket_restart))
        .route("/socket/stop", post(socket_stop))
        .route("/socket/start", post(socket_start))
        .route("/socket/env", post(socket_env))
        .route("/worker/restart", post(worker_restart))
        .with_state(lab.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let control_port = listener.local_addr()?.port();
    tokio::spawn(async move { axum::serve(listener, control).await });

    let m = &target.mysql;
    let config: Value = json!({
        "socketUrl": format!("http://127.0.0.1:{socket_port}"),
        "workerUrl": format!("http://127.0.0.1:{worker_port}"),
        "control": format!("http://127.0.0.1:{control_port}"),
        "origin": SITE_ORIGIN,
        "nestPort": nest_port,
        "sharedSecret": shared,
        "tokenSecret": token,
        "mysql": { "host": m.host, "port": m.port, "user": m.user, "password": m.password, "database": m.database },
        "threads": { "main": THREADS[0].name, "noDoubles": THREADS[1].name },
        "accounts": accounts.iter().map(|a| json!({ "uuid": a.uuid, "cookie": a.cookie })).collect::<Vec<_>>(),
        "moderator": MODERATOR,
        "clientLib": client_lib,
    });
    let config_path = std::path::absolute(o.out.join("regional.json"))?;
    std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;

    let mut suite = Command::new("node");
    suite
        .args(["--experimental-strip-types", "--no-warnings", "suite.mts"])
        .args(o.scenarios.iter())
        .current_dir(&o.suite_dir)
        .env("REGIONAL_CONFIG", &config_path)
        .kill_on_drop(true);
    let status = suite.status().await.context("running the regional suite");
    let _ = std::fs::remove_file(&config_path);
    let status = status?;

    let mut lab = lab.lock().await;
    stop_tree(&mut lab.worker.child).await;
    lab.socket.stop().await?;
    if !status.success() {
        bail!(
            "regional suite failed ({status}); logs in {} and {}",
            lab.socket.log_path().display(),
            lab.worker.log.display()
        );
    }
    Ok(())
}
