use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};

use crate::env::Target;

pub struct Socket {
    child: Child,
    pub port: u16,
    pub nest_port: u16,
    bin: PathBuf,
    workdir: PathBuf,
    target: Target,
    rate_limits: bool,
    extra_env: Vec<(String, String)>,
    launcher: Option<PathBuf>,
    control: Option<ChildStdin>,
}

const IPC_LAUNCHER: &str = r#"import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
const child = spawn(process.argv[2], [], { stdio: ['ignore', 'inherit', 'inherit', 'ipc'] })
const stop = () => (process.platform === 'win32' ? child.send('shutdown') : child.kill('SIGINT'))
createInterface({ input: process.stdin }).on('line', line => (line === 'shutdown' ? stop() : child.kill('SIGKILL')))
process.stdin.on('end', () => child.kill('SIGKILL'))
child.on('exit', code => process.exit(code ?? 1))
"#;

impl Socket {
    pub async fn start(bin: &Path, target: &Target, workdir: &Path, rate_limits: bool) -> Result<Socket> {
        Socket::start_on(bin, target, workdir, rate_limits, free_port()?, true).await
    }

    pub async fn start_on(
        bin: &Path,
        target: &Target,
        workdir: &Path,
        rate_limits: bool,
        port: u16,
        fresh: bool,
    ) -> Result<Socket> {
        Socket::start_with(bin, target, workdir, rate_limits, port, fresh, Vec::new()).await
    }

    pub async fn start_with(
        bin: &Path,
        target: &Target,
        workdir: &Path,
        rate_limits: bool,
        port: u16,
        fresh: bool,
        extra_env: Vec<(String, String)>,
    ) -> Result<Socket> {
        if fresh && workdir.exists() {
            std::fs::remove_dir_all(workdir).with_context(|| format!("clear {}", workdir.display()))?;
        }
        std::fs::create_dir_all(workdir)?;
        let bin = std::fs::canonicalize(bin).with_context(|| format!("socket binary {}", bin.display()))?;
        let nest_port = free_port()?;
        let child = spawn(&bin, None, target, workdir, port, rate_limits, nest_port, &extra_env)?;
        let mut s = Socket {
            child,
            port,
            nest_port,
            bin,
            workdir: workdir.to_owned(),
            target: target.clone(),
            rate_limits,
            extra_env,
            launcher: None,
            control: None,
        };
        s.wait_ready().await?;
        Ok(s)
    }

    pub async fn under_ipc_launcher(mut self) -> Result<Socket> {
        let launcher = std::fs::canonicalize(&self.workdir)?.join("ipc-launcher.mjs");
        std::fs::write(&launcher, IPC_LAUNCHER)?;
        self.launcher = Some(launcher);
        self.kill_and_restart().await?;
        Ok(self)
    }

    pub async fn kill_and_restart(&mut self) -> Result<()> {
        self.stop().await?;
        self.start_again().await
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.ask_launcher("kill", Duration::from_secs(10)).await?;
        if self.child.try_wait()?.is_none() {
            self.child.kill().await?;
        }
        Ok(())
    }

    pub async fn stop_gracefully(&mut self) -> Result<()> {
        if self.control.is_none() {
            bail!("a graceful stop needs the socket under the IPC launcher");
        }
        self.ask_launcher("shutdown", Duration::from_secs(30)).await?;
        self.stop().await
    }

    async fn ask_launcher(&mut self, line: &str, wait: Duration) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        let Some(control) = self.control.as_mut() else {
            return Ok(());
        };
        if control
            .write_all(
                format!(
                    "{line}
"
                )
                .as_bytes(),
            )
            .await
            .is_ok()
        {
            let _ = control.flush().await;
            let _ = tokio::time::timeout(wait, self.child.wait()).await;
        }
        Ok(())
    }

    pub async fn start_again(&mut self) -> Result<()> {
        self.child = spawn(
            &self.bin,
            self.launcher.as_deref(),
            &self.target,
            &self.workdir,
            self.port,
            self.rate_limits,
            self.nest_port,
            &self.extra_env,
        )?;
        self.control = self.child.stdin.take();
        self.wait_ready().await
    }

    pub fn set_env(&mut self, key: &str, value: Option<String>) {
        self.extra_env.retain(|(k, _)| k != key);
        if let Some(v) = value {
            self.extra_env.push((key.to_owned(), v));
        }
    }

    async fn wait_ready(&mut self) -> Result<()> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(60) {
            if let Some(status) = self.child.try_wait()? {
                bail!(
                    "socket exited during startup ({status}); see {}",
                    self.log_path().display()
                );
            }
            if crate::client::Client::connect(self.port, None).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        bail!("socket not ready after 60s; see {}", self.log_path().display())
    }

    pub fn log_path(&self) -> PathBuf {
        self.workdir.join("socket.log")
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    bin: &Path,
    launcher: Option<&Path>,
    target: &Target,
    workdir: &Path,
    port: u16,
    rate_limits: bool,
    nest_port: u16,
    extra_env: &[(String, String)],
) -> Result<Child> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(workdir.join("socket.log"))?;
    let m = &target.mysql;
    let mut cmd = match launcher {
        Some(launcher) => {
            let mut c = Command::new("node");
            c.arg(launcher).arg(bin).stdin(Stdio::piped());
            c
        }
        None => Command::new(bin),
    };
    cmd.current_dir(workdir)
        .env_clear()
        .envs(std::env::vars().filter(|(k, _)| is_os_variable(k)))
        .env("MYSQL_DB_HOST", &m.host)
        .env("MYSQL_DB_PORT", m.port.to_string())
        .env("MYSQL_DB_USERNAME", &m.user)
        .env("MYSQL_DB_PASSWORD", &m.password)
        .env("MYSQL_DB_DATABASE", &m.database)
        .env("REDIS_URL", &target.redis_url)
        .env("COOKIE_SECRET", &target.cookie_secret)
        .env("SOCKET_PORT", port.to_string())
        .env("WEBSOCKET_APP_PORT", nest_port.to_string())
        .env("BOT_PORT", free_port()?.to_string())
        .env("STATS_PORT", free_port()?.to_string())
        .env("WAL_DIR", workdir.join("wal"))
        .env("RATE_LIMITS", if rate_limits { "on" } else { "off" })
        .env("HOST_URL", crate::env::SITE_ORIGIN)
        .env("TRUST_PROXY_HOPS", "1")
        .env("RUST_LOG", "warn")
        .envs(extra_env.iter().map(|(k, v)| (k, v)))
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .kill_on_drop(true);
    Ok(cmd.spawn()?)
}

pub fn is_os_variable(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    [
        "PATH",
        "SYSTEMROOT",
        "WINDIR",
        "TEMP",
        "TMP",
        "HOME",
        "USERPROFILE",
        "LANG",
        "TZ",
    ]
    .contains(&k.as_str())
}

pub fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}
