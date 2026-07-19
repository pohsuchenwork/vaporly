use anyhow::{anyhow, Result};
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_specta::Event;

/// Database migrations for transcription history.
/// Each migration is applied in order. The library tracks which migrations
/// have been applied using SQLite's user_version pragma.
///
/// Note: For users upgrading from tauri-plugin-sql, migrate_from_tauri_plugin_sql()
/// converts the old _sqlx_migrations table tracking to the user_version pragma,
/// ensuring migrations don't re-run on existing databases.
static MIGRATIONS: &[M] = &[
    M::up(
        "CREATE TABLE IF NOT EXISTS transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL
        );",
    ),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_processed_text TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_prompt TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_requested BOOLEAN NOT NULL DEFAULT 0;"),
    // Vaporly: which app the dictation was inserted into (context-awareness)
    M::up("ALTER TABLE transcription_history ADD COLUMN app_name TEXT;"),
    // F4 auto-learn (RepeatedWords): per-word repeat counter. `word` is the
    // lowercased core, `display` the latest as-dictated casing; timestamps
    // are epoch seconds. Rows are deleted when the word is learned.
    M::up(
        "CREATE TABLE word_candidates (
            word TEXT PRIMARY KEY,
            display TEXT NOT NULL,
            count INTEGER NOT NULL,
            first_seen INTEGER NOT NULL,
            last_seen INTEGER NOT NULL
        );",
    ),
];

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct PaginatedHistory {
    pub entries: Vec<HistoryEntry>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(tag = "action")]
pub enum HistoryUpdatePayload {
    #[serde(rename = "added")]
    Added { entry: HistoryEntry },
    #[serde(rename = "updated")]
    Updated { entry: HistoryEntry },
    #[serde(rename = "deleted")]
    Deleted { id: i64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct HistoryEntry {
    pub id: i64,
    pub file_name: String,
    pub timestamp: i64,
    pub saved: bool,
    pub title: String,
    pub transcription_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_requested: bool,
    /// Vaporly: the app the dictation targeted (frontmost at transcription time).
    pub app_name: Option<String>,
}

pub struct HistoryManager {
    app_handle: AppHandle,
    recordings_dir: PathBuf,
    db_path: PathBuf,
}

impl HistoryManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        // Create recordings directory in app data dir
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let recordings_dir = app_data_dir.join("recordings");
        let db_path = app_data_dir.join("history.db");

        // Ensure recordings directory exists
        if !recordings_dir.exists() {
            fs::create_dir_all(&recordings_dir)?;
            debug!("Created recordings directory: {:?}", recordings_dir);
        }

        let manager = Self {
            app_handle: app_handle.clone(),
            recordings_dir,
            db_path,
        };

        // Initialize database and run migrations synchronously
        manager.init_database()?;

        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        info!("Initializing database at {:?}", self.db_path);

        let mut conn = Connection::open(&self.db_path)?;

        // Handle migration from tauri-plugin-sql to rusqlite_migration
        // tauri-plugin-sql used _sqlx_migrations table, rusqlite_migration uses user_version pragma
        self.migrate_from_tauri_plugin_sql(&conn)?;

        // Create migrations object and run to latest version
        let migrations = Migrations::new(MIGRATIONS.to_vec());

        // Validate migrations in debug builds
        #[cfg(debug_assertions)]
        migrations.validate().expect("Invalid migrations");

        // Get current version before migration
        let version_before: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        debug!("Database version before migration: {}", version_before);

        // Apply any pending migrations
        migrations.to_latest(&mut conn)?;

        // Get version after migration
        let version_after: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if version_after > version_before {
            info!(
                "Database migrated from version {} to {}",
                version_before, version_after
            );
        } else {
            debug!("Database already at latest version {}", version_after);
        }

        Ok(())
    }

