use anyhow::{Result, bail};

pub const DATABASE_PREFIX: &str = "cgg_harness";

pub fn local_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "mysql" | "redis")
}

pub fn check_mysql(host: &str, database: &str) -> Result<()> {
    if !local_host(host) {
        bail!("refusing MySQL host {host}: the harness only runs against a local database");
    }
    if !database.starts_with(DATABASE_PREFIX) {
        bail!("refusing database {database}: harness databases must start with {DATABASE_PREFIX}");
    }
    Ok(())
}

pub fn check_redis(url: &str) -> Result<()> {
    let rest = url.strip_prefix("redis://").unwrap_or(url);
    let rest = rest.rsplit('@').next().unwrap_or(rest);
    let (authority, db) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    if !local_host(host) {
        bail!("refusing Redis host {host}: the harness only runs against a local Redis");
    }
    match db.parse::<u32>() {
        Ok(n) if n > 0 => Ok(()),
        _ => bail!("refusing Redis url {url}: give a database index above 0, e.g. redis://127.0.0.1:6379/9"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mysql_must_be_local_and_prefixed() {
        assert!(check_mysql("127.0.0.1", "cgg_harness").is_ok());
        assert!(check_mysql("127.0.0.1", "cgg").is_err());
        assert!(check_mysql("db.example.com", "cgg_harness").is_err());
    }

    #[test]
    fn redis_must_be_local_with_a_nonzero_database() {
        assert!(check_redis("redis://127.0.0.1:6382/9").is_ok());
        assert!(check_redis("redis://:pw@localhost:6379/3").is_ok());
        assert!(check_redis("redis://127.0.0.1:6382").is_err());
        assert!(check_redis("redis://127.0.0.1:6382/0").is_err());
        assert!(check_redis("redis://cache.example.com:6379/9").is_err());
    }
}
