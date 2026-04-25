//! Integration tests for [`SettingsRepo`] — the typed key/value
//! store the daemon uses to persist user-tunable knobs.
//!
//! [`SettingsRepo`]: ludex_core::repo::SettingsRepo

use ludex_core::repo::GPU_MEMORY_THRESHOLD_BYTES;
use ludex_core::Database;

#[tokio::test]
async fn settings_get_u64_returns_fallback_when_row_absent() {
    let db = Database::open_memory().await.unwrap();
    let v = db
        .settings()
        .get_u64(GPU_MEMORY_THRESHOLD_BYTES, 123)
        .await
        .unwrap();
    assert_eq!(v, 123);
}

#[tokio::test]
async fn settings_set_then_get_round_trips() {
    let db = Database::open_memory().await.unwrap();
    db.settings()
        .set_u64(GPU_MEMORY_THRESHOLD_BYTES, 10_000_000)
        .await
        .unwrap();
    let v = db
        .settings()
        .get_u64(GPU_MEMORY_THRESHOLD_BYTES, 0)
        .await
        .unwrap();
    assert_eq!(v, 10_000_000);
}

#[tokio::test]
async fn settings_set_is_upsert() {
    let db = Database::open_memory().await.unwrap();
    let s = db.settings();
    s.set_u64(GPU_MEMORY_THRESHOLD_BYTES, 1).await.unwrap();
    s.set_u64(GPU_MEMORY_THRESHOLD_BYTES, 2).await.unwrap();
    assert_eq!(s.get_u64(GPU_MEMORY_THRESHOLD_BYTES, 99).await.unwrap(), 2);
}

#[tokio::test]
async fn settings_remove_returns_false_when_absent() {
    let db = Database::open_memory().await.unwrap();
    assert!(!db.settings().remove("nope").await.unwrap());
}

#[tokio::test]
async fn settings_set_rejects_empty_value() {
    let db = Database::open_memory().await.unwrap();
    let err = db
        .settings()
        .set_raw(GPU_MEMORY_THRESHOLD_BYTES, "")
        .await
        .expect_err("empty value should be rejected");
    assert!(err.to_string().contains("empty"), "got: {err}");
}

#[tokio::test]
async fn settings_get_u64_rejects_non_numeric() {
    let db = Database::open_memory().await.unwrap();
    db.settings()
        .set_raw(GPU_MEMORY_THRESHOLD_BYTES, "not-a-number")
        .await
        .unwrap();
    let err = db
        .settings()
        .get_u64(GPU_MEMORY_THRESHOLD_BYTES, 0)
        .await
        .expect_err("unparseable value should surface");
    assert!(err.to_string().contains("u64"), "got: {err}");
}
