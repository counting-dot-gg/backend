use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

use crate::client::Client;
use crate::env::{Target, ThreadSpec, account, seed};
use crate::server::Socket;

pub const ACCOUNTS: usize = 5;

const THREADS: [ThreadSpec; 2] = [
    ThreadSpec {
        name: "main",
        allow_double_posts: false,
    },
    ThreadSpec {
        name: "double_counting",
        allow_double_posts: true,
    },
];

pub async fn run(bin: &Path, port: u16, keep: bool) -> Result<()> {
    let target = Target::dev()?;
    let reuse = keep && target.has_data().await?;
    if !reuse {
        target.reset().await?;
        seed(&target.pool().await?, &target, &THREADS, ACCOUNTS).await?;
    }
    let socket = Socket::start_on(bin, &target, Path::new("target/dev"), true, port, !reuse)
        .await
        .with_context(|| format!("starting {}", bin.display()))?;

    println!();
    println!("counting.gg backend running");
    println!();
    println!("  socket    ws://127.0.0.1:{}/socket.io/", socket.port);
    println!(
        "  database  {} ({})",
        target.mysql.database,
        if reuse { "kept" } else { "fresh" }
    );
    println!(
        "  threads   {}",
        THREADS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
    );
    println!("  log       {}", socket.log_path().display());
    println!();
    println!("  try it    cgg-harness say main 1");
    println!("            cgg-harness say --as 1 main 2");
    println!();
    println!("Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    drop(socket);
    Ok(())
}

pub async fn say(port: u16, who: usize, thread: &str, text: &str) -> Result<()> {
    let target = Target::dev()?;
    let acc = account(&target, who.min(ACCOUNTS - 1));
    let mut c = Client::connect(port, Some(&acc.cookie))
        .await
        .with_context(|| format!("is `cgg-harness dev` running on port {port}?"))?
        .authored_by(&acc.uuid);
    c.watch(thread)
        .await
        .with_context(|| format!("no thread called {thread}"))?;
    c.post(thread, text)?;
    let e = c.own_post(text, Duration::from_secs(5)).await?;
    let p = &e.data["post"];
    match (p["isValidCount"].as_bool(), p["validCountNumber"].as_i64()) {
        (Some(true), Some(n)) => println!("harness_{who}: {text}  valid, count #{n}"),
        (Some(true), None) => println!("harness_{who}: {text}  valid"),
        _ if p["isCount"].as_bool() == Some(true) => println!("harness_{who}: {text}  not valid"),
        _ => println!("harness_{who}: {text}  (comment)"),
    }
    Ok(())
}
