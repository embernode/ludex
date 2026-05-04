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
async fn heroic_app_name_lookup_fills_title_publisher_and_real_exe() {
    // The daemon opens a session for a Heroic-launched game with
    // launcher_type=Heroic and launcher_id=HEROIC_APP_NAME, plus a
    // placeholder product_name (typically the X11 resource_class —
    // here `steam_app_0` because Heroic-via-Proton sets that). The
    // executable_path captured at session-start is the wine
    // preloader, not the real Windows binary. The cascade should
    // overwrite all three fields from the Heroic library cache.
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("store_cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(
        cache.join("legendary_library.json"),
        r#"{"library":[{
            "app_name": "deadbeef",
            "title": "Doors - Paradox",
            "developer": "Big Loop Studios",
            "is_installed": true,
            "install": {
                "executable": "Doors Paradox.exe",
                "install_path": "/home/u/Games/Heroic/Doors"
            }
        }]}"#,
    )
    .unwrap();

    let ctx = EnrichmentContext {
        heroic_config_dir: Some(tmp.path().to_path_buf()),
        ..Default::default()
    };
    let db = Database::open_memory().await.unwrap();
    let app = db
        .applications()
        .create(NewApplication {
            executable_path: Some(
                "/home/u/.config/heroic/tools/proton/Proton-GE/files/bin/wine64-preloader".into(),
            ),
            ..new_app(LauncherType::Heroic, "deadbeef", "steam_app_0")
        })
        .await
        .unwrap();

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.product_name, "Doors - Paradox");
    assert_eq!(after.publisher.as_deref(), Some("Big Loop Studios"));
    assert_eq!(
        after.executable_path.as_deref(),
        Some("/home/u/Games/Heroic/Doors/Doors Paradox.exe"),
    );
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

#[tokio::test]
async fn gog_info_provides_name_and_version() {
    let tmp = tempfile::tempdir().unwrap();
    let game_dir = tmp.path().join("Witcher 3");
    fs::create_dir_all(&game_dir).unwrap();
    fs::write(
        game_dir.join("goggame-1207664663.info"),
        r#"{
            "name": "The Witcher 3: Wild Hunt",
            "gameId": "1207664663",
            "rootGameId": "1207664663",
            "version": "4.04"
        }"#,
    )
    .unwrap();
    let exe_path = game_dir.join("witcher3.exe");
    fs::write(&exe_path, b"not a real PE").unwrap();

    let ctx = EnrichmentContext::default();
    let db = Database::open_memory().await.unwrap();
    let app = db
        .applications()
        .create(NewApplication {
            executable_path: Some(exe_path.display().to_string()),
            ..new_app(
                LauncherType::Native,
                &exe_path.display().to_string(),
                "witcher3",
            )
        })
        .await
        .unwrap();

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.product_name, "The Witcher 3: Wild Hunt");
    assert_eq!(after.version.as_deref(), Some("4.04"));
}

#[tokio::test]
async fn gog_info_is_found_in_parent_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let install_dir = tmp.path().join("Witcher 3");
    let nested = install_dir.join("bin").join("x64");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        install_dir.join("goggame-1207664663.info"),
        r#"{"name": "The Witcher 3", "version": "4.04"}"#,
    )
    .unwrap();
    let exe_path = nested.join("witcher3.exe");
    fs::write(&exe_path, b"not a real PE").unwrap();

    let ctx = EnrichmentContext::default();
    let db = Database::open_memory().await.unwrap();
    let app = db
        .applications()
        .create(NewApplication {
            executable_path: Some(exe_path.display().to_string()),
            ..new_app(
                LauncherType::Native,
                &exe_path.display().to_string(),
                "witcher3",
            )
        })
        .await
        .unwrap();

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.product_name, "The Witcher 3");
}

#[tokio::test]
async fn pe_enricher_skips_non_exe_executables() {
    // A native Linux ELF path should never even touch the PE parser.
    // We can verify this by pointing at /bin/ls; if the PE source were
    // called indiscriminately it would OS-read the file and produce
    // garbage results or an error. Here we just assert the enrichment
    // preserves the existing name.
    let db = Database::open_memory().await.unwrap();
    let ctx = EnrichmentContext::default();
    let app = db
        .applications()
        .create(NewApplication {
            executable_path: Some("/bin/ls".into()),
            ..new_app(LauncherType::Native, "/bin/ls", "ls")
        })
        .await
        .unwrap();

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.product_name, "ls");
    assert!(after.publisher.is_none());
    assert!(after.version.is_none());
}

#[tokio::test]
async fn pe_enricher_gracefully_handles_bogus_exe() {
    // executable_path ends in .exe but the file is garbage — must not
    // panic, must not error, must not contribute any fields.
    let tmp = tempfile::tempdir().unwrap();
    let exe_path = tmp.path().join("nonsense.exe");
    fs::write(&exe_path, b"this is not a PE").unwrap();

    let ctx = EnrichmentContext::default();
    let db = Database::open_memory().await.unwrap();
    let app = db
        .applications()
        .create(NewApplication {
            executable_path: Some(exe_path.display().to_string()),
            ..new_app(
                LauncherType::Native,
                &exe_path.display().to_string(),
                "nonsense",
            )
        })
        .await
        .unwrap();

    run_cascade(&db, &ctx, app.id).await.unwrap();

    let after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.product_name, "nonsense");
}
