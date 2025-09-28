use anyhow::{Context, Result};
use img_hash::{HasherConfig, image::{DynamicImage as ImgHashDynamicImage}};
use log::{debug, warn};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

use crate::db::PerceptualHashData;
use crate::util::GpuAccelConfig;

/// Extract frames from video and compute perceptual hashes
pub async fn compute_perceptual_hashes(
    video_path: &Path,
    frame_count: u32,
    duration_seconds: f64,
) -> Result<PerceptualHashData> {
    compute_perceptual_hashes_with_gpu(video_path, frame_count, duration_seconds, None).await
}

/// Extract frames from video and compute perceptual hashes with GPU acceleration
pub async fn compute_perceptual_hashes_with_gpu(
    video_path: &Path,
    frame_count: u32,
    duration_seconds: f64,
    gpu_config: Option<&GpuAccelConfig>,
) -> Result<PerceptualHashData> {
    debug!("Computing perceptual hashes for {} frames from: {}", frame_count, video_path.display());

    // Extract frames at evenly spaced intervals
    let frame_hashes = extract_and_hash_frames(video_path, frame_count, duration_seconds, gpu_config).await?;
    
    if frame_hashes.is_empty() {
        anyhow::bail!("No frames could be extracted from video: {}", video_path.display());
    }
    
    // Compute average hash from all frames
    let average_hash = compute_average_hash(&frame_hashes)?;
    
    debug!("Computed {} perceptual hashes", frame_hashes.len());
    
    let frame_count = frame_hashes.len() as u32;
    
    Ok(PerceptualHashData {
        average_hash,
        individual_hashes: frame_hashes,
        frame_count,
    })
}

/// Extract frames from video at specified intervals and compute their perceptual hashes
async fn extract_and_hash_frames(
    video_path: &Path,
    target_frame_count: u32,
    duration_seconds: f64,
    gpu_config: Option<&GpuAccelConfig>,
) -> Result<Vec<Vec<u8>>> {
    // Calculate frame extraction intervals (skip first and last 5% to avoid intro/credits)
    let start_time = duration_seconds * 0.05;
    let end_time = duration_seconds * 0.95;
    let usable_duration = end_time - start_time;
    
    if usable_duration <= 0.0 {
        anyhow::bail!("Video is too short for frame extraction");
    }
    
    let interval = usable_duration / (target_frame_count as f64 - 1.0).max(1.0);
    
    let mut frame_hashes = Vec::new();
    let hasher = HasherConfig::new().to_hasher();
    
    for i in 0..target_frame_count {
        let timestamp = start_time + (i as f64 * interval);
        
        match extract_single_frame(video_path, timestamp, gpu_config).await {
            Ok(Some(image)) => {
                // Compute perceptual hash using difference hash (dHash)
                let img_hash_image = ImgHashDynamicImage::from(image.clone());
                let hash = hasher.hash_image(&img_hash_image);
                frame_hashes.push(hash.as_bytes().to_vec());
                
                debug!("Extracted frame at {:.1}s: hash={}", timestamp, hash.to_base64());
            }
            Ok(None) => {
                warn!("Failed to extract frame at timestamp {:.1}s", timestamp);
            }
            Err(e) => {
                warn!("Error extracting frame at {:.1}s: {}", timestamp, e);
            }
        }
    }
    
    Ok(frame_hashes)
}

/// Extract a single frame from video at specified timestamp
async fn extract_single_frame(
    video_path: &Path,
    timestamp: f64,
    gpu_config: Option<&GpuAccelConfig>
) -> Result<Option<image::DynamicImage>> {
    let mut cmd = Command::new("ffmpeg");

    // Add GPU acceleration arguments if available
    if let Some(config) = gpu_config {
        if let Some(hwaccel) = &config.hwaccel {
            cmd.args(["-hwaccel", hwaccel]);
        }

        // Add extra GPU-specific arguments
        for arg in &config.extra_args {
            cmd.arg(arg);
        }
    }

    cmd.args([
        "-v", "quiet",                    // Suppress verbose output
        "-ss", &timestamp.to_string(),    // Seek to timestamp
    ]);

    // Add input file
    cmd.args([
        "-i", video_path.to_str().context("Invalid path encoding")?,
    ]);

    // Configure frame extraction
    cmd.args([
        "-vframes", "1",                  // Extract 1 frame
        "-q:v", "2",                      // High quality
        "-vf", "scale=256:256",           // Resize for consistent hashing
        "-f", "image2pipe",               // Output to pipe
        "-vcodec", "png",                 // PNG format
        "-",                              // Output to stdout
    ]);

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await?;
    
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    
    // Load image from bytes
    match image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png) {
        Ok(img) => Ok(Some(img)),
        Err(e) => {
            debug!("Failed to parse extracted frame: {}", e);
            Ok(None)
        }
    }
}

/// Compute average hash from multiple frame hashes
fn compute_average_hash(frame_hashes: &[Vec<u8>]) -> Result<Vec<u8>> {
    if frame_hashes.is_empty() {
        anyhow::bail!("Cannot compute average of empty hash set");
    }
    
    // All hashes should be same length (64 bits = 8 bytes for dHash)
    let hash_length = frame_hashes[0].len();
    if !frame_hashes.iter().all(|h| h.len() == hash_length) {
        anyhow::bail!("Inconsistent hash lengths");
    }
    
    // Compute bitwise majority for each bit position
    let mut average_hash = vec![0u8; hash_length];
    let threshold = frame_hashes.len() / 2;
    
    for byte_idx in 0..hash_length {
        let mut byte_value = 0u8;
        
        for bit_idx in 0..8 {
            let bit_mask = 1u8 << bit_idx;
            let set_count = frame_hashes
                .iter()
                .filter(|hash| hash[byte_idx] & bit_mask != 0)
                .count();
            
            if set_count > threshold {
                byte_value |= bit_mask;
            }
        }
        
        average_hash[byte_idx] = byte_value;
    }
    
    Ok(average_hash)
}

