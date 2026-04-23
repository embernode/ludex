//! Integration tests for `backup::{create_snapshot, list_backups, prune_backups}`.
//!
//! Exercises the real VACUUM INTO path against a temp database so
//! we catch sqlx/sqlite behavioural regressions end-to-end, not
//! just via unit tests on the filename helpers.

use std::fs;

use ludex_core::backup::{
    create_snapshot, format_backup_filename, list_backups, prune_backups, BACKUP_FILENAME_PREFIX,
};
use ludex_core::{Database, GameKey, GraphicsPlatform, Icons, NewApplication, ProcessArchitecture};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

fn sample_new_app() -> NewApplication {
    NewApplication {
        launcher_type: ludex_core::LauncherType::Steam,
        launcher_id: "440".into(),
        product_name: "Team Fortress 2".into(),
        publisher: None,
        version: None,
        executable_path: None,
        launcher_exe_path: None,
        wineprefix_path: None,
        installed_flatpak_ref: None,
        graphics_platform: GraphicsPlatform::Unknown,
        process_architecture: ProcessArchitecture::Unknown,
        group_id: None,
        icons: Icons::default(),
        first_seen_at: OffsetDateTime::now_utc(),
    }
}

/// A snapshot of a populated live DB must be openable in its own
/// right and carry the same rows. This is the contract every other
/// piece of the backup stack relies on.
#[tokio::test]
async fn snapshot_round_trips_application_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let live_path = tmp.path().join("live.sqlite");
    let live = Database::open(&live_path).await.unwrap();
    let created = live.applications().create(sample_new_app()).await.unwrap();

    let snapshot_path = tmp.path().join("snap.sqlite");
    create_snapshot(&live, &snapshot_path).await.unwrap();
    assert!(snapshot_path.is_file());

    // Close the live DB so there's no pool holding anyone up when
    // we reopen the snapshot; not strictly required, but makes the
    // test intent clearer.
    live.close().await;

    let snap = Database::open(&snapshot_path).await.unwrap();
    let found = snap
        .applications()
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap()
        .expect("application row present in snapshot");
    assert_eq!(found.id, created.id);
    assert_eq!(found.product_name, "Team Fortress 2");
}

#[tokio::test]
async fn snapshot_refuses_to_overwrite_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let live_path = tmp.path().join("live.sqlite");
    let live = Database::open(&live_path).await.unwrap();

    let dst = tmp.path().join("snap.sqlite");
    fs::write(&dst, b"pre-existing bytes").unwrap();

    let err = create_snapshot(&live, &dst)
        .await
        .expect_err("overwrite should be refused");
    assert!(err.to_string().contains("already exists"), "got: {err}");
    // Original content untouched.
    assert_eq!(fs::read(&dst).unwrap(), b"pre-existing bytes");
}

/// Timestamped filenames sort newest-first through simple string
/// comparison. Unrelated files in the same directory are ignored.
#[test]
fn list_backups_sorts_newest_first_and_ignores_strangers() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let older = datetime!(2026-03-01 10:00:00 UTC);
    let newer = datetime!(2026-03-15 10:00:00 UTC);
    let older_name = format_backup_filename(older);
    let newer_name = format_backup_filename(newer);
    fs::write(dir.join(&older_name), b"db1").unwrap();
    fs::write(dir.join(&newer_name), b"db2").unwrap();
    // Unrelated file must not be picked up.
    fs::write(dir.join("notes.txt"), b"unrelated").unwrap();
    // Correct extension but wrong prefix.
    fs::write(dir.join("custom.sqlite"), b"also unrelated").unwrap();

    let entries = list_backups(dir).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path.file_name().unwrap(), newer_name.as_str());
    assert_eq!(entries[1].path.file_name().unwrap(), older_name.as_str());
    assert_eq!(entries[0].timestamp, Some(newer));
    assert_eq!(entries[1].timestamp, Some(older));
    assert!(entries.iter().all(|e| e
        .path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with(BACKUP_FILENAME_PREFIX)));
}

#[test]
fn list_backups_on_missing_directory_is_empty_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("does-not-exist");
    let entries = list_backups(&dir).unwrap();
    assert!(entries.is_empty());
}

/// Prune keeps the newest `keep` files and returns the removed set.
/// Already-newer files stay on disk.
#[test]
fn prune_backups_removes_oldest_beyond_retention() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let base = datetime!(2026-04-01 00:00:00 UTC);

    // Five backups, one day apart.
    for i in 0..5 {
        let name = format_backup_filename(base + Duration::days(i));
        fs::write(dir.join(&name), b"x").unwrap();
    }

    let removed = prune_backups(dir, 2).unwrap();
    assert_eq!(removed.len(), 3, "keep=2 removes the three oldest");
    let remaining: Vec<String> = list_backups(dir)
        .unwrap()
        .into_iter()
        .filter_map(|e| e.path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(remaining.len(), 2);
    // Remaining are the two newest (Apr 05 and Apr 04).
    assert!(remaining[0].contains("20260405"));
    assert!(remaining[1].contains("20260404"));
}

#[test]
fn prune_backups_is_no_op_when_under_retention() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let base = datetime!(2026-04-01 00:00:00 UTC);
    for i in 0..2 {
        let name = format_backup_filename(base + Duration::days(i));
        fs::write(dir.join(&name), b"x").unwrap();
    }
    let removed = prune_backups(dir, 5).unwrap();
    assert!(removed.is_empty());
    assert_eq!(list_backups(dir).unwrap().len(), 2);
}

/// `keep = 0` is clamped to 1 so a misconfigured retention count
/// can never wipe every backup.
#[test]
fn prune_backups_clamps_zero_retention() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let base = datetime!(2026-04-01 00:00:00 UTC);
    for i in 0..3 {
        let name = format_backup_filename(base + Duration::days(i));
        fs::write(dir.join(&name), b"x").unwrap();
    }
    let removed = prune_backups(dir, 0).unwrap();
    assert_eq!(removed.len(), 2, "retained one file; two removed");
    assert_eq!(list_backups(dir).unwrap().len(), 1);
}
