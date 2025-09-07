use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use log::{debug, info, warn};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::scan::VideoFile;

/// Database schema version for migrations
const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMetadata {
    pub duration_seconds: f64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub bitrate: Option<u64>,
    pub has_audio: bool,
    pub format_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CoarseHashes {
    pub head_hash: Vec<u8>,
    pub tail_hash: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptualHashData {
    pub average_hash: Vec<u8>,
    pub individual_hashes: Vec<Vec<u8>>,
    pub frame_count: u32,
}

#[derive(Debug)]
pub struct FileRecord {
    pub id: i64,
    pub path: PathBuf,
    pub size: u64,
    pub modified: DateTime<Utc>,
    pub metadata: VideoMetadata,
    pub coarse_hashes: Option<CoarseHashes>,
    pub phash_data: Option<PerceptualHashData>,
    pub chromaprint: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Open or create the database with proper configuration
pub async fn open_database(db_path: &Path) -> Result<Connection> {
    info!("Opening database: {}", db_path.display());
    
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;
    
    // Configure database for performance and reliability
    configure_database(&conn)?;
    
    // Initialize schema
    initialize_schema(&conn)?;
    
    debug!("Database opened and configured successfully");
    Ok(conn)
}

fn configure_database(conn: &Connection) -> Result<()> {
    // Enable WAL mode for better concurrency
    conn.execute_batch("
        PRAGMA journal_mode = WAL;
        PRAGMA cache_size = -64000;
        PRAGMA foreign_keys = ON;
        PRAGMA synchronous = NORMAL;
        PRAGMA temp_store = MEMORY;
        PRAGMA mmap_size = 268435456;
    ")?;
    
    debug!("Database configured with performance optimizations");
    Ok(())
}

fn initialize_schema(conn: &Connection) -> Result<()> {
    // Check current schema version
    let version: i32 = conn
        .prepare("PRAGMA user_version")?
        .query_row([], |row| row.get(0))
        .unwrap_or(0);
    
    if version < SCHEMA_VERSION {
        info!("Initializing database schema (version {} -> {})", version, SCHEMA_VERSION);
        create_tables(conn)?;
        create_indices(conn)?;
        
        // Update schema version
        conn.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION))?;
        info!("Database schema initialized successfully");
    } else {
        debug!("Database schema is up-to-date (version {})", version);
    }
    
    Ok(())
}

fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT UNIQUE NOT NULL,
            size INTEGER NOT NULL,
            modified INTEGER NOT NULL,
            
            -- Video metadata
            duration_seconds REAL,
            width INTEGER,
            height INTEGER,
            video_codec TEXT,
            audio_codec TEXT,
            bitrate INTEGER,
            has_audio BOOLEAN,
            format_name TEXT,
            
            -- Coarse hashes (head/tail xxhash)
            head_hash BLOB,
            tail_hash BLOB,
            
            -- Perceptual hash data (JSON serialized)
            phash_average BLOB,
            phash_individual TEXT, -- JSON array of base64 hashes
            phash_frame_count INTEGER,
            
            -- Audio fingerprint
            chromaprint TEXT,
            
            -- Metadata
            updated_at INTEGER NOT NULL,
            created_at INTEGER DEFAULT (strftime('%s', 'now'))
        );
        
        CREATE TABLE IF NOT EXISTS analysis_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL,
            analysis_type TEXT NOT NULL, -- 'metadata', 'coarse', 'phash', 'chromaprint'
            duration_ms INTEGER,
            success BOOLEAN NOT NULL,
            error_message TEXT,
            timestamp INTEGER DEFAULT (strftime('%s', 'now')),
            FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
        );
        
        CREATE TABLE IF NOT EXISTS duplicate_clusters (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            cluster_hash TEXT UNIQUE NOT NULL, -- Hash of sorted file IDs
            detection_method TEXT NOT NULL,
            confidence_score REAL NOT NULL,
            file_ids TEXT NOT NULL, -- JSON array of file IDs
            total_size INTEGER NOT NULL,
            created_at INTEGER DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER DEFAULT (strftime('%s', 'now'))
        );
    "#)?;
    
    debug!("Database tables created");
    Ok(())
}

