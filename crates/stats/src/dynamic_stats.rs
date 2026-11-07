use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use crate::models::DynamicStatParams;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicStatRow {
    pub start_uuid: Option<String>,
    pub end_uuid: Option<String>,
    pub time_diff_ms: Option<i64>,
    pub start_valid_count_number: Option<i64>,
    pub end_valid_count_number: Option<i64>,
    pub qualified_counters: Vec<String>,
}

pub async fn run_dynamic_query(pool: &MySqlPool, params: &DynamicStatParams) -> Result<Vec<DynamicStatRow>> {
    let sql = format!(
        r#"
        SELECT * FROM (
            SELECT
                end_post_uuid       AS start_uuid,
                LEAD(end_post_uuid)  OVER (PARTITION BY thread ORDER BY end_time) AS end_uuid,
                end_valid_count_number AS start_valid_count_number,
                LEAD(end_valid_count_number) OVER (PARTITION BY thread ORDER BY end_time) AS end_valid_count_number,
                TIMESTAMPDIFF(
                    MICROSECOND,
                    end_time,
                    LEAD(end_time) OVER (PARTITION BY thread ORDER BY end_time)
                ) / 1000 AS time_diff_ms,
                qualifying_authorUUIDs
            FROM (
                SELECT
                    thread,
                    split_number,
                    FROM_UNIXTIME(MAX(end_time_ms / 1000)) AS end_time,
                    MAX(end_post_uuid)              AS end_post_uuid,
                    MAX(end_valid_count_number)     AS end_valid_count_number,
                    GROUP_CONCAT(
                        CASE WHEN post_count >= {min_posts} THEN authorUUID ELSE NULL END
                    ) AS qualifying_authorUUIDs
                FROM (
                    SELECT
                        thread,
                        split_number,
                        MAX(CAST(timestamp AS UNSIGNED))   AS end_time_ms,
                        MAX(uuid)                          AS end_post_uuid,
                        MAX(validCountNumber)               AS end_valid_count_number,
                        authorUUID,
                        COUNT(*) AS post_count
                    FROM (
                        SELECT
                            thread,
                            timestamp,
                            uuid,
                            authorUUID,
                            validCountNumber,
                            FLOOR((ROW_NUMBER() OVER (PARTITION BY thread ORDER BY timestamp) - 1 + {offset}) / {size}) + 1
                                AS split_number
                        FROM post
                        WHERE isValidCount = 1
                          AND thread = ?
                    ) AS numbered_rows
                    GROUP BY thread, split_number, authorUUID
                ) AS splits
                GROUP BY thread, split_number
                ORDER BY end_valid_count_number
            ) AS grouped
        ) AS result
        WHERE end_uuid IS NOT NULL
          AND start_uuid IS NOT NULL
          AND (end_valid_count_number + {offset}) % {size} = 0
          AND (start_valid_count_number + {offset}) % {size} = 0
        ORDER BY time_diff_ms
        "#,
        min_posts = params.min_posts_qualification,
        offset = params.split_offset,
        size = params.split_size,
    );

    let rows = sqlx::query_as::<_, RawDynamicRow>(&sql)
        .bind(&params.thread_uuid)
        .fetch_all(pool)
        .await?;

    let result = rows
        .into_iter()
        .map(|r| DynamicStatRow {
            start_uuid: r.start_uuid,
            end_uuid: r.end_uuid,
            time_diff_ms: r.time_diff_ms,
            start_valid_count_number: r.start_valid_count_number,
            end_valid_count_number: r.end_valid_count_number,
            qualified_counters: r
                .qualifying_author_uuids
                .as_deref()
                .unwrap_or("")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect(),
        })
        .collect();

    Ok(result)
}

#[derive(sqlx::FromRow)]
struct RawDynamicRow {
    start_uuid: Option<String>,
    end_uuid: Option<String>,
    time_diff_ms: Option<i64>,
    start_valid_count_number: Option<i64>,
    end_valid_count_number: Option<i64>,
    qualifying_author_uuids: Option<String>,
}
