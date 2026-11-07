use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::routes::sitegoals;
use crate::state::AppState;
use crate::users;
use serde_json::{Value, json};
use stats::achievements::LiveEvent;

pub async fn site_goal_contribute(state: &AppState, uuid: &str, goal_key: &str, amount: i64) -> ApiResult<()> {
    let _turn = state.jobs.lock().await;
    if uuid.is_empty() || goal_key.is_empty() || amount <= 0 {
        return Ok(());
    }
    let Some(user) = crate::db::find_user(state, uuid).await? else {
        return Ok(());
    };
    if crate::http::js_number(&user["money"]) < amount as f64 {
        return Ok(());
    }
    users::add_money(state, uuid, -amount).await?;
    let result = sitegoals::apply_contribution(state, goal_key, uuid, amount).await;
    match result {
        Ok(Ok(_)) => {
            state.socket.emit("updateUser", json!({ "uuid": uuid }));
            Ok(())
        }
        Ok(Err(_)) => users::add_money(state, uuid, amount).await,
        Err(e) => {
            users::add_money(state, uuid, amount).await?;
            Err(e)
        }
    }
}

fn has(counter: &Value, role: &str) -> bool {
    crate::db::has_role(counter, role)
}

pub async fn shop_purchase(state: &AppState, user: &Value, item: &Value) -> ApiResult<()> {
    let _turn = state.jobs.lock().await;
    let uuid = user["uuid"].as_str().unwrap_or_default();
    let owned: Option<(i32,)> =
        sqlx::query_as("SELECT ui.id FROM useritem ui WHERE ui.userUuid = ? AND ui.itemId = ? LIMIT 1")
            .bind(uuid)
            .bind(item["id"].as_i64())
            .fetch_optional(&state.pool)
            .await?;
    if owned.is_some() {
        return Ok(());
    }
    let price = crate::http::js_number(&item["price"]);
    let money = crate::db::find_user(state, uuid)
        .await?
        .map_or(f64::NAN, |u| crate::http::js_number(&u["money"]));
    if money.is_nan() || money < price || item["unlockMethod"] != "shop" {
        return Ok(());
    }
    sqlx::query("INSERT INTO useritem (unlocked, userUuid, itemId) VALUES (1, ?, ?)")
        .bind(uuid)
        .bind(item["id"].as_i64())
        .execute(&state.pool)
        .await?;
    users::add_money(state, uuid, -(price as i64)).await?;
    state.socket.emit("updateUserItems", json!({ "uuid": uuid }));
    Ok(())
}

async fn blog_json(state: &AppState, uuid: &str, with_author: bool) -> ApiResult<Value> {
    let row = sqlx::query("SELECT * FROM blog WHERE uuid = ? LIMIT 1")
        .bind(uuid)
        .fetch_optional(&state.pool)
        .await?;
    let Some(row) = row else {
        return Ok(Value::Null);
    };
    let mut b = entity::BLOG.json(&row);
    if with_author {
        let author: Option<String> = sqlx::Row::try_get(&row, "authorUuid").ok().flatten();
        b["author"] = match author {
            Some(a) => crate::db::find_counter(state, &a).await?.unwrap_or(Value::Null),
            None => Value::Null,
        };
    }
    Ok(b)
}

fn check_tags(tags: &[Value], compare_trimmed: bool) -> ApiResult<()> {
    let trimmed: Vec<String> = tags
        .iter()
        .map(|t| counting::js::trim(t.as_str().unwrap_or("")).to_owned())
        .collect();
    for tag in &trimmed {
        let n = tag.encode_utf16().count();
        if n > 20 {
            return Err(ApiError::bad_request("Tag is too long."));
        }
        if n < 1 {
            return Err(ApiError::bad_request("Tag is too short."));
        }
        let dupes = if compare_trimmed {
            trimmed.iter().filter(|t| *t == tag).count()
        } else {
            tags.iter().filter(|t| t.as_str() == Some(tag.as_str())).count()
        };
        if dupes > 1 {
            return Err(ApiError::bad_request("Duplicate tags."));
        }
    }
    Ok(())
}

