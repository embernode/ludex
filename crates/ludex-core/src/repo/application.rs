//! Application-shaped queries.

use sqlx::SqlitePool;

use crate::application::{Application, IdentityUpdate, NewApplication, PlaybackDelta};
use crate::error::Result;
use crate::key::GameKey;
use crate::types::LauncherType;

const SELECT_COLS: &str = "id, launcher_type, launcher_id, product_name, publisher, version, \
    executable_path, launcher_exe_path, wineprefix_path, installed_flatpak_ref, \
    graphics_platform, process_architecture, group_id, \
    icon_16, icon_32, icon_48, icon_256, \
    first_seen_at, last_played_at, \
    stat_run_count, stat_total_full, stat_total_interactive, stat_longest_full";

/// Typed access to the `applications` table.
pub struct ApplicationRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ApplicationRepo<'a> {
    /// Create a new repository bound to the given pool.
    #[must_use]
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a new application row and return the resulting [`Application`].
    ///
    /// Uniqueness on `(launcher_type, launcher_id)` is enforced by the
    /// schema. Call [`Self::find_by_key`] first if the caller needs to
    /// tolerate existing rows.
    pub async fn create(&self, new: NewApplication) -> Result<Application> {
        let NewApplication {
            launcher_type,
            launcher_id,
            product_name,
            publisher,
            version,
            executable_path,
            launcher_exe_path,
            wineprefix_path,
            installed_flatpak_ref,
            graphics_platform,
            process_architecture,
            group_id,
            icons,
            first_seen_at,
        } = new;

        let sql = "INSERT INTO applications (\
                launcher_type, launcher_id, product_name, publisher, version, \
                executable_path, launcher_exe_path, wineprefix_path, installed_flatpak_ref, \
                graphics_platform, process_architecture, group_id, \
                icon_16, icon_32, icon_48, icon_256, \
                first_seen_at\
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING "
            .to_string()
            + SELECT_COLS;

        sqlx::query_as::<_, Application>(&sql)
            .bind(launcher_type)
            .bind(launcher_id)
            .bind(product_name)
            .bind(publisher)
            .bind(version)
            .bind(executable_path)
            .bind(launcher_exe_path)
            .bind(wineprefix_path)
            .bind(installed_flatpak_ref)
            .bind(graphics_platform)
            .bind(process_architecture)
            .bind(group_id)
            .bind(icons.icon_16)
            .bind(icons.icon_32)
            .bind(icons.icon_48)
            .bind(icons.icon_256)
            .bind(first_seen_at)
            .fetch_one(self.pool)
            .await
            .map_err(Into::into)
    }

    /// Return the application with the given primary key, if any.
    pub async fn find_by_id(&self, id: i64) -> Result<Option<Application>> {
        let sql = format!("SELECT {SELECT_COLS} FROM applications WHERE id = ?");
        sqlx::query_as::<_, Application>(&sql)
            .bind(id)
            .fetch_optional(self.pool)
            .await
            .map_err(Into::into)
    }

    /// Return the application with the given launcher key, if any.
    pub async fn find_by_key(&self, key: &GameKey) -> Result<Option<Application>> {
        self.find_by_launcher(key.launcher_type, &key.launcher_id)
            .await
    }

    /// Return the application with the given launcher type + id, if any.
    pub async fn find_by_launcher(
        &self,
        launcher_type: LauncherType,
        launcher_id: &str,
    ) -> Result<Option<Application>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM applications \
             WHERE launcher_type = ? AND launcher_id = ?"
        );
        sqlx::query_as::<_, Application>(&sql)
            .bind(launcher_type)
            .bind(launcher_id)
            .fetch_optional(self.pool)
            .await
            .map_err(Into::into)
    }

    /// List all applications, most-recently-played first. Applications
    /// that have never been played sort last.
    pub async fn list(&self) -> Result<Vec<Application>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM applications \
             ORDER BY last_played_at DESC NULLS LAST, product_name ASC"
        );
        sqlx::query_as::<_, Application>(&sql)
            .fetch_all(self.pool)
            .await
            .map_err(Into::into)
    }

    /// Apply an enrichment patch. Fields present as `Some` overwrite the
    /// existing column value; fields that are `None` are left unchanged.
    /// The icon fields inside `update.icons` follow the same rule
    /// individually.
    ///
    /// Emits a single dynamic `UPDATE` statement; if the patch is empty,
    /// does nothing.
    pub async fn update_identity(&self, id: i64, update: IdentityUpdate) -> Result<()> {
        let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new("UPDATE applications SET ");
        let mut separated = builder.separated(", ");
        let mut fields = 0_u32;

        if let Some(v) = update.product_name {
            separated.push("product_name = ").push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.publisher {
            separated.push("publisher = ").push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.version {
            separated.push("version = ").push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.executable_path {
            separated
                .push("executable_path = ")
                .push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.launcher_exe_path {
            separated
                .push("launcher_exe_path = ")
                .push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.wineprefix_path {
            separated
                .push("wineprefix_path = ")
                .push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.installed_flatpak_ref {
            separated
                .push("installed_flatpak_ref = ")
                .push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.graphics_platform {
            separated
                .push("graphics_platform = ")
                .push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.process_architecture {
            separated
                .push("process_architecture = ")
                .push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.group_id {
            separated.push("group_id = ").push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.icons.icon_16 {
            separated.push("icon_16 = ").push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.icons.icon_32 {
            separated.push("icon_32 = ").push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.icons.icon_48 {
            separated.push("icon_48 = ").push_bind_unseparated(v);
            fields += 1;
        }
        if let Some(v) = update.icons.icon_256 {
            separated.push("icon_256 = ").push_bind_unseparated(v);
            fields += 1;
        }

        if fields == 0 {
            return Ok(());
        }

        builder.push(" WHERE id = ").push_bind(id);
        builder.build().execute(self.pool).await?;
        Ok(())
    }

    /// Apply a session-close delta to the application's aggregate
    /// statistics atomically.
    pub async fn apply_playback(&self, id: i64, delta: PlaybackDelta) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "UPDATE applications \
             SET stat_run_count         = stat_run_count + 1, \
                 stat_total_full        = stat_total_full + ?, \
                 stat_total_interactive = stat_total_interactive + ?, \
                 last_played_at         = ? \
             WHERE id = ?",
        )
        .bind(delta.full_runtime_seconds)
        .bind(delta.interactive_runtime_seconds)
        .bind(delta.last_played_at)
        .bind(id)
        .execute(&mut *tx)
        .await?;

        if let Some(candidate) = delta.longest_full_candidate {
            sqlx::query(
                "UPDATE applications \
                 SET stat_longest_full = MAX(stat_longest_full, ?) \
                 WHERE id = ?",
            )
            .bind(candidate)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Delete an application and all dependent rows (sessions cascade).
    pub async fn delete(&self, id: i64) -> Result<bool> {
        let rows = sqlx::query("DELETE FROM applications WHERE id = ?")
            .bind(id)
            .execute(self.pool)
            .await?
            .rows_affected();
        Ok(rows > 0)
    }
}
