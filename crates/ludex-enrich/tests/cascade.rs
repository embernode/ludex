//! End-to-end tests for the enrichment cascade.
//!
//! Use an in-memory database and a fixture-backed [`EnrichmentContext`]
//! so the tests hit the real code paths (SQL, I/O, parsing) without
//! depending on anything on the machine running them.

use std::fs;
use std::path::PathBuf;

use ludex_core::{
    Database, GraphicsPlatform, Icons, LauncherType, NewApplication, ProcessArchitecture,
};
use ludex_enrich::{run_cascade, EnrichmentContext};
use tempfile::TempDir;
use time::OffsetDateTime;

const TF2_ACF: &str = include_str!("fixtures/steam_appmanifest_440.acf");

fn new_app(launcher_type: LauncherType, launcher_id: &str, product_name: &str) -> NewApplication {
    NewApplication {
        launcher_type,
        launcher_id: launcher_id.into(),
        product_name: product_name.into(),
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

fn make_steam_fixture() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(steamapps.join("appmanifest_440.acf"), TF2_ACF).unwrap();
    let steam_dir = tmp.path().to_path_buf();
    (tmp, steam_dir)
}

#[tokio::test]
async fn steam_acf_overrides_placeholder_name() {
    let (_tmp, steam_dir) = make_steam_fixture();
    let ctx = EnrichmentContext {
        steam_dir: Some(steam_dir),
        ..Default::default()
    };
    let db = Database::open_memory().await.unwrap();

    // Daemon normally writes a placeholder like "AppID 440" on first
    // detection when it hasn't consulted the .acf yet.
    let app = db
        .applications()
        .create(new_app(LauncherType::Steam, "440", "AppID 440"))
        .await
        .unwrap();
    assert_eq!(app.product_name, "AppID 440");

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.product_name, "Team Fortress 2");
}

#[tokio::test]
async fn missing_acf_does_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("steamapps")).unwrap();
    let ctx = EnrichmentContext {
        steam_dir: Some(tmp.path().to_path_buf()),
        ..Default::default()
    };
    let db = Database::open_memory().await.unwrap();
    let app = db
        .applications()
        .create(new_app(LauncherType::Steam, "999999", "AppID 999999"))
        .await
        .unwrap();

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    // Missing acf: no enricher contributes; original placeholder stands.
    assert_eq!(after.product_name, "AppID 999999");
}

#[tokio::test]
async fn flatpak_desktop_file_provides_name() {
    let tmp = tempfile::tempdir().unwrap();
    let apps_dir = tmp.path().join("applications");
    fs::create_dir_all(&apps_dir).unwrap();
    fs::write(
        apps_dir.join("com.example.Widget.desktop"),
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Widget\n\
         Exec=example-widget %U\n\
         Icon=com.example.Widget\n",
    )
    .unwrap();

    let ctx = EnrichmentContext {
        desktop_dirs: vec![apps_dir],
        ..Default::default()
    };
    let db = Database::open_memory().await.unwrap();
    let app = db
        .applications()
        .create(new_app(
            LauncherType::Flatpak,
            "com.example.Widget",
            "com.example.Widget",
        ))
        .await
        .unwrap();

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.product_name, "Widget");
}

#[tokio::test]
async fn unknown_app_id_is_safe_no_op() {
    let db = Database::open_memory().await.unwrap();
    let ctx = EnrichmentContext::default();
    run_cascade(&db, &ctx, 9_999_999).await.unwrap();
}

#[tokio::test]
async fn cascade_preserves_existing_fields_when_sources_silent() {
    let db = Database::open_memory().await.unwrap();
    let ctx = EnrichmentContext::default();
    let app = db
        .applications()
        .create(NewApplication {
            publisher: Some("existing pub".into()),
            ..new_app(LauncherType::Native, "/opt/games/foo", "Foo")
        })
        .await
        .unwrap();

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.publisher.as_deref(), Some("existing pub"));
    assert_eq!(after.product_name, "Foo");
}
