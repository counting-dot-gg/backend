use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

use crate::env::Target;

pub struct Socket {
    child: Child,
    pub port: u16,
    bin: PathBuf,
    workdir: PathBuf,
    target: Target,
    rate_limits: bool,
}

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
        if fresh && workdir.exists() {
            std::fs::remove_dir_all(workdir).with_context(|| format!("clear {}", workdir.display()))?;
        }
        std::fs::create_dir_all(workdir)?;
        let bin = std::fs::canonicalize(bin).with_context(|| format!("socket binary {}", bin.display()))?;
        let child = spawn(&bin, target, workdir, port, rate_limits)?;
        let mut s = Socket {
            child,
            port,
            bin,
            workdir: workdir.to_owned(),
            target: target.clone(),
            rate_limits,
        };
        s.wait_ready().await?;
        Ok(s)
    }

    pub async fn kill_and_restart(&mut self) -> Result<()> {
        self.child.kill().await?;
        self.child = spawn(&self.bin, &self.target, &self.workdir, self.port, self.rate_limits)?;
        self.wait_ready().await
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

fn spawn(bin: &Path, target: &Target, workdir: &Path, port: u16, rate_limits: bool) -> Result<Child> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(workdir.join("socket.log"))?;
    let m = &target.mysql;
    let mut cmd = Command::new(bin);
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
        .env("WEBSOCKET_APP_PORT", free_port()?.to_string())
        .env("BOT_PORT", free_port()?.to_string())
        .env("STATS_PORT", free_port()?.to_string())
        .env("WAL_DIR", workdir.join("wal"))
        .env("RATE_LIMITS", if rate_limits { "on" } else { "off" })
        .env("HOST_URL", crate::env::SITE_ORIGIN)
        .env("TRUST_PROXY_HOPS", "1")
        .env("RUST_LOG", "warn")
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