    /// Migrate from tauri-plugin-sql's migration tracking to rusqlite_migration's.
    /// tauri-plugin-sql used a _sqlx_migrations table, while rusqlite_migration uses
    /// SQLite's user_version pragma. This function checks if the old system was in use
    /// and sets the user_version accordingly so migrations don't re-run.
    fn migrate_from_tauri_plugin_sql(&self, conn: &Connection) -> Result<()> {
        // Check if the old _sqlx_migrations table exists
        let has_sqlx_migrations: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !has_sqlx_migrations {
            return Ok(());
        }

        // Check current user_version
        let current_version: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if current_version > 0 {
            // Already migrated to rusqlite_migration system
            return Ok(());
        }

        // Get the highest version from the old migrations table
        let old_version: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if old_version > 0 {
            info!(
                "Migrating from tauri-plugin-sql (version {}) to rusqlite_migration",
                old_version
            );

            // Set user_version to match the old migration state
            conn.pragma_update(None, "user_version", old_version)?;

            // Optionally drop the old migrations table (keeping it doesn't hurt)
            // conn.execute("DROP TABLE IF EXISTS _sqlx_migrations", [])?;

            info!(
                "Migration tracking converted: user_version set to {}",
                old_version
            );
        }

        Ok(())
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    fn map_history_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
        Ok(HistoryEntry {
            id: row.get("id")?,
            file_name: row.get("file_name")?,
            timestamp: row.get("timestamp")?,
            saved: row.get("saved")?,
            title: row.get("title")?,
            transcription_text: row.get("transcription_text")?,
            post_processed_text: row.get("post_processed_text")?,
            post_process_prompt: row.get("post_process_prompt")?,
            post_process_requested: row.get("post_process_requested")?,
            app_name: row.get("app_name")?,
        })
    }

    pub fn recordings_dir(&self) -> &std::path::Path {
        &self.recordings_dir
    }

    /// Save a new history entry to the database.
    /// The WAV file should already have been written to the recordings directory.
    pub fn save_entry(
        &self,
        file_name: String,
        transcription_text: String,
        post_process_requested: bool,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
        app_name: Option<String>,
    ) -> Result<HistoryEntry> {
        let timestamp = Utc::now().timestamp();
        let title = self.format_timestamp_title(timestamp);

        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO transcription_history (
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested,
                app_name
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &file_name,
                timestamp,
                false,
                &title,
                &transcription_text,
                &post_processed_text,
                &post_process_prompt,
                post_process_requested,
                &app_name,
            ],
        )?;

        let entry = HistoryEntry {
            id: conn.last_insert_rowid(),
            file_name,
            timestamp,
            saved: false,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
            app_name,
        };

        debug!("Saved history entry with id {}", entry.id);

        self.cleanup_old_entries()?;

