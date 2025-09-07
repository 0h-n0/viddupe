use anyhow::{Context, Result};
use log::{debug, warn};
use std::io::SeekFrom;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use xxhash_rust::xxh3::xxh3_64;

use crate::db::CoarseHashes;

/// Default chunk size for reading file portions (16MB)
const DEFAULT_CHUNK_SIZE: usize = 16 * 1024 * 1024;

/// Compute xxHash3 hashes for the head and tail portions of a file
/// This provides a fast first-pass similarity check before expensive perceptual analysis
pub async fn compute_coarse_hashes(file_path: &Path, chunk_size: usize) -> Result<CoarseHashes> {
    debug!("Computing coarse hashes for: {} (chunk_size: {})", file_path.display(), chunk_size);
    
    let mut file = File::open(file_path)
        .await
        .with_context(|| format!("Failed to open file for hashing: {}", file_path.display()))?;
    
    // Get file size
    let metadata = file.metadata()
        .await
        .with_context(|| format!("Failed to get file metadata: {}", file_path.display()))?;
    let file_size = metadata.len() as usize;
    
    if file_size == 0 {
        anyhow::bail!("File is empty: {}", file_path.display());
    }
    
    // Adjust chunk size if file is smaller
    let actual_chunk_size = chunk_size.min(file_size / 2).max(1024); // Minimum 1KB, max half the file
    
    debug!("File size: {}, using chunk size: {}", file_size, actual_chunk_size);
    
    // Read head portion
    let head_data = read_file_chunk(&mut file, SeekFrom::Start(0), actual_chunk_size).await
        .with_context(|| format!("Failed to read head chunk from: {}", file_path.display()))?;
    
    // Read tail portion
    let tail_offset = if file_size > actual_chunk_size {
        file_size - actual_chunk_size
    } else {
        0
    };
    
    let tail_data = read_file_chunk(&mut file, SeekFrom::Start(tail_offset as u64), actual_chunk_size).await
        .with_context(|| format!("Failed to read tail chunk from: {}", file_path.display()))?;
    
    // Compute hashes
    let head_hash = compute_xxh3_hash(&head_data);
    let tail_hash = compute_xxh3_hash(&tail_data);
    
    debug!("Computed coarse hashes: head={:016x}, tail={:016x}", 
           u64::from_le_bytes(head_hash[0..8].try_into().unwrap()),
           u64::from_le_bytes(tail_hash[0..8].try_into().unwrap()));
    
    Ok(CoarseHashes {
        head_hash,
        tail_hash,
    })
}

/// Read a chunk of data from a file at the specified position
async fn read_file_chunk(file: &mut File, seek_pos: SeekFrom, chunk_size: usize) -> Result<Vec<u8>> {
    file.seek(seek_pos).await?;
    
    let mut buffer = vec![0u8; chunk_size];
    let bytes_read = file.read(&mut buffer).await?;
    
    if bytes_read == 0 {
        anyhow::bail!("No data read from file chunk");
    }
    
    // Truncate buffer to actual bytes read
    buffer.truncate(bytes_read);
    Ok(buffer)
}

/// Compute xxHash3 64-bit hash and return as bytes for database storage
fn compute_xxh3_hash(data: &[u8]) -> Vec<u8> {
    let hash = xxh3_64(data);
    hash.to_le_bytes().to_vec()
}

/// Convert hash bytes back to u64 for comparison
pub fn hash_bytes_to_u64(hash_bytes: &[u8]) -> Result<u64> {
    if hash_bytes.len() != 8 {
        anyhow::bail!("Invalid hash length: expected 8 bytes, got {}", hash_bytes.len());
    }
    
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(hash_bytes);
    Ok(u64::from_le_bytes(bytes))
}

/// Compare two coarse hash sets for similarity
/// Returns true if both head and tail hashes match exactly
pub fn hashes_match_exactly(hash1: &CoarseHashes, hash2: &CoarseHashes) -> bool {
    hash1.head_hash == hash2.head_hash && hash1.tail_hash == hash2.tail_hash
}

