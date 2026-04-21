//! `ludex sessions` — print recent sessions from the database.
//!
//! Reads the same SQLite file the daemon writes to. No daemon connection
//! is needed: SQLite's WAL journaling allows another process to read
//! concurrently with the daemon's writes.

use anyhow::{Context, Result};
use ludex_core::{default_database_path, Database, RecentSession};
use time::format_description::FormatItem;
use time::macros::format_description;

const TIMESTAMP_FMT: &[FormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]");

pub(crate) async fn run(limit: u32) -> Result<()> {
    let path = default_database_path().context("neither XDG_DATA_HOME nor HOME is set")?;
    if !path.exists() {
        eprintln!(
            "no database at {} — has ludex-daemon run yet?",
            path.display()
        );
        return Ok(());
    }

    let db = Database::open(&path)
        .await
        .with_context(|| format!("open database at {}", path.display()))?;
    let rows = db.sessions().list_recent_with_app(limit).await?;
    db.close().await;

    if rows.is_empty() {
        println!("(no sessions recorded yet)");
        return Ok(());
    }
    print_table(&rows);
    Ok(())
}

fn print_table(rows: &[RecentSession]) {
    let name_width = rows
        .iter()
        .map(|r| r.product_name.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(8, 40);

    println!(
        "{:<16}  {:<name_width$}  {:<10}  {:<10}  status",
        "started (UTC)",
        "application",
        "full",
        "interactive",
        name_width = name_width,
    );
    println!(
        "{}",
        "─".repeat(16 + 2 + name_width + 2 + 10 + 2 + 10 + 2 + 20)
    );
    for row in rows {
        let started = row.started_at.format(&TIMESTAMP_FMT).unwrap_or_default();
        let name = truncate(&row.product_name, name_width);
        let full = fmt_duration(row.full_runtime_seconds);
        let inter = fmt_duration(row.interactive_runtime_seconds);
        let status = session_status(row);
        println!("{started:<16}  {name:<name_width$}  {full:<10}  {inter:<10}  {status}");
    }
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_owned()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

fn fmt_duration(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

fn session_status(row: &RecentSession) -> String {
    match (row.ended_at, row.exit_reason) {
        (Some(_), Some(reason)) => reason.to_string(),
        (None, _) => "open".into(),
        (Some(_), None) => "closed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_duration_covers_ranges() {
        assert_eq!(fmt_duration(0), "0s");
        assert_eq!(fmt_duration(45), "45s");
        assert_eq!(fmt_duration(60), "1m 0s");
        assert_eq!(fmt_duration(125), "2m 5s");
        assert_eq!(fmt_duration(3_600), "1h 0m");
        assert_eq!(fmt_duration(3_725), "1h 2m");
        assert_eq!(fmt_duration(-5), "0s");
    }

    #[test]
    fn truncate_respects_char_count_not_bytes() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
        // Multi-byte chars must not split in the middle.
        assert_eq!(truncate("日本語テスト", 4), "日本語…");
    }
}
