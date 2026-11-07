use anyhow::{Context, Result, bail};
use sqlx::{ConnectOptions, Executor, MySqlConnection, Row};

use crate::guard;

pub async fn dump_from_dev(out: &std::path::Path) -> Result<()> {
    let cfg = cgg_core::MysqlConfig::from_env()?;
    if !guard::local_host(&cfg.host) {
        bail!("refusing to read schema from {}: dev MySQL must be local", cfg.host);
    }
    let mut conn = cfg.connect_options().connect().await.context("connect to dev MySQL")?;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(table_name AS CHAR) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' ORDER BY table_name",
    )
    .fetch_all(&mut conn)
    .await?;
    let mut sql = String::new();
    for t in &tables {
        let row = sqlx::query(&format!("SHOW CREATE TABLE `{t}`"))
            .fetch_one(&mut conn)
            .await?;
        let create: String = row.try_get(1)?;
        sql.push_str(&strip_auto_increment(&create));
        sql.push_str(";\n\n");
    }
    std::fs::write(out, sql)?;
    println!("wrote {} tables to {}", tables.len(), out.display());
    Ok(())
}

fn strip_auto_increment(create: &str) -> String {
    create
        .split(' ')
        .filter(|w| !w.starts_with("AUTO_INCREMENT="))
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn load(conn: &mut MySqlConnection, schema: &str) -> Result<()> {
    conn.execute("SET FOREIGN_KEY_CHECKS = 0").await?;
    for statement in statements(schema) {
        conn.execute(statement.as_str())
            .await
            .with_context(|| first_line(&statement).to_owned())?;
    }
    conn.execute("SET FOREIGN_KEY_CHECKS = 1").await?;
    Ok(())
}

fn statements(schema: &str) -> Vec<String> {
    schema
        .replace("\r\n", "\n")
        .split(";\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

#[cfg(test)]
mod tests {
    #[test]
    fn statements_split_the_same_with_either_line_ending() {
        let lf = "CREATE TABLE a (x int);\n\nCREATE TABLE b (y int);\n\n";
        assert_eq!(super::statements(lf), super::statements(&lf.replace('\n', "\r\n")));
        assert_eq!(super::statements(lf).len(), 2);
    }

    #[test]
    fn auto_increment_counters_are_dropped() {
        let create = "CREATE TABLE `t` (\n  `id` int\n) ENGINE=InnoDB AUTO_INCREMENT=91 DEFAULT CHARSET=utf8mb4";
        assert_eq!(
            super::strip_auto_increment(create),
            "CREATE TABLE `t` (\n  `id` int\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
        );
    }
}