/// Compare coarse hashes with some tolerance (for similar but not identical files)
/// This could be useful for files with different metadata or slight encoding differences
pub fn hashes_match_approximately(hash1: &CoarseHashes, hash2: &CoarseHashes, tolerance: u64) -> Result<bool> {
    let head1 = hash_bytes_to_u64(&hash1.head_hash)?;
    let head2 = hash_bytes_to_u64(&hash2.head_hash)?;
    let tail1 = hash_bytes_to_u64(&hash1.tail_hash)?;
    let tail2 = hash_bytes_to_u64(&hash2.tail_hash)?;
    
    let head_diff = head1.abs_diff(head2);
    let tail_diff = tail1.abs_diff(tail2);
    
    Ok(head_diff <= tolerance && tail_diff <= tolerance)
}

/// Compute similarity score between two coarse hash sets (0.0 = completely different, 1.0 = identical)
pub fn compute_hash_similarity(hash1: &CoarseHashes, hash2: &CoarseHashes) -> Result<f64> {
    if hashes_match_exactly(hash1, hash2) {
        return Ok(1.0);
    }
    
    let head1 = hash_bytes_to_u64(&hash1.head_hash)?;
    let head2 = hash_bytes_to_u64(&hash2.head_hash)?;
    let tail1 = hash_bytes_to_u64(&hash1.tail_hash)?;
    let tail2 = hash_bytes_to_u64(&hash2.tail_hash)?;
    
    // Calculate Hamming distance for each hash
    let head_hamming = (head1 ^ head2).count_ones() as f64;
    let tail_hamming = (tail1 ^ tail2).count_ones() as f64;
    
    // Normalize to 0-1 scale (64 bits max difference)
    let head_similarity = (64.0 - head_hamming) / 64.0;
    let tail_similarity = (64.0 - tail_hamming) / 64.0;
    
    // Average similarity of head and tail
    Ok((head_similarity + tail_similarity) / 2.0)
}

/// Fast pre-filter for potential duplicates using coarse hashes
/// Groups files by their coarse hashes to reduce comparison space
pub fn group_by_coarse_hashes(files_with_hashes: Vec<(u64, CoarseHashes)>) -> Vec<Vec<u64>> {
    use std::collections::HashMap;
    
    let mut hash_groups: HashMap<(u64, u64), Vec<u64>> = HashMap::new();
    
    for (file_id, hashes) in files_with_hashes {
        if let (Ok(head_hash), Ok(tail_hash)) = (
            hash_bytes_to_u64(&hashes.head_hash),
            hash_bytes_to_u64(&hashes.tail_hash),
        ) {
            let key = (head_hash, tail_hash);
            hash_groups.entry(key).or_default().push(file_id);
        } else {
            warn!("Failed to convert hash bytes for file ID: {}", file_id);
        }
    }
    
    // Return groups with more than one file (potential duplicates)
    hash_groups
        .into_values()
        .filter(|group| group.len() > 1)
        .collect()
}

/// Utility function to format hash for display
pub fn format_hash(hash_bytes: &[u8]) -> String {
    if let Ok(hash_u64) = hash_bytes_to_u64(hash_bytes) {
        format!("{:016x}", hash_u64)
    } else {
        hash_bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
    }
}


