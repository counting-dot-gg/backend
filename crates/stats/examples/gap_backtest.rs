use anyhow::Result;
use sqlx::MySqlPool;
use stats::db::{get_max_post_id, post_ids_with_times};
use stats::stats_engine::settled_end;
use std::time::Instant;

const RUN_EVERY_MS: i64 = 60_000;
const RACE_MS: i64 = 600_000;
const SCAN_IDS: u64 = 20_000;
const PHASES: [i64; 6] = [0, 10_000, 20_000, 30_000, 40_000, 50_000];
const GRACES: [i64; 5] = [5_000, 30_000, 60_000, 120_000, 300_000];

struct Outcome {
    skipped: u64,
    skipped_races: u64,
    holds: u64,
    delays: Vec<i64>,
}

fn percentile(sorted: &[i64], p: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn lateness_of(posts: &[(u64, i64)]) -> Vec<i64> {
    let mut later_min = i64::MAX;
    let mut lateness = vec![0i64; posts.len()];
    for (k, &(_, at)) in posts.iter().enumerate().rev() {
        lateness[k] = (at - later_min).max(0);
        later_min = later_min.min(at);
    }
    lateness
}

fn replay(posts: &[(u64, i64)], lateness: &[i64], phase: i64, grace: Option<i64>) -> Outcome {
    let mut by_time: Vec<(i64, u64)> = posts.iter().map(|&(id, at)| (at, id)).collect();
    by_time.sort_unstable();
    let first_at = by_time[0].0;
    let last_at = by_time[by_time.len() - 1].0;
    let mut out = Outcome {
        skipped: 0,
        skipped_races: 0,
        holds: 0,
        delays: Vec::with_capacity(posts.len()),
    };
    let mut cursor = posts[0].0 - 1;
    let mut max_visible = 0u64;
    let mut seen = 0usize;
    let mut run_at = first_at - first_at.rem_euclid(RUN_EVERY_MS) + phase;
    while run_at <= last_at + 2 * RUN_EVERY_MS + grace.unwrap_or(0) {
        while seen < by_time.len() && by_time[seen].0 <= run_at {
            max_visible = max_visible.max(by_time[seen].1);
            seen += 1;
        }
        if max_visible > cursor {
            let lo = posts.partition_point(|p| p.0 <= cursor);
            let hi = posts.partition_point(|p| p.0 <= max_visible);
            let end = match grace {
                None => max_visible,
                Some(g) => {
                    let scan_after = cursor.max(max_visible.saturating_sub(SCAN_IDS));
                    let from = posts.partition_point(|p| p.0 <= scan_after);
                    let visible: Vec<(u64, i64)> = posts[from..hi]
                        .iter()
                        .copied()
                        .filter(|&(_, at)| at <= run_at)
                        .collect();
                    settled_end(scan_after, max_visible, &visible, run_at, g)
                }
            };
            if end < max_visible {
                out.holds += 1;
            }
            let upto = posts.partition_point(|p| p.0 <= end);
            for (k, &(_, at)) in posts[lo..upto].iter().enumerate() {
                if at <= run_at {
                    out.delays.push(run_at - at);
                } else {
                    out.skipped += 1;
                    if lateness[lo + k] <= RACE_MS {
                        out.skipped_races += 1;
                    }
                }
            }
            cursor = end;
        }
        run_at += RUN_EVERY_MS;
    }
    out.delays.sort_unstable();
    out
}

async fn first_id_after(pool: &MySqlPool, newest: u64, cutoff_ms: i64) -> Result<u64> {
    let mut lo = 1u64;
    let mut hi = newest;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let probe = post_ids_with_times(pool, mid, mid + 200).await?;
        let typical = probe.get(probe.len() / 2).map_or(0, |p| p.1);
        if typical < cutoff_ms { lo = mid + 1 } else { hi = mid }
    }
    Ok(lo)
}