fn create_indices(conn: &Connection) -> Result<()> {
    let indices = [
        "CREATE INDEX IF NOT EXISTS idx_files_path ON files(path)",
        "CREATE INDEX IF NOT EXISTS idx_files_size ON files(size)",
        "CREATE INDEX IF NOT EXISTS idx_files_modified ON files(modified)",
        "CREATE INDEX IF NOT EXISTS idx_files_duration ON files(duration_seconds)",
        "CREATE INDEX IF NOT EXISTS idx_files_resolution ON files(width, height)",
        "CREATE INDEX IF NOT EXISTS idx_files_updated_at ON files(updated_at)",
        "CREATE INDEX IF NOT EXISTS idx_files_head_hash ON files(head_hash)",
        "CREATE INDEX IF NOT EXISTS idx_files_tail_hash ON files(tail_hash)",
        "CREATE INDEX IF NOT EXISTS idx_files_chromaprint ON files(chromaprint)",
        "CREATE INDEX IF NOT EXISTS idx_analysis_log_file_id ON analysis_log(file_id)",
        "CREATE INDEX IF NOT EXISTS idx_analysis_log_type ON analysis_log(analysis_type)",
        "CREATE INDEX IF NOT EXISTS idx_duplicate_clusters_hash ON duplicate_clusters(cluster_hash)",
    ];
    
    let all_indices = indices.join(";\n");
    conn.execute_batch(&all_indices)?;
    
    debug!("Database indices created");
    Ok(())
}

/// Check if a file is already processed and up-to-date in the cache
pub fn is_file_up_to_date(conn: &Connection, video_file: &VideoFile) -> Result<bool> {
    let path_str = crate::util::path_to_string(&video_file.path);
    
    let result: Option<(i64, i64)> = conn
        .query_row(
            "SELECT size, modified FROM files WHERE path = ? AND updated_at > ?",
            params![
                path_str,
                video_file.modified.timestamp() - 1 // 1 second tolerance
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    
    match result {
        Some((cached_size, cached_modified)) => {
            let is_current = cached_size == video_file.size as i64 
                && cached_modified == video_file.modified.timestamp();
            
            if is_current {
                debug!("File is up-to-date in cache: {}", video_file.path.display());
            } else {
                debug!("File has changed since cache: {}", video_file.path.display());
            }
            
            Ok(is_current)
        }
        None => {
            debug!("File not found in cache: {}", video_file.path.display());
            Ok(false)
        }
    }
}

/// Store complete analysis results for a file
pub fn store_file_analysis(
    conn: &Connection,
    video_file: &VideoFile,
    metadata: &VideoMetadata,
    coarse_hashes: &CoarseHashes,
    phash_data: Option<&PerceptualHashData>,
    chromaprint: Option<&str>,
) -> Result<i64> {
    let path_str = crate::util::path_to_string(&video_file.path);
    let now = Utc::now().timestamp();
    
    // Serialize phash data
    let (phash_avg, phash_individual, phash_frame_count) = match phash_data {
        Some(data) => (
            Some(data.average_hash.clone()),
            Some(serde_json::to_string(&data.individual_hashes)?),
            Some(data.frame_count as i32),
        ),
        None => (None, None, None),
    };
    
    let tx = conn.unchecked_transaction()?;
    
    // Insert or replace file record
    tx.execute(
        r#"
        INSERT OR REPLACE INTO files (
            path, size, modified, duration_seconds, width, height,
            video_codec, audio_codec, bitrate, has_audio, format_name,
            head_hash, tail_hash, phash_average, phash_individual, phash_frame_count,
            chromaprint, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18
        )
        "#,
        params![
            path_str,
            video_file.size as i64,
            video_file.modified.timestamp(),
            metadata.duration_seconds,
            metadata.width.map(|w| w as i32),
            metadata.height.map(|h| h as i32),
            metadata.video_codec,
            metadata.audio_codec,
            metadata.bitrate.map(|b| b as i64),
            metadata.has_audio,
            metadata.format_name,
            coarse_hashes.head_hash,
            coarse_hashes.tail_hash,
            phash_avg,
            phash_individual,
            phash_frame_count,
            chromaprint,
            now,
        ],
    )?;
    
    let file_id = tx.last_insert_rowid();
    
    // Log successful analysis
    tx.execute(
        "INSERT INTO analysis_log (file_id, analysis_type, success) VALUES (?1, 'complete', 1)",
        params![file_id],
    )?;
    
    tx.commit()?;
    
    debug!("Stored analysis for file ID {}: {}", file_id, video_file.path.display());
    Ok(file_id)
}

/// Retrieve all files for duplicate detection
pub fn get_all_files(conn: &Connection) -> Result<Vec<FileRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT 
            id, path, size, modified, duration_seconds, width, height,
            video_codec, audio_codec, bitrate, has_audio, format_name,
            head_hash, tail_hash, phash_average, phash_individual, phash_frame_count,
            chromaprint, updated_at
        FROM files 
        ORDER BY path
        "#,
    )?;
    
    let file_iter = stmt.query_map([], |row| {
        Ok(FileRecord {
            id: row.get(0)?,
            path: PathBuf::from(row.get::<_, String>(1)?),
            size: row.get::<_, i64>(2)? as u64,
            modified: DateTime::from_timestamp(row.get::<_, i64>(3)?, 0).unwrap_or_default(),
            metadata: VideoMetadata {
                duration_seconds: row.get(4)?,
                width: row.get::<_, Option<i32>>(5)?.map(|w| w as u32),
                height: row.get::<_, Option<i32>>(6)?.map(|h| h as u32),
                video_codec: row.get(7)?,
                audio_codec: row.get(8)?,
                bitrate: row.get::<_, Option<i64>>(9)?.map(|b| b as u64),
                has_audio: row.get(10)?,
                format_name: row.get(11)?,
            },
            coarse_hashes: match (row.get::<_, Option<Vec<u8>>>(12)?, row.get::<_, Option<Vec<u8>>>(13)?) {
                (Some(head), Some(tail)) => Some(CoarseHashes { head_hash: head, tail_hash: tail }),
                _ => None,
            },
            phash_data: match (
                row.get::<_, Option<Vec<u8>>>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<i32>>(16)?,
            ) {
                (Some(avg), Some(individual_json), Some(count)) => {
                    let individual_hashes: Vec<Vec<u8>> = serde_json::from_str(&individual_json)
                        .unwrap_or_default();
                    Some(PerceptualHashData {
                        average_hash: avg,
                        individual_hashes,
                        frame_count: count as u32,
                    })
                }
                _ => None,
            },
            chromaprint: row.get(17)?,
            updated_at: DateTime::from_timestamp(row.get::<_, i64>(18)?, 0).unwrap_or_default(),
        })
    })?;
    
    let files: Result<Vec<_>, _> = file_iter.collect();
    let files = files?;
    
    info!("Retrieved {} files from database", files.len());
    Ok(files)
}

