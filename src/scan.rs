use anyhow::{Context, Result};
use ignore::WalkBuilder;
use log::{debug, info, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ScanConfig {
    pub extensions: Vec<String>,
    pub include_hidden: bool,
    pub exclude_patterns: Vec<String>,
    pub min_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: chrono::DateTime<chrono::Utc>,
    pub is_accessible: bool,
}

impl VideoFile {
    pub fn from_path(path: PathBuf) -> Result<Self> {
        let metadata = fs::metadata(&path)
            .with_context(|| format!("Failed to read metadata for {}", path.display()))?;
        
        let modified = metadata
            .modified()
            .context("Failed to get file modification time")?
            .into();

        Ok(Self {
            path,
            size: metadata.len(),
            modified,
            is_accessible: true,
        })
    }

    pub fn size_mb(&self) -> f64 {
        self.size as f64 / (1024.0 * 1024.0)
    }

    pub fn extension(&self) -> Option<String> {
        self.path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!(".{}", ext.to_lowercase()))
    }
}

/// Discover video files in the given directory according to the scan configuration
pub fn discover_video_files(root: &Path, config: &ScanConfig) -> Result<Vec<VideoFile>> {
    info!("Scanning directory: {}", root.display());
    debug!("Scan config: {:?}", config);

    if !root.exists() {
        anyhow::bail!("Directory does not exist: {}", root.display());
    }

    if !root.is_dir() {
        anyhow::bail!("Path is not a directory: {}", root.display());
    }

    // Compile exclude patterns
    let exclude_regexes = compile_exclude_patterns(&config.exclude_patterns)?;

    // Build ignore-aware walker
    let walker = WalkBuilder::new(root)
        .hidden(!config.include_hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .require_git(false)
        .follow_links(false)
        .build();

    let extensions_set: HashSet<String> = config.extensions.iter().cloned().collect();
    let mut video_files = Vec::new();
    let mut skipped_count = 0;

    info!("Walking directory tree...");

    for result in walker {
        match result {
            Ok(entry) => {
                let path = entry.path();
                
                // Skip directories
                if path.is_dir() {
                    continue;
                }

                // Check extension
                let extension = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| format!(".{}", ext.to_lowercase()));
                
                match extension {
                    Some(ext) if extensions_set.contains(&ext) => {
                        // This is a video file with matching extension
                    }
                    _ => {
                        // Not a video file or wrong extension
                        continue;
                    }
                }

                // Apply exclude patterns
                if is_excluded(path, &exclude_regexes) {
                    debug!("Excluding file due to pattern: {}", path.display());
                    skipped_count += 1;
                    continue;
                }

                // Try to create VideoFile
                match VideoFile::from_path(path.to_path_buf()) {
                    Ok(video_file) => {
                        // Check minimum size
                        if video_file.size < config.min_size_bytes {
                            debug!(
                                "Skipping small file: {} ({:.1} MB < {:.1} MB minimum)",
                                path.display(),
                                video_file.size_mb(),
                                config.min_size_bytes as f64 / (1024.0 * 1024.0)
                            );
                            skipped_count += 1;
                            continue;
                        }

                        debug!("Found video file: {} ({:.1} MB)", 
                               video_file.path.display(), 
                               video_file.size_mb());
                        video_files.push(video_file);
                    }
                    Err(e) => {
                        warn!("Failed to process file {}: {}", path.display(), e);
                        skipped_count += 1;
                    }
                }
            }
            Err(e) => {
                warn!("Error walking directory: {}", e);
            }
        }
    }

    if skipped_count > 0 {
        info!("Skipped {} files (excluded/too small/inaccessible)", skipped_count);
    }

    info!("Found {} video files matching criteria", video_files.len());

    // Sort by path for consistent ordering
    video_files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(video_files)
}

fn compile_exclude_patterns(patterns: &[String]) -> Result<Vec<Regex>> {
    let mut regexes = Vec::new();
    
    for pattern in patterns {
        let regex = Regex::new(pattern)
            .with_context(|| format!("Invalid exclude regex pattern: {}", pattern))?;
        regexes.push(regex);
    }
    
    Ok(regexes)
}

fn is_excluded(path: &Path, exclude_regexes: &[Regex]) -> bool {
    let path_str = path.to_string_lossy();
    
    for regex in exclude_regexes {
        if regex.is_match(&path_str) {
            return true;
        }
    }
    
    false
}

/// Get human-readable size string
pub fn format_size(bytes: u64) -> String {
    humansize::format_size(bytes, humansize::DECIMAL)
}

/// Check if a file still exists and is accessible
pub fn verify_file_access(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_scan_config() {
        let config = ScanConfig {
            extensions: vec![".mp4".to_string(), ".mkv".to_string()],
            include_hidden: false,
            exclude_patterns: vec![r".*test.*".to_string()],
            min_size_bytes: 1024 * 1024, // 1MB
        };

        assert_eq!(config.extensions.len(), 2);
        assert!(!config.include_hidden);
        assert_eq!(config.min_size_bytes, 1024 * 1024);
    }

    #[test]
    fn test_video_file_creation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let test_file = temp_dir.path().join("test.mp4");
        
        let mut file = File::create(&test_file)?;
        file.write_all(b"test video content")?;
        file.sync_all()?;

        let video_file = VideoFile::from_path(test_file)?;
        
        assert!(video_file.size > 0);
        assert!(video_file.is_accessible);
        assert_eq!(video_file.extension(), Some(".mp4".to_string()));
        
        Ok(())
    }

    #[test]
    fn test_exclude_patterns() -> Result<()> {
        let patterns = vec![".*test.*".to_string(), r"temp_.*\.mp4".to_string()];
        let regexes = compile_exclude_patterns(&patterns)?;
        
        assert!(is_excluded(Path::new("/path/test_video.mp4"), &regexes));
        assert!(is_excluded(Path::new("/path/temp_backup.mp4"), &regexes));
        assert!(!is_excluded(Path::new("/path/normal_video.mp4"), &regexes));
        
        Ok(())
    }

    #[test]
    fn test_size_formatting() {
        assert_eq!(format_size(1024), "1.02 KB");
        assert_eq!(format_size(1024 * 1024), "1.05 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.07 GB");
    }
}