async fn time_query(pool: &MySqlPool, newest: u64, width: u64) -> Result<(f64, f64)> {
    let mut ms = Vec::new();
    for k in 0..10u64 {
        let upto = newest - k * 997_331;
        let started = Instant::now();
        post_ids_with_times(pool, upto - width, upto).await?;
        ms.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(f64::total_cmp);
    Ok((ms[ms.len() / 2], ms[ms.len() - 1]))
}

#[tokio::main]
async fn main() -> Result<()> {
    cgg_core::load_env();
    let days: i64 = std::env::args().nth(1).and_then(|d| d.parse().ok()).unwrap_or(7);
    let pool = cgg_core::MysqlConfig::from_env()?.pool(4).await?;

    let newest = get_max_post_id(&pool).await?;
    let tail = post_ids_with_times(&pool, newest - 10_000, newest).await?;
    let newest_at = tail.iter().map(|p| p.1).max().unwrap_or(0);
    let first = first_id_after(&pool, newest, newest_at - days * 86_400_000).await?;

    let loading = Instant::now();
    let mut posts: Vec<(u64, i64)> = Vec::new();
    let mut after = first - 1;
    while after < newest {
        let upto = (after + 500_000).min(newest);
        posts.extend(post_ids_with_times(&pool, after, upto).await?);
        after = upto;
    }
    println!(
        "window: {days} days, ids {first}..{newest}, {} posts ({:.0}/day), loaded in {:.1}s",
        posts.len(),
        posts.len() as f64 / days as f64,
        loading.elapsed().as_secs_f64()
    );

    let missing = (newest - first + 1) - posts.len() as u64;
    let gap_starts = posts.windows(2).filter(|w| w[1].0 > w[0].0 + 1).count();
    println!("ids never written: {missing} in {gap_starts} gaps");

    let lateness = lateness_of(&posts);
    let anomalies = lateness.iter().filter(|&&l| l > RACE_MS).count();
    let mut races: Vec<i64> = lateness.iter().copied().filter(|&l| l > 0 && l <= RACE_MS).collect();
    races.sort_unstable();
    println!("posts over 10 min out of id order (imports, clock jumps): {anomalies}");
    println!(
        "races, a higher id timestamped earlier by up to 10 min: {} ({:.3}%), late by p50 {} ms, p99 {} ms, max {} ms",
        races.len(),
        100.0 * races.len() as f64 / posts.len() as f64,
        percentile(&races, 0.5),
        percentile(&races, 0.99),
        races.last().copied().unwrap_or(0)
    );

    println!("\nrule        skipped: races / all   holds/day   delay p50    p99    max");
    for grace in std::iter::once(None).chain(GRACES.iter().map(|&g| Some(g))) {
        let runs: Vec<Outcome> = PHASES.iter().map(|&p| replay(&posts, &lateness, p, grace)).collect();
        let n = runs.len() as f64;
        let skipped = runs.iter().map(|r| r.skipped).sum::<u64>() as f64 / n;
        let skipped_races = runs.iter().map(|r| r.skipped_races).sum::<u64>() as f64 / n;
        let holds = runs.iter().map(|r| r.holds).sum::<u64>() as f64 / n / days as f64;
        let mut delays: Vec<i64> = runs.iter().flat_map(|r| r.delays.iter().copied()).collect();
        delays.sort_unstable();
        let label = grace.map_or("current".to_owned(), |g| format!("hold {}s", g / 1000));
        println!(
            "{label:<11} {skipped_races:>9.1} / {skipped:<9.1} {holds:>9.1}   {:>7.1}s {:>6.1}s {:>6.1}s",
            percentile(&delays, 0.5) as f64 / 1000.0,
            percentile(&delays, 0.99) as f64 / 1000.0,
            delays.last().copied().unwrap_or(0) as f64 / 1000.0
        );
    }

    println!("\nextra query per run, ms (median / worst of 10 spots):");
    for width in [100u64, 2_000, 20_000] {
        let (median, worst) = time_query(&pool, newest, width).await?;
        println!("  {width:>6} ids: {median:.2} / {worst:.2}");
    }
    let started = Instant::now();
    get_max_post_id(&pool).await?;
    println!("  MAX(id) for scale: {:.2}", started.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}
