mod bench;
mod client;
mod compare;
mod dev;
mod env;
mod guard;
mod perf;
mod regional;
mod schema;
mod server;
mod suite;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "cgg-harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone)]
struct LoadArgs {
    #[arg(long, default_value_t = 4)]
    threads: usize,
    #[arg(long, default_value_t = 40)]
    watchers: usize,
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    #[arg(long, default_value_t = 3)]
    rounds: usize,
    #[arg(long, default_value_t = 32)]
    clients: usize,
    #[arg(long, default_value_t = 1000)]
    burst: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Kind {
    Socket,
    Api,
    Bot,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Socket => "socket",
            Kind::Api => "api",
            Kind::Bot => "bot",
        }
    }

    fn default_bin(self) -> PathBuf {
        PathBuf::from(format!(
            "target/release/cgg-{}{}",
            self.name(),
            std::env::consts::EXE_SUFFIX
        ))
    }

    fn rules(self) -> &'static [compare::Rule] {
        match self {
            Kind::Socket => &compare::SOCKET_RULES,
            Kind::Api => &compare::API_RULES,
            Kind::Bot => &compare::BOT_RULES,
        }
    }
}

impl LoadArgs {
    fn load(&self) -> bench::Load {
        bench::Load {
            threads: self.threads,
            watchers: self.watchers,
            seconds: self.seconds,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    DumpSchema {
        #[arg(long, default_value = "crates/harness/schema.sql")]
        out: PathBuf,
    },
    Test {
        #[arg(long, default_value_os_t = default_socket_bin())]
        socket_bin: PathBuf,
        #[arg(long)]
        only: Option<String>,
        #[arg(long, default_value = "target/harness")]
        out: PathBuf,
    },
    Bench {
        #[arg(long, value_enum, default_value_t = Kind::Socket)]
        kind: Kind,
        #[arg(long, alias = "socket-bin")]
        bin: Option<PathBuf>,
        #[command(flatten)]
        load: LoadArgs,
        #[arg(long, default_value = "target/harness")]
        out: PathBuf,
    },
    Ab {
        #[arg(long, value_enum, default_value_t = Kind::Socket)]
        kind: Kind,
        #[arg(long)]
        base_bin: PathBuf,
        #[arg(long)]
        head_bin: PathBuf,
        #[command(flatten)]
        load: LoadArgs,
        #[arg(long, default_value = "target/harness")]
        out: PathBuf,
    },
    Compare {
        #[arg(long, value_enum, default_value_t = Kind::Socket)]
        kind: Kind,
        base: PathBuf,
        head: PathBuf,
    },
    Dev {
        #[arg(long, default_value_os_t = default_socket_bin())]
        socket_bin: PathBuf,
        #[arg(long, default_value_t = 8805)]
        port: u16,
        #[arg(long)]
        keep: bool,
    },
    Regional {
        #[arg(long, default_value_os_t = default_socket_bin())]
        socket_bin: PathBuf,
        #[arg(long, default_value = "workers/do-poc")]
        worker_dir: PathBuf,
        #[arg(long, default_value = "e2e/regional")]
        suite_dir: PathBuf,
        #[arg(long, default_value = "../frontend")]
        frontend: PathBuf,
        #[arg(long, default_value = "target/regional")]
        out: PathBuf,
        scenarios: Option<String>,
    },
    Say {
        #[arg(long = "as", default_value_t = 0)]
        who: usize,
        #[arg(long, default_value_t = 8805)]
        port: u16,
        thread: String,
        text: String,
    },
}

fn default_socket_bin() -> PathBuf {
    PathBuf::from(format!("target/release/cgg-socket{}", std::env::consts::EXE_SUFFIX))
}

#[tokio::main]
async fn main() -> Result<()> {
    cgg_core::load_env();
    match Cli::parse().command {
        Command::DumpSchema { out } => schema::dump_from_dev(&out).await,
        Command::Test { socket_bin, only, out } => test(&socket_bin, only.as_deref(), &out).await,
        Command::Bench { kind, bin, load, out } => {
            let target = env::Target::from_env()?;
            let bin = bin.unwrap_or_else(|| kind.default_bin());
            let report = bench_rounds(kind, "head", &[("head", &bin)], &target, &load, &out)
                .await?
                .remove(0);
            println!("{}", serde_json::to_string_pretty(&report.median)?);
            Ok(())
        }
        Command::Ab {
            kind,
            base_bin,
            head_bin,
            load,
            out,
        } => {
            let target = env::Target::from_env()?;
            let mut reports = bench_rounds(
                kind,
                "ab",
                &[("base", &base_bin), ("head", &head_bin)],
                &target,
                &load,
                &out,
            )
            .await?;
            let head = reports.pop().expect("two reports");
            let base = reports.pop().expect("two reports");
            judge(kind, &base.median, &head.median, &out)
        }
        Command::Dev { socket_bin, port, keep } => dev::run(&socket_bin, port, keep).await,
        Command::Regional {
            socket_bin,
            worker_dir,
            suite_dir,
            frontend,
            out,
            scenarios,
        } => {
            regional::run(regional::Options {
                socket_bin,
                worker_dir,
                suite_dir,
                frontend,
                scenarios,
                out,
            })
            .await
        }
        Command::Say {
            who,
            port,
            thread,
            text,
        } => dev::say(port, who, &thread, &text).await,
        Command::Compare { kind, base, head } => {
            let read = |p: &Path| -> Result<bench::Report> { Ok(serde_json::from_slice(&std::fs::read(p)?)?) };
            judge(
                kind,
                &read(&base)?.median,
                &read(&head)?.median,
                Path::new("target/harness"),
            )
        }
    }
}

async fn test(bin: &Path, only: Option<&str>, out: &Path) -> Result<()> {
    let target = env::Target::from_env()?;
    std::fs::create_dir_all(out)?;
    let results = suite::run(bin, &target, &out.join("test-run"), only).await?;
    let summary = suite::summary(&results);
    std::fs::write(out.join("test.json"), serde_json::to_vec_pretty(&summary)?)?;
    let failed = results.iter().filter(|r| !r.passed).count();
    if failed > 0 {
        bail!("{failed} of {} harness cases failed", results.len());
    }
    println!("all {} harness cases passed", results.len());
    Ok(())
}

async fn bench_rounds(
    kind: Kind,
    run: &str,
    bins: &[(&str, &PathBuf)],
    target: &env::Target,
    load: &LoadArgs,
    out: &Path,
) -> Result<Vec<bench::Report>> {
    std::fs::create_dir_all(out)?;
    let mut rounds: Vec<Vec<bench::Metrics>> = vec![Vec::new(); bins.len()];
    for r in 0..load.rounds {
        for (i, (label, bin)) in bins.iter().enumerate() {
            let workdir = std::path::absolute(out.join(format!("{}-{run}-{label}-{r}", kind.name())))?;
            let perf = perf::Load {
                seconds: load.seconds,
                clients: load.clients,
                burst: load.burst,
            };
            let m = match kind {
                Kind::Socket => bench::round(bin, target, &workdir, load.load()).await?,
                Kind::Api => perf::api_round(bin, target, &workdir, perf).await?,
                Kind::Bot => perf::bot_round(bin, target, &workdir, perf).await?,
            };
            println!("round {} {label}: {}", r + 1, serde_json::to_string(&m)?);
            rounds[i].push(m);
        }
    }
    let mut reports = Vec::new();
    for ((label, _), rounds) in bins.iter().zip(rounds) {
        let report = bench::Report {
            label: label.to_string(),
            median: bench::median_of(&rounds),
            rounds,
        };
        std::fs::write(
            out.join(format!("bench-{}-{label}.json", kind.name())),
            serde_json::to_vec_pretty(&report)?,
        )?;
        reports.push(report);
    }
    Ok(reports)
}

fn judge(kind: Kind, base: &bench::Metrics, head: &bench::Metrics, out: &Path) -> Result<()> {
    let rows = compare::compare(kind.rules(), base, head);
    let table = compare::markdown(&rows);
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join(format!("comparison-{}.md", kind.name())), &table)?;
    println!("{table}");
    let regressed: Vec<&str> = rows.iter().filter(|r| r.regressed).map(|r| r.metric).collect();
    if !regressed.is_empty() {
        bail!("performance regressed: {}", regressed.join(", "));
    }
    Ok(())
}