        // Emit typed event for real-time frontend updates
        if let Err(e) = (HistoryUpdatePayload::Added {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    /// Update an existing history entry with new transcription results (used by retry).
    pub fn update_transcription(
        &self,
        id: i64,
        transcription_text: String,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history
             SET transcription_text = ?1,
                 post_processed_text = ?2,
                 post_process_prompt = ?3
             WHERE id = ?4",
            params![
                transcription_text,
                post_processed_text,
                post_process_prompt,
                id
            ],
        )?;

        if updated == 0 {
            return Err(anyhow!("History entry {} not found", id));
        }

        let entry = conn
            .query_row(
                "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, app_name
                 FROM transcription_history WHERE id = ?1",
                params![id],
                Self::map_history_entry,
            )?;

        debug!("Updated transcription for history entry {}", id);

        if let Err(e) = (HistoryUpdatePayload::Updated {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    /// Edit an entry's displayed text (v2 history editing). History shows the
    /// post-processed text when present, so that is what an edit replaces;
    /// otherwise the raw transcription is replaced. The `saved` column is
    /// legacy and untouched.
    pub fn update_entry_text(&self, id: i64, text: String) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let entry = Self::update_entry_text_with_conn(&conn, id, &text)?;

        debug!("Edited text of history entry {}", id);

        if let Err(e) = (HistoryUpdatePayload::Updated {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    fn update_entry_text_with_conn(conn: &Connection, id: i64, text: &str) -> Result<HistoryEntry> {
        let has_processed: bool = conn
            .query_row(
                "SELECT post_processed_text IS NOT NULL FROM transcription_history WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| anyhow!("History entry {} not found", id))?;

        let sql = if has_processed {
            "UPDATE transcription_history SET post_processed_text = ?1 WHERE id = ?2"
        } else {
            "UPDATE transcription_history SET transcription_text = ?1 WHERE id = ?2"
        };
        conn.execute(sql, params![text, id])?;

        let entry = conn.query_row(
            "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, app_name
             FROM transcription_history WHERE id = ?1",
            params![id],
            Self::map_history_entry,
        )?;
        Ok(entry)
    }

    /// F4 RepeatedWords: fold one dictation's final text into the
    /// `word_candidates` counter and return the words that just crossed the
    /// repeat threshold (their rows are deleted; the caller feeds them to
    /// `auto_learn::learn_words`). `now` is injected (epoch seconds) so tests
    /// control the clock; `custom_words` filters out already-known words
    /// before they are ever counted.
    pub fn observe_dictation_words(
        &self,
        final_text: &str,
        now: i64,
        custom_words: &[String],
    ) -> Result<Vec<String>> {
        let conn = self.get_connection()?;
        Self::observe_dictation_words_with_conn(&conn, final_text, now, custom_words)
    }

    fn observe_dictation_words_with_conn(
        conn: &Connection,
        final_text: &str,
        now: i64,
        custom_words: &[String],
    ) -> Result<Vec<String>> {
        use crate::auto_learn::{REPEAT_N, WINDOW_SECS};

        let mut learned = Vec::new();
        for display in crate::auto_learn::learnable_unknowns(final_text, custom_words) {
            let key = display.to_lowercase();
            let existing: Option<(i64, i64, i64)> = conn
                .query_row(
                    "SELECT count, first_seen, last_seen FROM word_candidates WHERE word = ?1",
                    params![key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let (count, first_seen) = match existing {
                None => (1, now),
                // Window expired since the last sighting: fresh count.
                Some((_, _, last_seen)) if now - last_seen > WINDOW_SECS => (1, now),
                Some((prev_count, prev_first, _)) => {
                    if prev_count + 1 >= REPEAT_N && now - prev_first > WINDOW_SECS {
                        // Enough repeats but stretched past one window (for
                        // example every 10 days): re-anchor the window here
                        // instead of letting first_seen age forever, which
                        // would leave a row that can never fire.
                        (1, now)
                    } else {
                        (prev_count + 1, prev_first)
                    }
                }
            };
            conn.execute(
                "INSERT INTO word_candidates (word, display, count, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(word) DO UPDATE SET
                    display = excluded.display,
                    count = excluded.count,
                    first_seen = excluded.first_seen,
                    last_seen = excluded.last_seen",
                params![key, display, count, first_seen, now],
            )?;
            if count >= REPEAT_N && now - first_seen <= WINDOW_SECS {
                conn.execute("DELETE FROM word_candidates WHERE word = ?1", params![key])?;
                learned.push(display);
            }
        }
        Ok(learned)
    }

    pub fn cleanup_old_entries(&self) -> Result<()> {
        let retention_period = crate::settings::get_recording_retention_period(&self.app_handle);

        match retention_period {
            crate::settings::RecordingRetentionPeriod::Never => {
                // Don't delete anything
                Ok(())
            }
            crate::settings::RecordingRetentionPeriod::PreserveLimit => {
                // Use the old count-based logic with history_limit
                let limit = crate::settings::get_history_limit(&self.app_handle);
                self.cleanup_by_count(limit)
            }
            _ => {
                // Use time-based logic
                self.cleanup_by_time(retention_period)
            }
        }
    }

    fn delete_entries_and_files(&self, entries: &[(i64, String)]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        let conn = self.get_connection()?;
        let mut deleted_count = 0;

        for (id, file_name) in entries {
            // Delete database entry
            conn.execute(
                "DELETE FROM transcription_history WHERE id = ?1",
                params![id],
            )?;

            // Delete WAV file
            let file_path = self.recordings_dir.join(file_name);
            if file_path.exists() {
                if let Err(e) = fs::remove_file(&file_path) {
                    error!("Failed to delete WAV file {}: {}", file_name, e);
                } else {
                    debug!("Deleted old WAV file: {}", file_name);
                    deleted_count += 1;
                }
            }
        }

        Ok(deleted_count)
    }

    fn cleanup_by_count(&self, limit: usize) -> Result<()> {
        let conn = self.get_connection()?;

        // All entries, newest first. The legacy `saved` column no longer
        // exempts entries from cleanup (v2 has no star/saved feature).
        let mut stmt = conn
            .prepare("SELECT id, file_name FROM transcription_history ORDER BY timestamp DESC")?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>("id")?, row.get::<_, String>("file_name")?))
        })?;

        let mut entries: Vec<(i64, String)> = Vec::new();
        for row in rows {
            entries.push(row?);
        }

        if entries.len() > limit {
            let entries_to_delete = &entries[limit..];
            let deleted_count = self.delete_entries_and_files(entries_to_delete)?;

            if deleted_count > 0 {
                debug!("Cleaned up {} old history entries by count", deleted_count);
            }
        }

        Ok(())
    }

    fn cleanup_by_time(
        &self,
        retention_period: crate::settings::RecordingRetentionPeriod,
    ) -> Result<()> {
        let conn = self.get_connection()?;

        // Calculate cutoff timestamp (current time minus retention period)
        let now = Utc::now().timestamp();
        let cutoff_timestamp = match retention_period {
            crate::settings::RecordingRetentionPeriod::Days3 => now - (3 * 24 * 60 * 60), // 3 days in seconds
            crate::settings::RecordingRetentionPeriod::Weeks2 => now - (2 * 7 * 24 * 60 * 60), // 2 weeks in seconds
            crate::settings::RecordingRetentionPeriod::Months3 => now - (3 * 30 * 24 * 60 * 60), // 3 months in seconds (approximate)
            _ => unreachable!("Should not reach here"),
        };

        // All entries older than the cutoff timestamp (legacy `saved` ignored)
        let mut stmt =
            conn.prepare("SELECT id, file_name FROM transcription_history WHERE timestamp < ?1")?;

        let rows = stmt.query_map(params![cutoff_timestamp], |row| {
            Ok((row.get::<_, i64>("id")?, row.get::<_, String>("file_name")?))
        })?;

        let mut entries_to_delete: Vec<(i64, String)> = Vec::new();
        for row in rows {
            entries_to_delete.push(row?);
        }

        let deleted_count = self.delete_entries_and_files(&entries_to_delete)?;

        if deleted_count > 0 {
            debug!(
                "Cleaned up {} old history entries based on retention period",
                deleted_count
            );
        }

        Ok(())
    }

    pub async fn get_history_entries(
        &self,
        cursor: Option<i64>,
        limit: Option<usize>,
    ) -> Result<PaginatedHistory> {
        let conn = self.get_connection()?;
        let limit = limit.map(|l| l.min(100));

        let mut entries: Vec<HistoryEntry> = match (cursor, limit) {
            (Some(cursor_id), Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, app_name
                     FROM transcription_history
                     WHERE id < ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )?;
                let result = stmt
                    .query_map(params![cursor_id, fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (None, Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, app_name
                     FROM transcription_history
                     ORDER BY id DESC
                     LIMIT ?1",
                )?;
                let result = stmt
                    .query_map(params![fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (_, None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, app_name
                     FROM transcription_history
                     ORDER BY id DESC",
                )?;
                let result = stmt
                    .query_map([], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
        };

        let has_more = limit.is_some_and(|lim| entries.len() > lim);
        if has_more {
            entries.pop();
        }

        Ok(PaginatedHistory { entries, has_more })
    }

    #[cfg(test)]
    fn get_latest_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested,
                app_name
             FROM transcription_history
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;

        let entry = stmt.query_row([], Self::map_history_entry).optional()?;
        Ok(entry)
    }

    /// Get the latest entry with non-empty transcription text.
    pub fn get_latest_completed_entry(&self) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        Self::get_latest_completed_entry_with_conn(&conn)
    }

    fn get_latest_completed_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested,
                app_name
             FROM transcription_history
             WHERE transcription_text != ''
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;

        let entry = stmt.query_row([], Self::map_history_entry).optional()?;
        Ok(entry)
    }

    /// Search transcriptions, formatted output, and target app name. Newest
    /// first; LIKE with escaped wildcards so user input is matched literally.
    pub fn search_entries(&self, query: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
        let conn = self.get_connection()?;
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{}%", escaped);
        let mut stmt = conn.prepare(
            "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, app_name
             FROM transcription_history
             WHERE transcription_text LIKE ?1 ESCAPE '\\'
                OR post_processed_text LIKE ?1 ESCAPE '\\'
                OR app_name LIKE ?1 ESCAPE '\\'
             ORDER BY id DESC
             LIMIT ?2",
        )?;
        let entries = stmt
            .query_map(params![pattern, limit as i64], Self::map_history_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn get_audio_file_path(&self, file_name: &str) -> PathBuf {
        // Defense in depth: resolve ONLY the final path component inside the
        // recordings dir, so a crafted name (separators or `..`) cannot escape
        // it.
        self.recordings_dir.join(sanitize_recording_name(file_name))
    }

    pub async fn get_entry_by_id(&self, id: i64) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested,
                app_name
             FROM transcription_history
             WHERE id = ?1",
        )?;

        let entry = stmt.query_row([id], Self::map_history_entry).optional()?;

        Ok(entry)
    }

    pub async fn delete_entry(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;

        // Get the entry to find the file name
        if let Some(entry) = self.get_entry_by_id(id).await? {
            // Delete the audio file first
            let file_path = self.get_audio_file_path(&entry.file_name);
            if file_path.exists() {
                if let Err(e) = fs::remove_file(&file_path) {
                    error!("Failed to delete audio file {}: {}", entry.file_name, e);
                    // Continue with database deletion even if file deletion fails
                }
            }
        }

        // Delete from database
        conn.execute(
            "DELETE FROM transcription_history WHERE id = ?1",
            params![id],
        )?;

        debug!("Deleted history entry with id: {}", id);

        // Emit history updated event
        if let Err(e) = (HistoryUpdatePayload::Deleted { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(())
    }

    fn format_timestamp_title(&self, timestamp: i64) -> String {
        if let Some(utc_datetime) = DateTime::from_timestamp(timestamp, 0) {
            // Convert UTC to local timezone
            let local_datetime = utc_datetime.with_timezone(&Local);
            local_datetime.format("%B %e, %Y - %l:%M%p").to_string()
        } else {
            format!("Recording {}", timestamp)
        }
    }
}

/// Reduce a caller-supplied recording name to its final path component so it
/// can only ever resolve inside the recordings dir. `Path::file_name` drops any
/// directory parts and yields None for "." / "..", which we map to an empty
/// (non-file) name.
fn sanitize_recording_name(file_name: &str) -> &std::ffi::OsStr {
    std::path::Path::new(file_name)
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    #[test]
    fn sanitize_recording_name_blocks_traversal() {
        use std::ffi::OsStr;
        // A plain name passes through unchanged.
        assert_eq!(sanitize_recording_name("clip.wav"), OsStr::new("clip.wav"));
        // Separators and parent refs are stripped to the final component.
        assert_eq!(
            sanitize_recording_name("../secret.wav"),
            OsStr::new("secret.wav")
        );
        assert_eq!(
            sanitize_recording_name("../../etc/passwd"),
            OsStr::new("passwd")
        );
        assert_eq!(sanitize_recording_name("/etc/passwd"), OsStr::new("passwd"));
        // A bare ".." has no file component: maps to empty (not a file).
        assert_eq!(sanitize_recording_name(".."), OsStr::new(""));
    }

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_name TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                saved BOOLEAN NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                transcription_text TEXT NOT NULL,
                post_processed_text TEXT,
                post_process_prompt TEXT,
                post_process_requested BOOLEAN NOT NULL DEFAULT 0,
                app_name TEXT
            );",
        )
        .expect("create transcription_history table");
        conn
    }

    fn insert_entry(conn: &Connection, timestamp: i64, text: &str, post_processed: Option<&str>) {
        conn.execute(
            "INSERT INTO transcription_history (
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                format!("vaporly-{}.wav", timestamp),
                timestamp,
                false,
                format!("Recording {}", timestamp),
                text,
                post_processed,
                Option::<String>::None,
                false,
            ],
        )
        .expect("insert history entry");
    }

    #[test]
    fn get_latest_entry_returns_none_when_empty() {
        let conn = setup_conn();
        let entry = HistoryManager::get_latest_entry_with_conn(&conn).expect("fetch latest entry");
        assert!(entry.is_none());
    }

    #[test]
    fn get_latest_entry_returns_newest_entry() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "first", None);
        insert_entry(&conn, 200, "second", Some("processed"));

        let entry = HistoryManager::get_latest_entry_with_conn(&conn)
            .expect("fetch latest entry")
            .expect("entry exists");

        assert_eq!(entry.timestamp, 200);
        assert_eq!(entry.transcription_text, "second");
        assert_eq!(entry.post_processed_text.as_deref(), Some("processed"));
    }

    #[test]
    fn get_latest_completed_entry_skips_empty_entries() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "completed", None);
        insert_entry(&conn, 200, "", None);

        let entry = HistoryManager::get_latest_completed_entry_with_conn(&conn)
            .expect("fetch latest completed entry")
            .expect("completed entry exists");

        assert_eq!(entry.timestamp, 100);
        assert_eq!(entry.transcription_text, "completed");
    }

    #[test]
    fn edit_replaces_the_displayed_text() {
        let conn = setup_conn();
        // Raw-only entry: the raw transcription IS the displayed text.
        insert_entry(&conn, 100, "raw text", None);
        let entry = HistoryManager::update_entry_text_with_conn(&conn, 1, "edited raw")
            .expect("edit raw entry");
        assert_eq!(entry.transcription_text, "edited raw");
        assert_eq!(entry.post_processed_text, None);

        // Processed entry: history shows the processed text, so the edit
        // lands there and the raw original is preserved.
        insert_entry(&conn, 200, "raw two", Some("processed two"));
        let entry = HistoryManager::update_entry_text_with_conn(&conn, 2, "edited processed")
            .expect("edit processed entry");
        assert_eq!(entry.transcription_text, "raw two");
        assert_eq!(
            entry.post_processed_text.as_deref(),
            Some("edited processed")
        );
    }

    #[test]
    fn edit_of_missing_entry_errors() {
        let conn = setup_conn();
        assert!(HistoryManager::update_entry_text_with_conn(&conn, 42, "x").is_err());
    }

    // ---- F4 RepeatedWords (word_candidates) ----

    const DAY: i64 = 24 * 60 * 60;

    /// Real migration chain on an in-memory database: proves every migration
    /// (including the F4 `word_candidates` one) applies cleanly in order.
    fn migrated_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        Migrations::new(MIGRATIONS.to_vec())
            .to_latest(&mut conn)
            .expect("apply all migrations");
        conn
    }