pub async fn create_blog(state: &AppState, counter: &Value, title: &str, body: &str, tags: &Value) -> ApiResult<Value> {
    let _turn = state.jobs.lock().await;
    let author = counter["uuid"].as_str().unwrap_or_default();
    let now = cgg_web::now_ms();
    let (recent,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM blog WHERE authorUuid = ? AND timestamp > ? AND timestamp < ?")
            .bind(author)
            .bind((now - 86_400_000).to_string())
            .bind(now.to_string())
            .fetch_one(&state.pool)
            .await?;
    if recent > 3 && !has(counter, "admin") {
        return Err(ApiError::bad_request(
            "User already has 3 blogs within the last 24 hours.",
        ));
    }
    let Some(all_tags) = tags.as_array() else {
        return Err(ApiError::Internal(anyhow::anyhow!("tags missing")));
    };
    let valid: Vec<Value> = all_tags.iter().take(4).cloned().collect();
    check_tags(&valid, false)?;
    let ns = state.clock.now_ns();
    let uuid = cgg_web::ids::uuid_for(ns);
    sqlx::query("INSERT INTO blog (uuid, timestamp, title, body, tags, authorUuid) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(&uuid)
        .bind(cgg_web::ids::timestamp_for(ns))
        .bind(title)
        .bind(body)
        .bind(Value::Array(valid).to_string())
        .bind(author)
        .execute(&state.pool)
        .await?;
    crate::achievements::record(state, author, LiveEvent::BlogPublished);
    blog_json(state, &uuid, true).await
}

pub async fn update_blog(
    state: &AppState,
    counter: &Value,
    uuid: &str,
    title: &Value,
    body: &Value,
    tags: &Value,
) -> ApiResult<Value> {
    let _turn = state.jobs.lock().await;
    let blog = blog_json(state, uuid, true).await?;
    if blog.is_null() {
        return Err(ApiError::not_found(format!("No blog {uuid}.")));
    }
    if blog["author"]["uuid"] != counter["uuid"] {
        return Err(ApiError::forbidden("Not your blog."));
    }
    let valid: Vec<Value> = tags
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .take(4)
        .map(|t| json!(counting::js::trim(t.as_str().unwrap_or(""))))
        .collect();
    check_tags(&valid, true)?;
    sqlx::query("UPDATE blog SET title = ?, body = ?, tags = ?, edited_timestamp = ? WHERE uuid = ?")
        .bind(title.as_str())
        .bind(body.as_str())
        .bind(Value::Array(valid).to_string())
        .bind(cgg_web::now_ms().to_string())
        .bind(uuid)
        .execute(&state.pool)
        .await?;
    blog_json(state, uuid, true).await
}

pub async fn hide_blog(state: &AppState, counter: &Value, uuid: &str, hidden: bool) -> ApiResult<Value> {
    let _turn = state.jobs.lock().await;
    if !has(counter, "admin") && !has(counter, "mod") {
        return Err(ApiError::forbidden("Moderators only."));
    }
    let blog = blog_json(state, uuid, false).await?;
    if blog.is_null() {
        return Err(ApiError::not_found(format!("No blog {uuid}.")));
    }
    sqlx::query("UPDATE blog SET hidden = ? WHERE uuid = ?")
        .bind(hidden)
        .bind(uuid)
        .execute(&state.pool)
        .await?;
    blog_json(state, uuid, false).await
}

pub async fn unlock_xp_reward(state: &AppState, uuid: &str, level: f64) -> ApiResult<Value> {
    let _turn = state.jobs.lock().await;
    if crate::db::find_user(state, uuid).await?.is_none() {
        return Ok(Value::Null);
    }
    let counter = crate::db::find_counter(state, uuid).await?.unwrap_or(Value::Null);
    let xp = crate::http::js_number(&counter["xp"]);
    let actual: f64 = crate::consts::level_for(xp).level.parse().unwrap_or(1.0);
    if actual < level {
        return Err(ApiError::bad_request(format!(
            "Counter {} just tried to unlock the reward for level {}, but given they have xp {}, I think their actual level is {}.",
            crate::http::js_string(&counter["name"]),
            counting::js::num_to_string(level),
            crate::http::js_string(&counter["xp"]),
            actual
        )));
    }
    let item = sqlx::query("SELECT * FROM item WHERE levelToUnlock = ? AND season = 1 LIMIT 1")
        .bind(level)
        .fetch_optional(&state.pool)
        .await?
        .map(|r| entity::ITEM.json(&r));
    let Some(item) = item else {
        return Err(ApiError::not_found(format!(
            "No item found for level {}",
            counting::js::num_to_string(level)
        )));
    };
    sqlx::query("INSERT INTO useritem (unlocked, userUuid, itemId) VALUES (1, ?, ?)")
        .bind(uuid)
        .bind(item["id"].as_i64())
        .execute(&state.pool)
        .await?;
    state
        .socket
        .emit("unlockItem", json!({ "item": item, "counter_uuid": uuid }));
    Ok(item)
}
