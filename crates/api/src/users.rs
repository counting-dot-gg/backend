use crate::error::ApiResult;
use crate::state::AppState;
use serde_json::json;

pub async fn add_money(state: &AppState, uuid: &str, delta: i64) -> ApiResult<()> {
    sqlx::query("UPDATE users SET money = money + ? WHERE uuid = ?")
        .bind(delta)
        .bind(uuid)
        .execute(&state.pool)
        .await?;
    state.socket.emit("updateUser", json!({ "uuid": uuid }));
    Ok(())
}
