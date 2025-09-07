use anyhow::Result;
use chrono::{DateTime, Utc};
use log::{debug, info};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::{
    chroma,
    cli::CommonOptions,
    coarse,
    db::{get_all_files, FileRecord},
    meta,
    phash,
};

#[derive(Debug, Clone)]
pub struct DuplicateCluster {
    pub files: Vec<ClusterFile>,
    pub confidence_score: f64,
    pub detection_method: String,
}

#[derive(Debug, Clone)]
pub struct ClusterFile {
    pub id: i64,
    pub path: PathBuf,
    pub size: u64,
    pub modified: DateTime<Utc>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub bitrate: Option<u64>,
    pub quality_score: f64,
}

impl DuplicateCluster {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Get the recommended file to keep based on quality metrics
    pub fn get_recommended_to_keep(&self) -> Option<&ClusterFile> {
        self.files
            .iter()
            .max_by(|a, b| a.quality_score.partial_cmp(&b.quality_score).unwrap())
    }

    /// Get files recommended for deletion (all except the best one)
    pub fn get_files_to_delete(&self) -> Vec<&ClusterFile> {
        if let Some(keep) = self.get_recommended_to_keep() {
            self.files.iter().filter(|f| f.id != keep.id).collect()
        } else {
            Vec::new()
        }
    }
}

impl ClusterFile {
    fn from_file_record(record: &FileRecord) -> Self {
        let quality_score = meta::calculate_quality_score(&record.metadata);

        Self {
            id: record.id,
            path: record.path.clone(),
            size: record.size,
            modified: record.modified,
            width: record.metadata.width,
            height: record.metadata.height,
            video_codec: record.metadata.video_codec.clone(),
            bitrate: record.metadata.bitrate,
            quality_score,
        }
    }
}

/// Find all duplicate clusters in the database
pub async fn find_duplicate_clusters(
    conn: &Connection,
    options: &CommonOptions,
) -> Result<Vec<DuplicateCluster>> {
    info!("Finding duplicate clusters...");

    let files = get_all_files(conn)?;
    if files.len() < 2 {
        info!("Not enough files for duplicate detection");
        return Ok(vec![]);
    }

    info!("Analyzing {} files for duplicates", files.len());

    let mut clusters = Vec::new();

    // Stage 1: Group by coarse hashes (fast pre-filtering)
    let coarse_groups = group_by_coarse_hashes(&files);
    debug!("Found {} coarse hash groups", coarse_groups.len());

    for group in coarse_groups {
        if group.len() < 2 {
            continue;
        }

        // Stage 2: Detailed comparison within each coarse group
        let group_clusters = analyze_group_for_duplicates(&group, options).await?;
        clusters.extend(group_clusters);
    }

    // Stage 3: Look for perceptual duplicates that might have different coarse hashes
    // (e.g., different encoding but same content)
    if !files.is_empty() {
        let perceptual_clusters = find_perceptual_duplicates(&files, options).await?;
        clusters.extend(perceptual_clusters);
    }

    // Remove overlapping clusters and sort by confidence
    let unique_clusters = deduplicate_clusters(clusters);

    info!("Found {} duplicate clusters", unique_clusters.len());
    Ok(unique_clusters)
}

/// Group files by their coarse hashes for initial filtering
fn group_by_coarse_hashes(files: &[FileRecord]) -> Vec<Vec<&FileRecord>> {
    let mut hash_groups: HashMap<(Vec<u8>, Vec<u8>), Vec<&FileRecord>> = HashMap::new();

    for file in files {
        if let Some(ref hashes) = file.coarse_hashes {
            let key = (hashes.head_hash.clone(), hashes.tail_hash.clone());
            hash_groups.entry(key).or_default().push(file);
        }
    }

    // Return groups with 2+ files
    hash_groups
        .into_values()
        .filter(|group| group.len() > 1)
        .collect()
}

/// Analyze a group of files with similar coarse hashes for true duplicates
async fn analyze_group_for_duplicates(
    files: &[&FileRecord],
    options: &CommonOptions,
) -> Result<Vec<DuplicateCluster>> {
    let mut clusters = Vec::new();

    debug!("Analyzing group of {} files for duplicates", files.len());

    // Check all pairs in the group
    for i in 0..files.len() {
        for j in i + 1..files.len() {
            let file1 = files[i];
            let file2 = files[j];

            if let Some(cluster) = compare_files_for_duplicates(file1, file2, options).await? {
                // Check if these files are already in an existing cluster
                if !clusters.iter().any(|c: &DuplicateCluster| {
                    c.files.iter().any(|f| f.id == file1.id || f.id == file2.id)
                }) {
                    clusters.push(cluster);
                }
            }
        }
    }

    Ok(clusters)
}