/// Remove files that no longer exist on disk
pub async fn cleanup_missing_files(conn: &Connection) -> Result<usize> {
    info!("Cleaning up missing files from database...");
    
    let files = get_all_files(conn)?;
    let mut removed_count = 0;
    
    let tx = conn.unchecked_transaction()?;
    
    for file in files {
        if !file.path.exists() {
            tx.execute("DELETE FROM files WHERE id = ?", params![file.id])?;
            removed_count += 1;
            debug!("Removed missing file from database: {}", file.path.display());
        }
    }
    
    tx.commit()?;
    
    if removed_count > 0 {
        info!("Cleaned up {} missing files from database", removed_count);
    }
    
    Ok(removed_count)
}

/// Clear the entire cache database
pub async fn clear_cache(db_path: &Path) -> Result<()> {
    info!("Clearing cache database: {}", db_path.display());
    
    if db_path.exists() {
        std::fs::remove_file(db_path)
            .with_context(|| format!("Failed to remove database file: {}", db_path.display()))?;
        info!("Cache database cleared successfully");
    } else {
        info!("Cache database file does not exist, nothing to clear");
    }
    
    Ok(())
}

/// Get database statistics
pub fn get_database_stats(conn: &Connection) -> Result<DatabaseStats> {
    let total_files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
    
    let files_with_phash: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE phash_average IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    
    let files_with_chromaprint: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE chromaprint IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    
    let total_size: Option<i64> = conn
        .query_row("SELECT SUM(size) FROM files", [], |row| row.get(0))
        .optional()?
        .flatten();
    
    let db_file_size = conn.path()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    
    Ok(DatabaseStats {
        total_files: total_files as usize,
        files_with_phash: files_with_phash as usize,
        files_with_chromaprint: files_with_chromaprint as usize,
        total_content_size: total_size.unwrap_or(0) as u64,
        database_file_size: db_file_size,
    })
}

#[derive(Debug)]
pub struct DatabaseStats {
    pub total_files: usize,
    pub files_with_phash: usize,
    pub files_with_chromaprint: usize,
    pub total_content_size: u64,
    pub database_file_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_database_creation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        
        let conn = open_database(&db_path).await?;
        
        // Verify schema was created
        let table_count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
            [],
            |row| row.get(0),
        )?;
        
        assert!(table_count >= 3); // files, analysis_log, duplicate_clusters
        
        Ok(())
    }

    #[test]
    fn test_file_freshness_check() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        initialize_schema(&conn)?;
        
        let video_file = VideoFile {
            path: PathBuf::from("/test/video.mp4"),
            size: 1000,
            modified: Utc::now(),
            is_accessible: true,
        };
        
        // File not in database yet
        assert!(!is_file_up_to_date(&conn, &video_file)?);
        
        Ok(())
    }
}