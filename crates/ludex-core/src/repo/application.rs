//! Application-shaped queries.

use sqlx::SqlitePool;

use crate::application::{Application, IdentityUpdate, NewApplication, PlaybackDelta};
use crate::error::{Error, Result};
use crate::key::GameKey;
use crate::types::{GraphicsPlatform, LauncherType, ProcessArchitecture};

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

    /// Fold `src_id` into `dst_id` atomically, then delete `src_id`.
    ///
    /// Every session owned by `src_id` is re-parented to `dst_id`.
    /// Aggregate statistics are summed where sums make sense
    /// (`stat_run_count`, `stat_total_full`, `stat_total_interactive`)
    /// and combined via MAX/MIN otherwise (`stat_longest_full`,
    /// `first_seen_at`, `last_played_at`). Identity slots on `dst`
    /// are preserved; metadata slots (publisher, version,
    /// graphics/architecture, icons, paths, group) are filled from
    /// `src` only when the destination's current value is NULL or
    /// the canonical "unknown" placeholder.
    ///
    /// Idiomatic use is post-import deduplication: the Steam
    /// source detected a game as `(steam, appid)` and a migration
    /// landed the same game as `(native, exe_path)`; `merge_into`
    /// collapses the latter into the former so the dashboards see
    /// one row.
    ///
    /// Returns [`Error::Invariant`] when `src_id == dst_id` or
    /// either id does not resolve to an application row. The whole
    /// operation runs in one transaction — either every change
    /// lands or none do.
    pub async fn merge_into(&self, src_id: i64, dst_id: i64) -> Result<()> {
        if src_id == dst_id {
            return Err(Error::Invariant(
                "merge source and destination are the same application",
            ));
        }

        let mut tx = self.pool.begin().await?;

        // Fetch the source row up-front so the UPDATE against dst
        // can use plain bound parameters instead of correlated
        // subqueries. Also proves src exists before we touch
        // anything.
        let src: Application = sqlx::query_as::<_, Application>(&format!(
            "SELECT {SELECT_COLS} FROM applications WHERE id = ?"
        ))
        .bind(src_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::Invariant("merge: source application not found"))?;

        // Verify dst exists before committing. The UPDATE below
        // would no-op against a missing id without failing; we'd
        // rather surface the error than silently proceed.
        let dst_exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM applications WHERE id = ?")
            .bind(dst_id)
            .fetch_optional(&mut *tx)
            .await?;
        if dst_exists.is_none() {
            return Err(Error::Invariant("merge: destination application not found"));
        }

        // Re-parent every session owned by src. The index on
        // (application_id, started_at) keeps this fast even for
        // long histories.
        sqlx::query("UPDATE sessions SET application_id = ? WHERE application_id = ?")
            .bind(dst_id)
            .bind(src_id)
            .execute(&mut *tx)
            .await?;

        // Aggregates + timestamps. SQLite's MAX / MIN ignore NULL
        // operands, so `last_played_at = MAX(last_played_at, ?)`
        // does the right thing in every NULL-combo: both null →
        // stays null, one side null → picks the non-null, both
        // set → picks the later. No CASE needed.
        sqlx::query(
            "UPDATE applications SET \
             stat_run_count         = stat_run_count + ?, \
             stat_total_full        = stat_total_full + ?, \
             stat_total_interactive = stat_total_interactive + ?, \
             stat_longest_full      = MAX(stat_longest_full, ?), \
             first_seen_at          = MIN(first_seen_at, ?), \
             last_played_at         = MAX(last_played_at, ?) \
             WHERE id = ?",
        )
        .bind(src.stat_run_count)
        .bind(src.stat_total_full)
        .bind(src.stat_total_interactive)
        .bind(src.stat_longest_full)
        .bind(src.first_seen_at)
        .bind(src.last_played_at)
        .bind(dst_id)
        .execute(&mut *tx)
        .await?;

        // Metadata fill — each column name appears exactly once,
        // emitted only when src has a value worth copying. Built
        // with QueryBuilder so a future schema column is added in
        // a single place (a new `fill!` call) with no risk of
        // swapping positions in a 20-bind list.
        apply_metadata_fill(&mut tx, dst_id, &src).await?;

        sqlx::query("DELETE FROM applications WHERE id = ?")
            .bind(src_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }
}

/// Push one `UPDATE applications SET ... WHERE id = dst_id` carrying
/// only the COALESCE/CASE fragments for fields `src` can contribute.
/// When `src` has nothing to offer in any slot, emits nothing.
async fn apply_metadata_fill(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    dst_id: i64,
    src: &Application,
) -> Result<()> {
    let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new("UPDATE applications SET ");
    let mut sep = qb.separated(", ");
    let mut any = false;

    macro_rules! fill_nullable {
        ($column:literal, $value:expr) => {
            if let Some(v) = $value {
                sep.push(concat!($column, " = COALESCE(", $column, ", "))
                    .push_bind_unseparated(v);
                sep.push_unseparated(")");
                any = true;
            }
        };
    }

    fill_nullable!("publisher", src.publisher.clone());
    fill_nullable!("version", src.version.clone());
    fill_nullable!("executable_path", src.executable_path.clone());
    fill_nullable!("launcher_exe_path", src.launcher_exe_path.clone());
    fill_nullable!("wineprefix_path", src.wineprefix_path.clone());
    fill_nullable!("installed_flatpak_ref", src.installed_flatpak_ref.clone());
    fill_nullable!("group_id", src.group_id);
    fill_nullable!("icon_16", src.icon_16.clone());
    fill_nullable!("icon_32", src.icon_32.clone());
    fill_nullable!("icon_48", src.icon_48.clone());
    fill_nullable!("icon_256", src.icon_256.clone());

    // Enum columns: only meaningful to fill when src isn't itself
    // Unknown. `graphics_platform = 'unknown'` in SQL treats the
    // stored string literal as "unset" — matches the sqlx Type
    // serialisation for GraphicsPlatform::Unknown.
    if src.graphics_platform != GraphicsPlatform::Unknown {
        sep.push("graphics_platform = CASE WHEN graphics_platform = 'unknown' THEN ")
            .push_bind_unseparated(src.graphics_platform)
            .push_unseparated(" ELSE graphics_platform END");
        any = true;
    }
    if src.process_architecture != ProcessArchitecture::Unknown {
        sep.push("process_architecture = CASE WHEN process_architecture = 'unknown' THEN ")
            .push_bind_unseparated(src.process_architecture)
            .push_unseparated(" ELSE process_architecture END");
        any = true;
    }

    if !any {
        return Ok(());
    }
    qb.push(" WHERE id = ").push_bind(dst_id);
    qb.build().execute(&mut **tx).await?;
    Ok(())
}
