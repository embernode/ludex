//! Integration tests for [`Database`] connection handling.
//!
//! [`Database`]: ludex_core::Database

use std::time::Duration;

use ludex_core::Database;

/// [`Database::open_memory`] pins its pool to a single never-recycled
/// connection: the in-memory database lives only as long as at least
/// one connection is open, and on older sqlx (pre-0.8 shared-cache
/// handling) a second pooled connection was a separate, schema-less
/// database. Holding the sole connection while a second query arrives
/// exercises the pinning — the query must queue and then run against
/// the migrated schema, never against a fresh connection.
#[tokio::test]
async fn open_memory_serves_queries_from_the_migrated_connection() {
    let db = Database::open_memory().await.unwrap();

    let held = db.pool().acquire().await.unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(held);
    });

    // With the pool pinned to one connection this waits for the
    // release above rather than opening a second connection.
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM applications")
        .fetch_one(db.pool())
        .await
        .expect("query must run against the migrated connection");
    assert_eq!(row.0, 0);

    release.await.unwrap();
}
