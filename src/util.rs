use anyhow::{Result};
use log::{debug, error, info, warn};
use std::io::Write;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::{cli, cluster::DuplicateCluster, db, scan::VideoFile};

/// Check availability of external tools with timeout and progress indication
pub fn check_external_tools() -> Result<()> {
    println!("🔍 Checking external dependencies...");

    // Check required tools in parallel with timeout
    let required_tools = ["ffprobe", "ffmpeg"];
    let mut missing_tools = Vec::new();

    for tool in &required_tools {
        print!("  Checking {}... ", tool);
        std::io::stdout().flush().unwrap();

        if is_command_available_fast(tool) {
            println!("✓");
            debug!("✓ {} found", tool);
        } else {
            println!("✗");
            missing_tools.push(*tool);
        }
    }

    // If any required tools are missing, show help and exit
    if !missing_tools.is_empty() {
        error!("Required dependencies not found: {}", missing_tools.join(", "));
        print_ffmpeg_installation_help();
        anyhow::bail!("Required dependencies missing: {}", missing_tools.join(", "));
    }

    // Check optional tools
    print!("  Checking fpcalc (optional)... ");
    std::io::stdout().flush().unwrap();

    if is_command_available_fast("fpcalc") {
        println!("✓ (audio fingerprinting enabled)");
        info!("✓ fpcalc found - audio fingerprinting enabled");
    } else {
        println!("- (audio fingerprinting disabled)");
        debug!("fpcalc not found - audio fingerprinting will be skipped");
    }

    println!("✅ Dependency check completed");
    Ok(())
}

fn is_command_available(cmd: &str) -> bool {
    is_command_available_fast(cmd)
}