/// Compare two files to determine if they are duplicates
async fn compare_files_for_duplicates(
    file1: &FileRecord,
    file2: &FileRecord,
    options: &CommonOptions,
) -> Result<Option<DuplicateCluster>> {
    debug!("Comparing files: {} vs {}", file1.path.display(), file2.path.display());

    let mut confidence = 0.0f64;
    let mut detection_methods = Vec::new();

    // Check 1: Coarse hashes (already similar if we got here, but double-check)
    if let (Some(hash1), Some(hash2)) = (&file1.coarse_hashes, &file2.coarse_hashes) {
        if coarse::hashes_match_exactly(hash1, hash2) {
            confidence += 0.9; // Very high confidence for exact coarse match
            detection_methods.push("exact_coarse_hash");
        }
    }

    // Check 2: Perceptual hashes
    if let (Some(phash1), Some(phash2)) = (&file1.phash_data, &file2.phash_data) {
        let phash_similar = phash::are_hashes_similar(
            phash1,
            phash2,
            options.threshold_phash_avg,
            options.threshold_phash_max,
        )?;

        if phash_similar {
            let similarity = phash::compute_similarity_score(phash1, phash2)?;
            confidence += similarity * 0.7; // Weight perceptual similarity
            detection_methods.push("perceptual_hash");
        }
    }

    // Check 3: Audio fingerprints
    if let (Some(fp1), Some(fp2)) = (&file1.chromaprint, &file2.chromaprint) {
        if chroma::compare_any_fingerprints(fp1, fp2, 0.8) {
            confidence += 0.6; // Audio similarity adds confidence
            detection_methods.push("audio_fingerprint");
        }
    }

    // Check 4: Metadata consistency
    let metadata_similar = are_metadata_compatible(&file1.metadata, &file2.metadata);
    if metadata_similar {
        confidence += 0.2;
        detection_methods.push("metadata");
    }

    // Decision threshold
    if confidence >= 0.8 && !detection_methods.is_empty() {
        let cluster = DuplicateCluster {
            files: vec![
                ClusterFile::from_file_record(file1),
                ClusterFile::from_file_record(file2),
            ],
            confidence_score: confidence.min(1.0),
            detection_method: detection_methods.join("+"),
        };

        debug!("Found duplicate pair: {} (confidence: {:.2})",
               cluster.detection_method, cluster.confidence_score);

        Ok(Some(cluster))
    } else {
        Ok(None)
    }
}

/// Find duplicates based on perceptual similarity even with different coarse hashes
async fn find_perceptual_duplicates(
    files: &[FileRecord],
    options: &CommonOptions,
) -> Result<Vec<DuplicateCluster>> {
    let mut clusters = Vec::new();
    
    // Only compare files that have perceptual hashes
    let files_with_phash: Vec<&FileRecord> = files
        .iter()
        .filter(|f| f.phash_data.is_some())
        .collect();

    if files_with_phash.len() < 2 {
        return Ok(clusters);
    }

    debug!("Checking {} files for perceptual duplicates", files_with_phash.len());

    // Compare all pairs (this could be optimized with similarity search structures)
    for i in 0..files_with_phash.len() {
        for j in i + 1..files_with_phash.len() {
            let file1 = files_with_phash[i];
            let file2 = files_with_phash[j];

            // Skip if already identified by coarse hash
            if let (Some(hash1), Some(hash2)) = (&file1.coarse_hashes, &file2.coarse_hashes) {
                if coarse::hashes_match_exactly(hash1, hash2) {
                    continue; // Already found by coarse hash analysis
                }
            }

            if let (Some(phash1), Some(phash2)) = (&file1.phash_data, &file2.phash_data) {
                if phash::are_hashes_similar(
                    phash1,
                    phash2,
                    options.threshold_phash_avg,
                    options.threshold_phash_max,
                )? {
                    let similarity = phash::compute_similarity_score(phash1, phash2)?;
                    
                    if similarity >= 0.85 {  // High threshold for perceptual-only matches
                        let cluster = DuplicateCluster {
                            files: vec![
                                ClusterFile::from_file_record(file1),
                                ClusterFile::from_file_record(file2),
                            ],
                            confidence_score: similarity,
                            detection_method: "perceptual_only".to_string(),
                        };

                        clusters.push(cluster);
                    }
                }
            }
        }
    }

    Ok(clusters)
}