/// Compute coarse hashes with custom settings optimized for different file sizes
pub async fn compute_adaptive_coarse_hashes(file_path: &Path, file_size: u64) -> Result<CoarseHashes> {
    // Adaptive chunk sizing based on file size
    let chunk_size = if file_size < 10 * 1024 * 1024 { // < 10MB
        1024 * 1024 // 1MB chunks
    } else if file_size < 100 * 1024 * 1024 { // < 100MB
        4 * 1024 * 1024 // 4MB chunks
    } else if file_size < 1024 * 1024 * 1024 { // < 1GB
        16 * 1024 * 1024 // 16MB chunks
    } else {
        32 * 1024 * 1024 // 32MB chunks for very large files
    };
    
    compute_coarse_hashes(file_path, chunk_size).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tokio::runtime::Runtime;

    #[test]
    fn test_hash_conversion() -> Result<()> {
        let original_hash = 0x123456789abcdef0u64;
        let hash_bytes = original_hash.to_le_bytes().to_vec();
        let converted_back = hash_bytes_to_u64(&hash_bytes)?;
        
        assert_eq!(original_hash, converted_back);
        Ok(())
    }

    #[test] 
    fn test_exact_hash_matching() -> Result<()> {
        let hash1 = CoarseHashes {
            head_hash: vec![1, 2, 3, 4, 5, 6, 7, 8],
            tail_hash: vec![9, 10, 11, 12, 13, 14, 15, 16],
        };
        
        let hash2 = CoarseHashes {
            head_hash: vec![1, 2, 3, 4, 5, 6, 7, 8],
            tail_hash: vec![9, 10, 11, 12, 13, 14, 15, 16],
        };
        
        let hash3 = CoarseHashes {
            head_hash: vec![1, 2, 3, 4, 5, 6, 7, 9], // Different
            tail_hash: vec![9, 10, 11, 12, 13, 14, 15, 16],
        };
        
        assert!(hashes_match_exactly(&hash1, &hash2));
        assert!(!hashes_match_exactly(&hash1, &hash3));
        Ok(())
    }

    #[tokio::test]
    async fn test_coarse_hash_computation() -> Result<()> {
        // Create a temporary file with known content
        let mut temp_file = NamedTempFile::new()?;
        let test_data = b"This is test data for hash computation testing. It should be long enough to test chunking behavior properly.";
        temp_file.write_all(test_data)?;
        
        let hashes = compute_coarse_hashes(temp_file.path(), 32).await?;
        
        // Verify hashes are computed (non-empty)
        assert_eq!(hashes.head_hash.len(), 8);
        assert_eq!(hashes.tail_hash.len(), 8);
        
        // Verify hash consistency (computing again should give same result)
        let hashes2 = compute_coarse_hashes(temp_file.path(), 32).await?;
        assert!(hashes_match_exactly(&hashes, &hashes2));
        
        Ok(())
    }

    #[test]
    fn test_hash_similarity_computation() -> Result<()> {
        let identical_hash1 = CoarseHashes {
            head_hash: vec![1, 2, 3, 4, 5, 6, 7, 8],
            tail_hash: vec![9, 10, 11, 12, 13, 14, 15, 16],
        };
        
        let identical_hash2 = identical_hash1.clone();
        
        let similarity = compute_hash_similarity(&identical_hash1, &identical_hash2)?;
        assert_eq!(similarity, 1.0);
        
        Ok(())
    }

    #[test]
    fn test_hash_grouping() {
        let files_with_hashes = vec![
            (1u64, CoarseHashes {
                head_hash: vec![1, 2, 3, 4, 5, 6, 7, 8],
                tail_hash: vec![9, 10, 11, 12, 13, 14, 15, 16],
            }),
            (2u64, CoarseHashes {
                head_hash: vec![1, 2, 3, 4, 5, 6, 7, 8], // Same as file 1
                tail_hash: vec![9, 10, 11, 12, 13, 14, 15, 16],
            }),
            (3u64, CoarseHashes {
                head_hash: vec![17, 18, 19, 20, 21, 22, 23, 24], // Different
                tail_hash: vec![25, 26, 27, 28, 29, 30, 31, 32],
            }),
        ];
        
        let groups = group_by_coarse_hashes(files_with_hashes);
        
        // Should have one group with files 1 and 2 
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
        assert!(groups[0].contains(&1));
        assert!(groups[0].contains(&2));
    }

    #[test]
    fn test_hash_formatting() {
        let hash_bytes = vec![0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0];
        let formatted = format_hash(&hash_bytes);
        assert_eq!(formatted.len(), 16); // 16 hex characters for 64-bit hash
    }
}