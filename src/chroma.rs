use anyhow::{Context, Result};
use log::{debug, warn};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Compute Chromaprint audio fingerprint using fpcalc
pub async fn compute_chromaprint(video_path: &Path) -> Result<String> {
    debug!("Computing Chromaprint for: {}", video_path.display());
    
    let output = Command::new("fpcalc")
        .args([
            "-length", "120",  // Analyze first 2 minutes
            "-raw",           // Output raw fingerprint
            video_path.to_str().context("Invalid path encoding")?,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("Failed to execute fpcalc on {}", video_path.display()))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("fpcalc failed for {}: {}", video_path.display(), stderr.trim());
    }
    
    let output_text = String::from_utf8(output.stdout)
        .context("fpcalc output is not valid UTF-8")?;
    
    parse_fpcalc_output(&output_text)
        .with_context(|| format!("Failed to parse fpcalc output for {}", video_path.display()))
}

/// Parse fpcalc output to extract the fingerprint
fn parse_fpcalc_output(output: &str) -> Result<String> {
    for line in output.lines() {
        if line.starts_with("FINGERPRINT=") {
            let fingerprint = line.strip_prefix("FINGERPRINT=")
                .context("Failed to extract fingerprint")?
                .trim()
                .to_string();
            
            if fingerprint.is_empty() {
                anyhow::bail!("Empty fingerprint extracted");
            }
            
            debug!("Extracted fingerprint: {} chars", fingerprint.len());
            return Ok(fingerprint);
        }
    }
    
    anyhow::bail!("No fingerprint found in fpcalc output");
}

/// Calculate similarity between two Chromaprint fingerprints
/// Returns a score from 0.0 (completely different) to 1.0 (identical)
pub fn calculate_chromaprint_similarity(fingerprint1: &str, fingerprint2: &str) -> f64 {
    if fingerprint1 == fingerprint2 {
        return 1.0;
    }
    
    // Simple approach: compare fingerprints as strings
    // More sophisticated approaches would parse the actual fingerprint data
    let fp1_parts: Vec<&str> = fingerprint1.split(',').collect();
    let fp2_parts: Vec<&str> = fingerprint2.split(',').collect();
    
    if fp1_parts.is_empty() || fp2_parts.is_empty() {
        return 0.0;
    }
    
    // Compare overlapping parts
    let min_len = fp1_parts.len().min(fp2_parts.len());
    let max_len = fp1_parts.len().max(fp2_parts.len());
    
    let mut matching_parts = 0;
    
    for i in 0..min_len {
        if fp1_parts[i] == fp2_parts[i] {
            matching_parts += 1;
        }
    }
    
    // Account for length differences
    let base_similarity = matching_parts as f64 / max_len as f64;
    let length_penalty = 1.0 - (max_len - min_len) as f64 / max_len as f64 * 0.3;
    
    (base_similarity * length_penalty).max(0.0)
}

/// Check if two Chromaprint fingerprints are considered similar
pub fn are_fingerprints_similar(fingerprint1: &str, fingerprint2: &str, threshold: f64) -> bool {
    calculate_chromaprint_similarity(fingerprint1, fingerprint2) >= threshold
}

/// Simplified Chromaprint extraction for cases where fpcalc is not available
/// This is a fallback that just returns a hash of the first few seconds of audio
pub async fn compute_simple_audio_hash(video_path: &Path) -> Result<String> {
    debug!("Computing simple audio hash for: {}", video_path.display());
    
    // Extract 10 seconds of audio data for hashing
    let output = Command::new("ffmpeg")
        .args([
            "-v", "quiet",
            "-i", video_path.to_str().context("Invalid path encoding")?,
            "-t", "10",          // First 10 seconds
            "-vn",               // No video
            "-acodec", "pcm_s16le",
            "-ar", "8000",       // Low sample rate for speed
            "-ac", "1",          // Mono
            "-f", "wav",
            "-",                 // Output to stdout
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await?;
    
    if !output.status.success() || output.stdout.is_empty() {
        anyhow::bail!("Failed to extract audio data");
    }
    
    // Compute hash of audio data
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    output.stdout.hash(&mut hasher);
    let hash = hasher.finish();
    
    Ok(format!("simple:{:016x}", hash))
}

/// Determine if a fingerprint is from the simple audio hash method
pub fn is_simple_hash(fingerprint: &str) -> bool {
    fingerprint.starts_with("simple:")
}

/// Compare fingerprints, handling both Chromaprint and simple hash formats
pub fn compare_any_fingerprints(fp1: &str, fp2: &str, threshold: f64) -> bool {
    match (is_simple_hash(fp1), is_simple_hash(fp2)) {
        (true, true) => fp1 == fp2, // Simple hashes must match exactly
        (false, false) => are_fingerprints_similar(fp1, fp2, threshold),
        _ => false, // Different formats don't match
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fpcalc_output_parsing() -> Result<()> {
        let mock_output = "DURATION=120.5\nFINGERPRINT=AQABEAmUaEkSRZEGHYmORBGl\nOTHER=value";
        
        let fingerprint = parse_fpcalc_output(mock_output)?;
        assert_eq!(fingerprint, "AQABEAmUaEkSRZEGHYmORBGl");
        
        Ok(())
    }

    #[test]
    fn test_fingerprint_similarity() {
        let fp1 = "123,456,789,012";
        let fp2 = "123,456,789,012"; // Identical
        let fp3 = "123,456,999,012"; // One different
        let fp4 = "999,888,777,666"; // Completely different
        
        assert_eq!(calculate_chromaprint_similarity(fp1, fp2), 1.0);
        assert!(calculate_chromaprint_similarity(fp1, fp3) > 0.5);
        assert!(calculate_chromaprint_similarity(fp1, fp4) < 0.5);
    }

    #[test]
    fn test_similarity_threshold() {
        let fp1 = "123,456,789";
        let fp2 = "123,456,999"; // 2/3 match
        
        assert!(are_fingerprints_similar(fp1, fp2, 0.6));
        assert!(!are_fingerprints_similar(fp1, fp2, 0.8));
    }

    #[test]
    fn test_simple_hash_detection() {
        assert!(is_simple_hash("simple:123456789abcdef0"));
        assert!(!is_simple_hash("AQABEAmUaEkSRZEGHYmORBGl"));
    }

    #[test]
    fn test_mixed_fingerprint_comparison() {
        let chromaprint = "AQABEAmUaEkSRZEGHYmORBGl";
        let simple = "simple:123456789abcdef0";
        
        assert!(!compare_any_fingerprints(chromaprint, simple, 0.8));
        assert!(compare_any_fingerprints(simple, simple, 0.8));
        assert!(compare_any_fingerprints(chromaprint, chromaprint, 0.8));
    }
}