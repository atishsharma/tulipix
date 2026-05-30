//! `np.p4.sync.shared` — shared albums + members.
//!
//! An owner creates a shared album and adds members with a role (editor /
//! viewer). This owns creation, membership add/remove with role changes, and
//! the permission check the UI gates edit actions on.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn create(pool: &SqlitePool, name: &str, owner: &str) -> Result<i64> {
    Ok(sqlx::query_scalar("INSERT INTO shared_albums (name, owner, created) VALUES (?,?,?) RETURNING id")
        .bind(name).bind(owner).bind(now()).fetch_one(pool).await?)
}

/// Add or update a member's role.
pub async fn set_member(pool: &SqlitePool, album_id: i64, member: &str, role: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO shared_album_members (album_id, member, role) VALUES (?,?,?)
         ON CONFLICT(album_id, member) DO UPDATE SET role = excluded.role",
    ).bind(album_id).bind(member).bind(role).execute(pool).await?;
    Ok(())
}

pub async fn remove_member(pool: &SqlitePool, album_id: i64, member: &str) -> Result<()> {
    sqlx::query("DELETE FROM shared_album_members WHERE album_id = ? AND member = ?")
        .bind(album_id).bind(member).execute(pool).await?;
    Ok(())
}

pub async fn members(pool: &SqlitePool, album_id: i64) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as("SELECT member, role FROM shared_album_members WHERE album_id = ? ORDER BY member")
        .bind(album_id).fetch_all(pool).await?)
}

/// Can this user edit the album? Owner always; members with the editor role.
pub async fn can_edit(pool: &SqlitePool, album_id: i64, user: &str) -> Result<bool> {
    let owner: Option<String> = sqlx::query_scalar("SELECT owner FROM shared_albums WHERE id = ?")
        .bind(album_id).fetch_optional(pool).await?;
    if owner.as_deref() == Some(user) { return Ok(true); }
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM shared_album_members WHERE album_id = ? AND member = ?")
        .bind(album_id).bind(user).fetch_optional(pool).await?;
    Ok(role.as_deref() == Some("editor"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn membership_and_permissions() {
        let (_t, pool) = open_pool().await;
        let id = create(&pool, "Trip", "owner@x").await.unwrap();
        set_member(&pool, id, "ed@x", "editor").await.unwrap();
        set_member(&pool, id, "vi@x", "viewer").await.unwrap();
        assert_eq!(members(&pool, id).await.unwrap().len(), 2);
        assert!(can_edit(&pool, id, "owner@x").await.unwrap());
        assert!(can_edit(&pool, id, "ed@x").await.unwrap());
        assert!(!can_edit(&pool, id, "vi@x").await.unwrap());
        // demote editor → loses edit
        set_member(&pool, id, "ed@x", "viewer").await.unwrap();
        assert!(!can_edit(&pool, id, "ed@x").await.unwrap());
        remove_member(&pool, id, "vi@x").await.unwrap();
        assert_eq!(members(&pool, id).await.unwrap().len(), 1);
    }
}