/// Check if metadata between two files is compatible (same content, different encoding)
fn are_metadata_compatible(meta1: &crate::db::VideoMetadata, meta2: &crate::db::VideoMetadata) -> bool {
    // Duration should be very similar (within 1% or 2 seconds)
    let duration_diff = (meta1.duration_seconds - meta2.duration_seconds).abs();
    let duration_tolerance = (meta1.duration_seconds * 0.01).max(2.0);
    
    if duration_diff > duration_tolerance {
        return false;
    }

    // Resolution can be different (different encodings), but if specified should be reasonable
    if let (Some(w1), Some(h1), Some(w2), Some(h2)) = (meta1.width, meta1.height, meta2.width, meta2.height) {
        let aspect_ratio_1 = w1 as f64 / h1 as f64;
        let aspect_ratio_2 = w2 as f64 / h2 as f64;
        
        // Aspect ratios should be similar
        if (aspect_ratio_1 - aspect_ratio_2).abs() > 0.1 {
            return false;
        }
    }

    // Audio presence should match
    if meta1.has_audio != meta2.has_audio {
        return false;
    }

    true
}

/// Remove overlapping clusters and merge compatible ones
fn deduplicate_clusters(mut clusters: Vec<DuplicateCluster>) -> Vec<DuplicateCluster> {
    // Sort by confidence descending
    clusters.sort_by(|a, b| b.confidence_score.partial_cmp(&a.confidence_score).unwrap());

    let mut unique_clusters = Vec::new();
    let mut used_file_ids: HashSet<i64> = HashSet::new();

    for cluster in clusters {
        // Check if any files in this cluster are already used
        let cluster_file_ids: HashSet<i64> = cluster.files.iter().map(|f| f.id).collect();
        
        if cluster_file_ids.is_disjoint(&used_file_ids) {
            // No overlap, add this cluster
            used_file_ids.extend(&cluster_file_ids);
            unique_clusters.push(cluster);
        }
        // Otherwise skip (overlapping cluster with lower confidence)
    }

    unique_clusters
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::VideoMetadata;

    #[test]
    fn test_metadata_compatibility() {
        let meta1 = VideoMetadata {
            duration_seconds: 120.0,
            width: Some(1920),
            height: Some(1080),
            video_codec: Some("h264".to_string()),
            audio_codec: Some("aac".to_string()),
            bitrate: Some(5000000),
            has_audio: true,
            format_name: Some("mp4".to_string()),
        };

        let meta2_compatible = VideoMetadata {
            duration_seconds: 121.0, // Slightly different duration
            width: Some(1920),
            height: Some(1080),
            video_codec: Some("h265".to_string()), // Different codec OK
            audio_codec: Some("ac3".to_string()),
            bitrate: Some(8000000), // Different bitrate OK
            has_audio: true,
            format_name: Some("mkv".to_string()),
        };

        let meta2_incompatible = VideoMetadata {
            duration_seconds: 240.0, // Very different duration
            width: Some(1920),
            height: Some(1080),
            video_codec: Some("h264".to_string()),
            audio_codec: Some("aac".to_string()),
            bitrate: Some(5000000),
            has_audio: false, // Different audio presence
            format_name: Some("mp4".to_string()),
        };

        assert!(are_metadata_compatible(&meta1, &meta2_compatible));
        assert!(!are_metadata_compatible(&meta1, &meta2_incompatible));
    }

    #[test]
    fn test_cluster_file_quality_scoring() {
        use chrono::Utc;
        use std::path::PathBuf;
        
        let high_quality_metadata = VideoMetadata {
            duration_seconds: 120.0,
            width: Some(3840),
            height: Some(2160),
            video_codec: Some("h265".to_string()),
            audio_codec: Some("aac".to_string()),
            bitrate: Some(20000000),
            has_audio: true,
            format_name: Some("mp4".to_string()),
        };

        let record = FileRecord {
            id: 1,
            path: PathBuf::from("/test.mp4"),
            size: 1000000,
            modified: Utc::now(),
            metadata: high_quality_metadata,
            coarse_hashes: None,
            phash_data: None,
            chromaprint: None,
            updated_at: Utc::now(),
        };

        let cluster_file = ClusterFile::from_file_record(&record);
        
        // Should have a high quality score due to 4K resolution and H.265
        assert!(cluster_file.quality_score > 50.0);
    }
}