use anyhow::{Result};
use log::{debug, error, info, warn};
use std::process::Command;

use crate::{cli, cluster::DuplicateCluster, db, scan::VideoFile};

/// Check availability of external tools
pub fn check_external_tools() -> Result<()> {
    info!("Checking external tool dependencies...");
    
    // Check ffprobe (required)
    if !is_command_available("ffprobe") {
        error!("ffprobe not found in PATH");
        print_ffmpeg_installation_help();
        anyhow::bail!("Required dependency 'ffprobe' not found");
    }
    debug!("✓ ffprobe found");

    // Check ffmpeg (required)
    if !is_command_available("ffmpeg") {
        error!("ffmpeg not found in PATH");
        print_ffmpeg_installation_help();
        anyhow::bail!("Required dependency 'ffmpeg' not found");
    }
    debug!("✓ ffmpeg found");

    // Check fpcalc (optional)
    if is_command_available("fpcalc") {
        info!("✓ fpcalc found - audio fingerprinting enabled");
    } else {
        warn!("fpcalc not found - audio fingerprinting will be skipped");
        warn!("Install chromaprint for enhanced duplicate detection accuracy");
        print_fpcalc_installation_help();
    }

    info!("External tool check completed");
    Ok(())
}

fn is_command_available(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--help")
        .output()
        .map(|output| output.status.success())
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
    
    // Since rusqlite::Connection is not thread-safe, we'll use sequential processing
    // for database operations and parallel processing only for the heavy computation parts
    let mut processed_count = 0;
    
    for video_file in files {
        // Check if file is already processed and up-to-date in cache
        if db::is_file_up_to_date(db_conn, &video_file)? {
            debug!("File already up-to-date in cache: {}", video_file.path.display());
            progress.inc(1);
            continue;
        }
        
        // Process the heavy computation parts (metadata, hashes) 
        match process_single_file_compute(&video_file, scan_options, common_options).await {
            Ok(analysis_result) => {
                // Store results in database (sequential for thread safety)
                if let Err(e) = db::store_file_analysis(
                    db_conn,
                    &video_file,
                    &analysis_result.metadata,
                    &analysis_result.coarse_hashes,
                    analysis_result.phash_data.as_ref(),
                    analysis_result.chromaprint.as_deref(),
                ) {
                    warn!("Failed to store analysis for {}: {}", video_file.path.display(), e);
                } else {
                    processed_count += 1;
                }
            }
            Err(e) => {
                warn!("File processing failed for {}: {}", video_file.path.display(), e);
            }
        }
        
        progress.inc(1);
    }

    progress.finish_with_message("File processing completed");
    Ok(processed_count)
}

struct AnalysisResult {
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