    fn observe(conn: &Connection, text: &str, day: i64, known: &[String]) -> Vec<String> {
        HistoryManager::observe_dictation_words_with_conn(conn, text, day * DAY, known)
            .expect("observe dictation words")
    }

    fn candidate_row(conn: &Connection, word: &str) -> Option<(String, i64, i64, i64)> {
        conn.query_row(
            "SELECT display, count, first_seen, last_seen FROM word_candidates WHERE word = ?1",
            params![word],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .expect("query candidate row")
    }

    #[test]
    fn migrations_apply_cleanly_and_create_word_candidates() {
        let conn = migrated_conn();
        conn.execute(
            "INSERT INTO word_candidates (word, display, count, first_seen, last_seen)
             VALUES ('zzyzx', 'Zzyzx', 1, 0, 0)",
            [],
        )
        .expect("word_candidates accepts rows");
        // The history table from the earlier migrations is intact too.
        conn.execute(
            "INSERT INTO transcription_history (file_name, timestamp, title, transcription_text)
             VALUES ('a.wav', 1, 't', 'text')",
            [],
        )
        .expect("transcription_history accepts rows");
    }

    #[test]
    fn repeated_word_fires_at_three_within_the_window() {
        let conn = migrated_conn();
        assert_eq!(
            observe(&conn, "deploy Zzyzx now", 0, &[]),
            Vec::<String>::new()
        );
        assert_eq!(observe(&conn, "Zzyzx again", 1, &[]), Vec::<String>::new());
        assert_eq!(
            observe(&conn, "and Zzyzx once more", 2, &[]),
            vec!["Zzyzx".to_string()]
        );
        // The learned row is deleted.
        assert_eq!(candidate_row(&conn, "zzyzx"), None);
    }

    #[test]
    fn repeats_within_one_dictation_count_once() {
        let conn = migrated_conn();
        assert_eq!(
            observe(&conn, "Zzyzx zzyzx ZZYZX zzyzx", 0, &[]),
            Vec::<String>::new()
        );
        let (_, count, _, _) = candidate_row(&conn, "zzyzx").expect("row exists");
        assert_eq!(count, 1, "one dictation adds at most one count");
    }

    #[test]
    fn window_expiry_resets_the_count() {
        let conn = migrated_conn();
        observe(&conn, "Zzyzx", 0, &[]);
        observe(&conn, "Zzyzx", 1, &[]);
        // 20 days of silence: past the 14-day window, the count restarts.
        assert_eq!(observe(&conn, "Zzyzx", 21, &[]), Vec::<String>::new());
        let (_, count, first_seen, _) = candidate_row(&conn, "zzyzx").expect("row exists");
        assert_eq!((count, first_seen), (1, 21 * DAY));
        // A fresh burst then fires normally.
        observe(&conn, "Zzyzx", 22, &[]);
        assert_eq!(observe(&conn, "Zzyzx", 23, &[]), vec!["Zzyzx".to_string()]);
    }

    #[test]
    fn slow_drip_reanchors_instead_of_poisoning_the_row() {
        let conn = migrated_conn();
        // Every 10 days: each sighting is inside the last_seen window, but by
        // the third the FIRST sighting is 20 days old. Not "3 in 14 days", so
        // no learn; the row re-anchors so a later burst can still fire.
        observe(&conn, "Zzyzx", 0, &[]);
        observe(&conn, "Zzyzx", 10, &[]);
        assert_eq!(observe(&conn, "Zzyzx", 20, &[]), Vec::<String>::new());
        let (_, count, first_seen, _) = candidate_row(&conn, "zzyzx").expect("row exists");
        assert_eq!((count, first_seen), (1, 20 * DAY), "window re-anchored");
        observe(&conn, "Zzyzx", 21, &[]);
        assert_eq!(observe(&conn, "Zzyzx", 22, &[]), vec!["Zzyzx".to_string()]);
    }

    #[test]
    fn known_and_common_words_are_never_counted() {
        let conn = migrated_conn();
        let known = vec!["Claude".to_string()];
        observe(&conn, "Claude helps with the deploy today", 0, &known);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM word_candidates", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "known and common words leave no candidate rows");
    }

    #[test]
    fn display_keeps_the_latest_casing() {
        let conn = migrated_conn();
        observe(&conn, "ZZYZX", 0, &[]);
        observe(&conn, "zzyzx", 1, &[]);
        assert_eq!(observe(&conn, "Zzyzx", 2, &[]), vec!["Zzyzx".to_string()]);
    }
}
