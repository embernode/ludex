//! Integration tests for [`BlockedRepo`].
//!
//! [`BlockedRepo`]: ludex_core::repo::BlockedRepo

use ludex_core::{Database, GameKey};
use time::OffsetDateTime;

#[tokio::test]
async fn blocked_repo_round_trips_keys() {
    let db = Database::open_memory().await.unwrap();
    let repo = db.blocked();
    let key = GameKey::steam("440");

    assert!(!repo.contains(&key).await.unwrap());
    assert!(repo.list().await.unwrap().is_empty());

    let inserted = repo.insert(&key, OffsetDateTime::now_utc()).await.unwrap();
    assert!(inserted, "first insert returns true");
    assert!(repo.contains(&key).await.unwrap());

    // Second insert of the same key is a no-op (INSERT OR IGNORE).
    let second = repo.insert(&key, OffsetDateTime::now_utc()).await.unwrap();
    assert!(!second, "duplicate insert returns false");
    assert_eq!(repo.list().await.unwrap().len(), 1);

    let removed = repo.remove(&key).await.unwrap();
    assert!(removed);
    assert!(!repo.contains(&key).await.unwrap());

    // Removing an absent key is a no-op.
    assert!(!repo.remove(&key).await.unwrap());
}

#[tokio::test]
async fn blocked_repo_lists_every_launcher_type() {
    let db = Database::open_memory().await.unwrap();
    let repo = db.blocked();
    let now = OffsetDateTime::now_utc();

    for key in [
        GameKey::steam("440"),
        GameKey::lutris("celeste"),
        GameKey::heroic("com.example.fooo"),
        GameKey::native("/opt/games/foo/foo"),
    ] {
        repo.insert(&key, now).await.unwrap();
    }

    let set = repo.list().await.unwrap();
    assert_eq!(set.len(), 4);
    assert!(set.contains(&GameKey::steam("440")));
    assert!(set.contains(&GameKey::native("/opt/games/foo/foo")));
}
