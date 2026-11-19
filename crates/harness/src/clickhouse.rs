use anyhow::{Context as _, Result, bail};
use post_codec::{Context, tsv};
use sqlx::MySqlPool;

use crate::env::CLICKHOUSE_DATABASE;

pub struct Clickhouse {
    http: reqwest::Client,
    url: String,
}

impl Clickhouse {
    pub fn new(url: &str) -> Result<Clickhouse> {
        Ok(Clickhouse {
            http: reqwest::Client::builder().no_proxy().build()?,
            url: url.to_owned(),
        })
    }

    pub async fn query(&self, sql: &str) -> Result<String> {
        let res = self
            .http
            .post(&self.url)
            .query(&[("database", CLICKHOUSE_DATABASE)])
            .body(sql.to_owned())
            .send()
            .await
            .with_context(|| format!("connect to ClickHouse at {}", self.url))?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            bail!("clickhouse {status}: {body}");
        }
        Ok(body)
    }

    pub async fn stored_rows(&self) -> Result<String> {
        self.query(&format!(
            "SELECT {} FROM post ORDER BY threadId, id FORMAT TSV",
            tsv::ROW_COLUMNS
        ))
        .await
    }

    pub async fn count(&self, table: &str) -> Result<u64> {
        Ok(self
            .query(&format!("SELECT count() FROM {table}"))
            .await?
            .trim()
            .parse()?)
    }
}

pub async fn expected_rows(pool: &MySqlPool) -> Result<String> {
    let rows = sqlx::query(&format!(
        "SELECT {} FROM post ORDER BY threadId, id",
        tsv::PLAIN_COLUMNS
    ))
    .fetch_all(pool)
    .await?;
    let mut out = String::new();
    let mut ctx = Context::default();
    let mut thread = None;
    for r in &rows {
        let p = post_codec::mysql::plain(r)?;
        if thread != Some(p.thread_id) {
            ctx = Context::default();
            thread = Some(p.thread_id);
        }
        tsv::write_row(&post_codec::encode(&ctx, &p), &mut out);
        out.push('\n');
        ctx.advance(&p);
    }
    Ok(out)
}

fn fields(line: &str) -> Vec<Option<String>> {
    line.split('\t').map(tsv::unescape).collect()
}

pub fn first_difference(expected: &str, stored: &str) -> Option<String> {
    let want: Vec<_> = expected.lines().filter(|l| !l.is_empty()).collect();
    let got: Vec<_> = stored.lines().filter(|l| !l.is_empty()).collect();
    if want.len() != got.len() {
        return Some(format!("MySQL has {} posts, ClickHouse {}", want.len(), got.len()));
    }
    want.iter()
        .zip(&got)
        .map(|(w, g)| (fields(w), fields(g)))
        .find(|(w, g)| w != g)
        .map(|(w, g)| format!("expected {w:?}, stored {g:?}"))
}