fn is_command_available_fast(cmd: &str) -> bool {
    // Use "which" on Unix-like systems or "where" on Windows for faster checking
    let check_cmd = if cfg!(target_os = "windows") { "where" } else { "which" };

    Command::new(check_cmd)
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn print_ffmpeg_installation_help() {
    eprintln!("\n🔧 How to install FFmpeg:");
    eprintln!("  Windows (chocolatey): choco install ffmpeg");
    eprintln!("  Windows (winget):     winget install Gyan.FFmpeg");
    eprintln!("  macOS (homebrew):     brew install ffmpeg");
    eprintln!("  Ubuntu/Debian:        sudo apt install ffmpeg");
    eprintln!("  CentOS/RHEL:          sudo yum install ffmpeg");
    eprintln!("  Arch Linux:           sudo pacman -S ffmpeg");
    eprintln!("\n  Or download from: https://ffmpeg.org/download.html");
}

fn print_fpcalc_installation_help() {
    eprintln!("\n🔧 How to install Chromaprint (for fpcalc):");
    eprintln!("  Windows (chocolatey): choco install chromaprint");
    eprintln!("  macOS (homebrew):     brew install chromaprint");
    eprintln!("  Ubuntu/Debian:        sudo apt install chromaprint-tools");
    eprintln!("  CentOS/RHEL:          sudo yum install chromaprint-tools");
    eprintln!("  Arch Linux:           sudo pacman -S chromaprint");
}

/// Process files in parallel with proper concurrency control
pub async fn process_files_parallel(
    files: Vec<VideoFile>,
    db_conn: &rusqlite::Connection,
    scan_options: &cli::ScanOptions,
    common_options: &cli::CommonOptions,
    progress: indicatif::ProgressBar,
) -> Result<usize> {
    info!("Processing {} files with {} parallel jobs", files.len(), common_options.jobs);

    let start_time = Instant::now();
    let total_files = files.len();

    // Enhanced progress tracking with ETA calculation
    let progress_tracker = Arc::new(ProgressTracker::new(total_files, progress.clone()));

    // Filter out files that are already up-to-date
    let mut files_to_process = Vec::new();
    for video_file in files {
        if db::is_file_up_to_date(db_conn, &video_file)? {
            debug!("File already up-to-date in cache: {}", video_file.path.display());
            progress_tracker.update_skipped();
        } else {
            files_to_process.push(video_file);
        }
    }

    if files_to_process.is_empty() {
        progress.finish_with_message("All files are up-to-date");
        return Ok(0);
    }

    info!("Processing {} new/changed files", files_to_process.len());

    // Use parallel processing for computation with batched database writes
    let batch_size = (common_options.jobs * 2).max(10).min(50);
    let processed_count = process_files_in_batches(
        files_to_process,
        db_conn,
        scan_options,
        common_options,
        progress_tracker,
        batch_size,
    ).await?;

    let elapsed = start_time.elapsed();
    let files_per_sec = processed_count as f64 / elapsed.as_secs_f64();
    let final_message = format!(
        "Processing completed: {} files in {:.1}s ({:.1} files/sec)",
        processed_count,
        elapsed.as_secs_f64(),
        files_per_sec
    );

    progress.finish_with_message(final_message);
    Ok(processed_count)
}

/// Enhanced progress tracking with ETA calculation
#[derive(Debug)]
struct ProgressTracker {
    total_files: usize,
    processed: Arc<Mutex<usize>>,
    skipped: Arc<Mutex<usize>>,
    start_time: Instant,
    progress_bar: indicatif::ProgressBar,
}

impl ProgressTracker {
    fn new(total_files: usize, progress_bar: indicatif::ProgressBar) -> Self {
        Self {
            total_files,
            processed: Arc::new(Mutex::new(0)),
            skipped: Arc::new(Mutex::new(0)),
            start_time: Instant::now(),
            progress_bar,
        }
    }

    fn update_processed(&self, filename: &str) {
        let mut processed = self.processed.lock().unwrap();
        *processed += 1;
        let current_processed = *processed;
        let current_skipped = *self.skipped.lock().unwrap();
        drop(processed);

        debug!("Progress update: {} processed, {} skipped, current: {}",
               current_processed, current_skipped, filename);
        self.update_progress_bar(current_processed, current_skipped, Some(filename));
    }

    fn update_skipped(&self) {
        let mut skipped = self.skipped.lock().unwrap();
        *skipped += 1;
        let current_skipped = *skipped;
        let current_processed = *self.processed.lock().unwrap();
        drop(skipped);

        self.update_progress_bar(current_processed, current_skipped, None);
    }

    fn update_progress_bar(&self, processed: usize, skipped: usize, current_file: Option<&str>) {
        let total_done = processed + skipped;
        let remaining = self.total_files.saturating_sub(total_done);

        // Calculate ETA
        let elapsed = self.start_time.elapsed();
        let files_per_sec = if elapsed.as_secs() > 0 {
            processed as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        let eta_seconds = if files_per_sec > 0.0 && remaining > 0 {
            (remaining as f64 / files_per_sec) as u64
        } else {
            0
        };

        let eta_text = if eta_seconds > 0 {
            format_duration_short(eta_seconds)
        } else {
            "calculating...".to_string()
        };

        let message = if let Some(filename) = current_file {
            format!("Processing: {} | ETA: {} | {:.1}/s",
                   filename, eta_text, files_per_sec)
        } else {
            format!("Scanning cache... | ETA: {} | {:.1}/s",
                   eta_text, files_per_sec)
        };

        self.progress_bar.set_position(total_done as u64);
        self.progress_bar.set_message(message);
    }
}

/// Process files in parallel batches with proper concurrency control
async fn process_files_in_batches(
    files: Vec<VideoFile>,
    db_conn: &rusqlite::Connection,
    scan_options: &cli::ScanOptions,
    common_options: &cli::CommonOptions,
    progress_tracker: Arc<ProgressTracker>,
    batch_size: usize,
) -> Result<usize> {
    use tokio::sync::Semaphore;
    use futures::stream::{StreamExt, FuturesUnordered};

    let mut processed_count = 0;
    let mut batch_results = Vec::new();

    // Create semaphore to limit concurrent processing
    let semaphore = Arc::new(Semaphore::new(common_options.jobs));

    // Process files in chunks to avoid overwhelming the system
    let chunk_size = (common_options.jobs * 4).max(batch_size);

    for chunk in files.chunks(chunk_size) {
        let mut futures = FuturesUnordered::new();

        // Create futures for parallel processing
        for video_file in chunk {
            let video_file = video_file.clone();
            let scan_options = scan_options.clone();
            let common_options = common_options.clone();
            let semaphore = semaphore.clone();

            let future = async move {
                let _permit = semaphore.acquire().await.unwrap();
                let filename = video_file.path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                debug!("Starting to process file: {}", filename);
                match process_single_file_compute(&video_file, &scan_options, &common_options).await {
                    Ok(result) => {
                        debug!("Completed processing file: {}", filename);
                        Some(result)
                    },
                    Err(e) => {
                        warn!("File processing failed for {}: {}", filename, e);
                        None
                    }
                }
            };

            futures.push(future);
        }

        // Collect results from parallel processing
        while let Some(result) = futures.next().await {
            if let Some(analysis_result) = result {
                // Update progress immediately when file is processed
                progress_tracker.update_processed(&analysis_result.filename);
                batch_results.push(analysis_result);

                // Store batch when it reaches optimal size
                if batch_results.len() >= batch_size {
                    processed_count += store_analysis_batch_silent(db_conn, &mut batch_results)?;
                }
            }
        }

        // Update progress after each chunk
        if !batch_results.is_empty() {
            processed_count += store_analysis_batch_silent(db_conn, &mut batch_results)?;
        }
    }

    Ok(processed_count)
}

/// Store analysis results in batch for better database performance (silent - no progress updates)
fn store_analysis_batch_silent(
    db_conn: &rusqlite::Connection,
    batch_results: &mut Vec<AnalysisResult>,
) -> Result<usize> {
    let mut stored_count = 0;

    // Use database transaction for batch operations
    let mut tx = db_conn.unchecked_transaction()?;

    for result in batch_results.drain(..) {
        match db::store_file_analysis_tx(
            &mut tx,
            &result.video_file,
            &result.metadata,
            &result.coarse_hashes,
            result.phash_data.as_ref(),
            result.chromaprint.as_deref(),
        ) {
            Ok(_) => {
                stored_count += 1;
            }
            Err(e) => {
                warn!("Failed to store analysis for {}: {}", result.filename, e);
            }
        }
    }

    tx.commit()?;
    Ok(stored_count)
}

/// Store analysis results in batch for better database performance (with progress updates)
fn store_analysis_batch(
    db_conn: &rusqlite::Connection,
    batch_results: &mut Vec<AnalysisResult>,
    progress_tracker: &ProgressTracker,
) -> Result<usize> {
    let mut stored_count = 0;

    // Use database transaction for batch operations
    let mut tx = db_conn.unchecked_transaction()?;

    for result in batch_results.drain(..) {
        match db::store_file_analysis_tx(
            &mut tx,
            &result.video_file,
            &result.metadata,
            &result.coarse_hashes,
            result.phash_data.as_ref(),
            result.chromaprint.as_deref(),
        ) {
            Ok(_) => {
                stored_count += 1;
                progress_tracker.update_processed(&result.filename);
            }
            Err(e) => {
                warn!("Failed to store analysis for {}: {}", result.filename, e);
            }
        }
    }

    tx.commit()?;
    Ok(stored_count)
}

struct AnalysisResult {
    video_file: VideoFile,
    filename: String,
    metadata: crate::db::VideoMetadata,
    coarse_hashes: crate::db::CoarseHashes,
    phash_data: Option<crate::db::PerceptualHashData>,
    chromaprint: Option<String>,
}

/// Process computation-heavy parts of file analysis
async fn process_single_file_compute(
    video_file: &VideoFile,
    scan_options: &cli::ScanOptions,
    _common_options: &cli::CommonOptions,
) -> Result<AnalysisResult> {
    debug!("Processing file: {}", video_file.path.display());
    
    // Stage 0: Extract metadata
    let metadata = crate::meta::extract_metadata(&video_file.path).await?;
    
    // Stage 1: Compute coarse hashes (head/tail)
    let coarse_hashes = crate::coarse::compute_coarse_hashes(
        &video_file.path, 
        scan_options.head_tail_mib as usize * 1024 * 1024
    ).await?;
    
    // Stage 2: Compute perceptual hashes if video is substantial enough
    let phash_data = if metadata.duration_seconds > 5.0 {
        Some(crate::phash::compute_perceptual_hashes(
            &video_file.path,
            scan_options.frame_samples,
            metadata.duration_seconds,
        ).await?)
    } else {
        debug!("Skipping pHash for short video: {}", video_file.path.display());
        None
    };
    
    // Stage 3: Compute chromaprint if available
    let chromaprint = if is_command_available("fpcalc") && metadata.has_audio {
        crate::chroma::compute_chromaprint(&video_file.path).await.ok()
    } else {
        None
    };
    
    debug!("Successfully processed: {}", video_file.path.display());
    
    Ok(AnalysisResult {
        video_file: video_file.clone(),
        filename: video_file.path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string(),
        metadata,
        coarse_hashes,
        phash_data,
        chromaprint,
    })
}

/// Display cluster information in a summary format
pub fn display_cluster_summary(cluster: &DuplicateCluster) {
    println!("  Similarity: {:.1}% confidence", cluster.confidence_score * 100.0);
    println!("  Total size: {}", crate::scan::format_size(cluster.total_size()));
    
    for (i, file_info) in cluster.files.iter().enumerate() {
        let marker = if i == 0 { "👑" } else { "  " };
        println!("  {} {} ({} | {})", 
                 marker,
                 file_info.path.display(),
                 crate::scan::format_size(file_info.size),
                 file_info.modified.format("%Y-%m-%d %H:%M")
        );
    }
    
    if let Some(best) = cluster.get_recommended_to_keep() {
        println!("  💡 Recommended to keep: {}", best.path.display());
    }
}

/// Display detailed cluster information
pub fn display_cluster_details(cluster: &DuplicateCluster) {
    println!("  Similarity: {:.1}% confidence", cluster.confidence_score * 100.0);
    println!("  Detection method: {}", cluster.detection_method);
    println!("  Total size: {}", crate::scan::format_size(cluster.total_size()));
    
    // Create table headers
    println!("\n  {:4} | {:60} | {:10} | {:12} | {:16} | {:19}", 
             "#", "Path", "Size", "Resolution", "Codec", "Modified");
    println!("  {:-<4}-+-{:-<60}-+-{:-<10}-+-{:-<12}-+-{:-<16}-+-{:-<19}", 
             "", "", "", "", "", "");
    
    for (i, file_info) in cluster.files.iter().enumerate() {
        let marker = if i == 0 { "👑" } else { &format!("{:2}", i + 1) };
        let size_str = humansize::format_size(file_info.size, humansize::BINARY);
        let resolution = format!("{}x{}", 
                                file_info.width.unwrap_or(0), 
                                file_info.height.unwrap_or(0));
        let codec = file_info.video_codec.as_deref().unwrap_or("unknown");
        let modified = file_info.modified.format("%Y-%m-%d %H:%M");
        
        // Truncate path if too long
        let path_display = if file_info.path.to_string_lossy().len() > 58 {
            format!("...{}", 
                   &file_info.path.to_string_lossy()[file_info.path.to_string_lossy().len() - 55..])
        } else {
            file_info.path.to_string_lossy().to_string()
        };
        
        println!("  {:4} | {:60} | {:>10} | {:>12} | {:16} | {}", 
                 marker, path_display, size_str, resolution, 
                 &codec[..codec.len().min(16)], modified);
    }
    
    if let Some(best) = cluster.get_recommended_to_keep() {
        println!("\n  💡 Recommended to keep: {}", best.path.display());
        println!("     Reason: Highest quality (resolution → bitrate → codec → date)");
    }
}

/// Create a standardized error for missing external tools
pub fn external_tool_error(tool: &str, purpose: &str) -> anyhow::Error {
    anyhow::anyhow!("External tool '{}' required for {} is not available", tool, purpose)
}

/// Convert file path to a safe string for database storage
pub fn path_to_string(path: &std::path::Path) -> String {
    dunce::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// Cross-platform path handling
pub fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    dunce::simplified(path).to_path_buf()
}

/// Format duration in human-readable format
pub fn format_duration(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;
    
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

/// Calculate percentage difference between two numbers
pub fn percentage_difference(a: f64, b: f64) -> f64 {
    if a == 0.0 && b == 0.0 {
        0.0
    } else {
        ((a - b).abs() / ((a + b) / 2.0)) * 100.0
    }
}

/// Format duration in short human-readable format for ETA
pub fn format_duration_short(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}h{}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m{}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(65.0), "1:05");
        assert_eq!(format_duration(3661.0), "1:01:01");
        assert_eq!(format_duration(30.5), "0:30");
    }

    #[test]
    fn test_percentage_difference() {
        assert_eq!(percentage_difference(100.0, 90.0), (10.0 / 95.0) * 100.0);
        assert_eq!(percentage_difference(0.0, 0.0), 0.0);
        assert!((percentage_difference(50.0, 60.0) - (10.0 / 55.0) * 100.0).abs() < 0.001);
    }

    #[test] 
    fn test_path_normalization() {
        let path = std::path::Path::new("./test/../video.mp4");
        let normalized = normalize_path(path);
        assert_eq!(normalized, std::path::Path::new("video.mp4"));
    }
}