/// Calculate Hamming distance between two hash byte arrays
pub fn hamming_distance(hash1: &[u8], hash2: &[u8]) -> Result<u32> {
    if hash1.len() != hash2.len() {
        anyhow::bail!("Hash lengths don't match: {} vs {}", hash1.len(), hash2.len());
    }
    
    let mut distance = 0u32;
    
    for (b1, b2) in hash1.iter().zip(hash2.iter()) {
        distance += (b1 ^ b2).count_ones();
    }
    
    Ok(distance)
}

/// Check if two perceptual hash datasets are similar within thresholds
pub fn are_hashes_similar(
    hash1: &PerceptualHashData,
    hash2: &PerceptualHashData,
    avg_threshold: u32,
    max_threshold: u32,
) -> Result<bool> {
    // Compare average hashes first (fastest check)
    let avg_distance = hamming_distance(&hash1.average_hash, &hash2.average_hash)?;
    
    if avg_distance > avg_threshold {
        return Ok(false);
    }
    
    // If average hashes are similar, check individual frame comparisons
    let mut min_distances = Vec::new();
    
    // Compare each frame from hash1 to all frames in hash2, keep minimum distance
    for frame1 in &hash1.individual_hashes {
        let mut min_dist = u32::MAX;
        
        for frame2 in &hash2.individual_hashes {
            if let Ok(dist) = hamming_distance(frame1, frame2) {
                min_dist = min_dist.min(dist);
            }
        }
        
        if min_dist != u32::MAX {
            min_distances.push(min_dist);
        }
    }
    
    if min_distances.is_empty() {
        return Ok(false);
    }
    
    // Check if most frame comparisons are below threshold
    let similar_count = min_distances
        .iter()
        .filter(|&&dist| dist <= max_threshold)
        .count();
    
    let similarity_ratio = similar_count as f64 / min_distances.len() as f64;
    
    // Consider similar if at least 70% of frames are similar
    Ok(similarity_ratio >= 0.7)
}

/// Compute similarity score between two perceptual hash datasets (0.0 to 1.0)
pub fn compute_similarity_score(
    hash1: &PerceptualHashData,
    hash2: &PerceptualHashData,
) -> Result<f64> {
    // Start with average hash similarity
    let avg_distance = hamming_distance(&hash1.average_hash, &hash2.average_hash)?;
    let avg_similarity = (64.0 - avg_distance as f64) / 64.0; // Normalize for 64-bit hash
    
    // Compute frame-wise similarities
    let mut frame_similarities = Vec::new();
    
    for frame1 in &hash1.individual_hashes {
        let mut best_similarity = 0.0f64;
        
        for frame2 in &hash2.individual_hashes {
            if let Ok(dist) = hamming_distance(frame1, frame2) {
                let similarity = (64.0 - dist as f64) / 64.0;
                best_similarity = best_similarity.max(similarity);
            }
        }
        
        frame_similarities.push(best_similarity);
    }
    
    // Average frame similarity
    let avg_frame_similarity = if frame_similarities.is_empty() {
        0.0
    } else {
        frame_similarities.iter().sum::<f64>() / frame_similarities.len() as f64
    };
    
    // Weighted combination: 60% average hash, 40% frame-wise
    Ok(avg_similarity * 0.6 + avg_frame_similarity * 0.4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hamming_distance() -> Result<()> {
        let hash1 = vec![0b10101010, 0b11110000];
        let hash2 = vec![0b10101011, 0b11110001]; // 2 bits different
        
        let distance = hamming_distance(&hash1, &hash2)?;
        assert_eq!(distance, 2);
        
        Ok(())
    }

    #[test]
    fn test_average_hash_computation() -> Result<()> {
        let hashes = vec![
            vec![0b11110000], // 4 bits set
            vec![0b11111000], // 5 bits set  
            vec![0b11100000], // 3 bits set
        ];
        
        let average = compute_average_hash(&hashes)?;
        
        // Majority voting should give us a hash with the most common bits set
        assert_eq!(average.len(), 1);
        // First 3 bits should be set (appear in all), 4th bit in majority (2/3)
        assert_eq!(average[0] & 0b11110000, 0b11110000);
        
        Ok(())
    }

    #[test]
    fn test_similarity_thresholding() -> Result<()> {
        let hash1 = PerceptualHashData {
            average_hash: vec![0b10101010],
            individual_hashes: vec![vec![0b10101010], vec![0b11110000]],
            frame_count: 2,
        };
        
        let hash2_similar = PerceptualHashData {
            average_hash: vec![0b10101011], // 1 bit different
            individual_hashes: vec![vec![0b10101011], vec![0b11110001]], // Close matches
            frame_count: 2,
        };
        
        let hash2_different = PerceptualHashData {
            average_hash: vec![0b00000000], // Very different
            individual_hashes: vec![vec![0b00000000], vec![0b00001111]],
            frame_count: 2,
        };
        
        // Similar hashes should match with reasonable thresholds
        assert!(are_hashes_similar(&hash1, &hash2_similar, 5, 5)?);
        
        // Different hashes should not match
        assert!(!are_hashes_similar(&hash1, &hash2_different, 5, 5)?);
        
        Ok(())
    }
}