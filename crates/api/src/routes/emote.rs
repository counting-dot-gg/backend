use crate::db::{self, has_role};
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Reply, reply};
use crate::session::{Authed, SessionUser};
use crate::state::AppState;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use cgg_web::now_ms;
use serde_json::{Value, json};

const MAX_BYTES: usize = 512 * 1024;
const SLOT_CATEGORY: &str = "emote_slot";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/emote", get(list))
        .route("/emote/all", get(list_all))
        .route("/emote/upload", post(upload))
        .route("/emote/slots", get(slots))
        .route("/emote/submit", post(submit))
        .route("/emote/{id}/approve", post(approve))
        .route("/emote/{id}", delete(remove))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
}

fn mime_ext(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some(".png"),
        "image/gif" => Some(".gif"),
        "image/webp" => Some(".webp"),
        "image/jpeg" => Some(".jpg"),
        _ => None,
    }
}

fn bytes_match_mime(bytes: &[u8], mime: &str) -> bool {
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        _ => false,
    }
}

fn valid_name(name: &str) -> bool {
    (2..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

async fn assert_admin(state: &AppState, user: &SessionUser) -> ApiResult<Value> {
    match db::find_counter(state, &user.uuid).await? {
        Some(c) if has_role(&c, "admin") || has_role(&c, "mod") => Ok(c),
        _ => Err(ApiError::bare(StatusCode::FORBIDDEN)),
    }
}

async fn list(State(state): State<AppState>) -> ApiResult<Reply> {
    let rows = sqlx::query("SELECT id, name, filename FROM emote WHERE approved = 1 ORDER BY name ASC")
        .fetch_all(&state.pool)
        .await?;
    Ok(reply(entity::EMOTE.rows(&rows)))
}

async fn list_all(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    assert_admin(&state, &user).await?;
    let rows = sqlx::query("SELECT * FROM emote ORDER BY id DESC")
        .fetch_all(&state.pool)
        .await?;
    Ok(reply(entity::EMOTE.rows(&rows)))
}

struct Upload {
    name: Option<String>,
    file: Option<(Vec<u8>, String)>,
}

async fn read_upload(mut mp: Multipart) -> ApiResult<Upload> {
    let mut up = Upload { name: None, file: None };
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?
    {
        match field.name() {
            Some("name") => up.name = Some(field.text().await.unwrap_or_default()),
            Some("image") => {
                let mime = field.content_type().unwrap_or("").to_owned();
                let bytes = field.bytes().await.map_err(|e| ApiError::bad_request(e.to_string()))?;
                up.file = Some((bytes.to_vec(), mime));
            }
            _ => {}
        }
    }
    Ok(up)
}

async fn create(
    state: &AppState,
    up: &Upload,
    created_by: &str,
    approved: bool,
    owner: Option<&str>,
    slot: Option<i64>,
) -> ApiResult<Value> {
    let name = up.name.as_deref().unwrap_or("").trim().to_owned();
    if !valid_name(&name) {
        return Err(ApiError::bad_request(
            "Name must be 2-32 characters, letters, numbers and underscores only.",
        ));
    }
    let Some((bytes, mime)) = &up.file else {
        return Err(ApiError::bad_request("No image uploaded."));
    };
    if bytes.len() > MAX_BYTES {
        return Err(ApiError::bad_request("Image must be 512KB or smaller."));
    }
    let Some(ext) = mime_ext(mime).filter(|_| bytes_match_mime(bytes, mime)) else {
        return Err(ApiError::bad_request("Image must be a PNG, GIF, WebP or JPEG."));
    };
    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM emote WHERE name = ? LIMIT 1")
        .bind(&name)
        .fetch_optional(&state.pool)
        .await?;
    if existing.is_some() {
        return Err(ApiError::bad_request(format!(
            "An emote named \"{name}\" already exists."
        )));
    }
    let now = now_ms();
    let filename = format!("{name}-{now}{ext}");
    let dir = state.cfg.uploads_dir.join("emotes");
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(dir.join(&filename), bytes).await?;
    let res = sqlx::query(
        "INSERT INTO emote (name, filename, ownerUUID, approved, createdByUUID, slotUserItemId, createdAt) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&name)
    .bind(&filename)
    .bind(owner)
    .bind(approved)
    .bind(created_by)
    .bind(slot)
    .bind(now)
    .execute(&state.pool)
    .await?;
    let mut out = json!({
        "name": name,
        "filename": filename,
        "ownerUUID": owner,
        "approved": approved,
        "createdByUUID": created_by,
        "createdAt": now.to_string(),
        "id": res.last_insert_id(),
    });
    out["slotUserItemId"] = slot.map_or(Value::Null, Value::from);
    Ok(out)
}

async fn upload(State(state): State<AppState>, Authed(user): Authed, mp: Multipart) -> ApiResult<Reply> {
    assert_admin(&state, &user).await?;
    let up = read_upload(mp).await?;
    Ok(reply(create(&state, &up, &user.uuid, true, None, None).await?))
}

async fn find_slots(state: &AppState, uuid: &str) -> ApiResult<Vec<(i64, Option<i64>)>> {
    Ok(sqlx::query_as(
        "SELECT ui.id, ui.consumedAt FROM useritem ui INNER JOIN item ON item.id = ui.itemId \
         INNER JOIN users u ON u.uuid = ui.userUuid \
         WHERE u.uuid = ? AND ui.unlocked = true AND item.category = ? ORDER BY ui.id ASC",
    )
    .bind(uuid)
    .bind(SLOT_CATEGORY)
    .fetch_all(&state.pool)
    .await?)
}

async fn slots(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    let available = find_slots(&state, &user.uuid)
        .await?
        .iter()
        .filter(|(_, c)| c.is_none())
        .count();
    let mine = sqlx::query(
        "SELECT id, name, filename, approved, createdAt FROM emote WHERE createdByUUID = ? ORDER BY id DESC",
    )
    .bind(&user.uuid)
    .fetch_all(&state.pool)
    .await?;
    Ok(reply(
        json!({ "available": available, "submissions": entity::EMOTE.rows(&mine) }),
    ))
}

async fn refund_slot(state: &AppState, id: Option<i64>) -> ApiResult<()> {
    if let Some(id) = id.filter(|i| *i != 0) {
        sqlx::query("UPDATE useritem SET consumedAt = NULL WHERE id = ?")
            .bind(id)
            .execute(&state.pool)
            .await?;
    }
    Ok(())
}

async fn submit(State(state): State<AppState>, Authed(user): Authed, mp: Multipart) -> ApiResult<Reply> {
    let eligible = db::find_counter(&state, &user.uuid)
        .await?
        .is_some_and(|c| has_role(&c, "counter") && !has_role(&c, "banned") && !has_role(&c, "muted"));
    if !eligible {
        return Err(ApiError::forbidden("You cannot submit emotes."));
    }
    let up = read_upload(mp).await?;
    let Some((slot, _)) = find_slots(&state, &user.uuid)
        .await?
        .into_iter()
        .find(|(_, c)| c.is_none())
    else {
        return Err(ApiError::forbidden("You have no emote slots left."));
    };
    let claimed = sqlx::query("UPDATE useritem SET consumedAt = ? WHERE id = ? AND consumedAt IS NULL")
        .bind(now_ms())
        .bind(slot)
        .execute(&state.pool)
        .await?;
    if claimed.rows_affected() == 0 {
        return Err(ApiError::forbidden(
            "That slot was just used. Try again if you have another.",
        ));
    }
    match create(&state, &up, &user.uuid, false, Some(&user.uuid), Some(slot)).await {
        Ok(e) => Ok(reply(json!({
            "id": e["id"],
            "name": e["name"],
            "filename": e["filename"],
            "approved": e["approved"],
        }))),
        Err(err) => {
            refund_slot(&state, Some(slot)).await?;
            Err(err)
        }
    }
}

fn parse_id(id: &str) -> Option<i64> {
    let n = counting::js::parse_int(id, Some(10));
    n.is_finite().then_some(n as i64)
}

async fn find_emote(state: &AppState, id: Option<i64>) -> ApiResult<Value> {
    let Some(id) = id else {
        return Err(ApiError::not_found("Emote not found."));
    };
    let row = sqlx::query("SELECT * FROM emote WHERE id = ? LIMIT 1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    row.map(|r| entity::EMOTE.json(&r))
        .ok_or_else(|| ApiError::not_found("Emote not found."))
}

async fn approve(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(id): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    assert_admin(&state, &user).await?;
    let mut emote = find_emote(&state, parse_id(&id)).await?;
    let approved = body.get("approved") != &Value::Bool(false);
    sqlx::query("UPDATE emote SET approved = ? WHERE id = ?")
        .bind(approved)
        .bind(emote["id"].as_i64())
        .execute(&state.pool)
        .await?;
    emote["approved"] = Value::Bool(approved);
    Ok(reply(emote))
}

async fn remove(State(state): State<AppState>, Authed(user): Authed, Path(id): Path<String>) -> ApiResult<Reply> {
    assert_admin(&state, &user).await?;
    let emote = find_emote(&state, parse_id(&id)).await?;
    if emote["approved"] != Value::Bool(true) {
        refund_slot(&state, emote["slotUserItemId"].as_i64()).await?;
    }
    if let Some(f) = emote["filename"].as_str() {
        let _ = tokio::fs::remove_file(state.cfg.uploads_dir.join("emotes").join(f)).await;
    }
    sqlx::query("DELETE FROM emote WHERE id = ?")
        .bind(emote["id"].as_i64())
        .execute(&state.pool)
        .await?;
    Ok(reply(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_must_start_with_their_image_signature() {
        assert!(bytes_match_mime(b"\x89PNG\r\n\x1a\nrest", "image/png"));
        assert!(bytes_match_mime(b"GIF89a...", "image/gif"));
        assert!(bytes_match_mime(b"RIFF\0\0\0\0WEBPVP8 ", "image/webp"));
        assert!(bytes_match_mime(b"\xff\xd8\xff\xe0", "image/jpeg"));
        assert!(!bytes_match_mime(b"<html><script>", "image/png"));
        assert!(!bytes_match_mime(b"GIF89a", "image/png"));
        assert!(!bytes_match_mime(b"RIFF", "image/webp"));
    }
}
