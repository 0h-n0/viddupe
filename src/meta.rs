use anyhow::{Context, Result};
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

use crate::db::VideoMetadata;

#[derive(Debug, Deserialize)]
struct FFProbeOutput {
    streams: Vec<FFProbeStream>,
    format: FFProbeFormat,
}

#[derive(Debug, Deserialize)]
struct FFProbeStream {
    #[serde(rename = "codec_type")]
    codec_type: String,
    #[serde(rename = "codec_name")]
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(rename = "bit_rate")]
    bit_rate: Option<String>,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FFProbeFormat {
    #[serde(rename = "format_name")]
    format_name: Option<String>,
    duration: Option<String>,
    size: Option<String>,
    #[serde(rename = "bit_rate")]
    bit_rate: Option<String>,
}

/// Extract comprehensive metadata from a video file using ffprobe
pub async fn extract_metadata(video_path: &Path) -> Result<VideoMetadata> {
    debug!("Extracting metadata from: {}", video_path.display());
    
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",           // Suppress banner and progress info
            "-print_format", "json", // Output in JSON format
            "-show_format",          // Show container/format info
            "-show_streams",         // Show stream info
            video_path.to_str().ok_or_else(|| {
                anyhow::anyhow!("Invalid UTF-8 in path: {}", video_path.display())
            })?,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("Failed to execute ffprobe on {}", video_path.display()))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "ffprobe failed for {}: {}",
            video_path.display(),
            stderr.trim()
        );
    }
    
    let json_str = String::from_utf8(output.stdout)
        .context("ffprobe output is not valid UTF-8")?;
    
    let ffprobe_data: FFProbeOutput = serde_json::from_str(&json_str)
        .with_context(|| format!("Failed to parse ffprobe JSON output for {}", video_path.display()))?;
    
    let metadata = parse_ffprobe_output(ffprobe_data)?;
    
    debug!("Extracted metadata: duration={:.1}s, {}x{}, codecs={}|{}", 
           metadata.duration_seconds,
           metadata.width.unwrap_or(0),
           metadata.height.unwrap_or(0),
           metadata.video_codec.as_deref().unwrap_or("unknown"),
           metadata.audio_codec.as_deref().unwrap_or("none"));
    
    Ok(metadata)
}

fn parse_ffprobe_output(data: FFProbeOutput) -> Result<VideoMetadata> {
    // Extract duration (prefer format duration, fall back to stream)
    let duration_seconds = data.format.duration
        .as_ref()
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| {
            data.streams
                .iter()
                .find_map(|s| s.duration.as_ref()?.parse::<f64>().ok())
        })
        .unwrap_or(0.0);
    
    // Find video stream
    let video_stream = data.streams
        .iter()
        .find(|s| s.codec_type == "video");
    
    // Find audio stream
    let audio_stream = data.streams
        .iter()
        .find(|s| s.codec_type == "audio");
    
    // Extract video properties
    let (width, height, video_codec) = match video_stream {
        Some(stream) => (
            stream.width,
            stream.height,
            stream.codec_name.clone(),
        ),
        None => (None, None, None),
    };
    
    // Extract audio properties
    let (audio_codec, has_audio) = match audio_stream {
        Some(stream) => (stream.codec_name.clone(), true),
        None => (None, false),
    };
    
    // Extract bitrate (prefer format bitrate, fall back to video stream)
    let bitrate = data.format.bit_rate
        .as_ref()
        .and_then(|br| br.parse::<u64>().ok())
        .or_else(|| {
            video_stream?
                .bit_rate
                .as_ref()
                .and_then(|br| br.parse::<u64>().ok())
        });
    
    Ok(VideoMetadata {
        duration_seconds,
        width,
        height,
        video_codec,
        audio_codec,
        bitrate,
        has_audio,
        format_name: data.format.format_name,
    })
}

/// Get video resolution as a standardized string (e.g., "1920x1080")
pub fn format_resolution(width: Option<u32>, height: Option<u32>) -> String {
    match (width, height) {
        (Some(w), Some(h)) => format!("{}x{}", w, h),
        _ => "unknown".to_string(),
    }
}

/// Classify resolution into common categories
pub fn classify_resolution(width: Option<u32>, height: Option<u32>) -> ResolutionClass {
    match (width, height) {
        (Some(w), Some(h)) => {
            let pixels = w * h;
            if pixels >= 3840 * 2160 { // 4K
                ResolutionClass::UHD4K
            } else if pixels >= 2560 * 1440 { // 1440p
                ResolutionClass::QHD
            } else if pixels >= 1920 * 1080 { // 1080p
                ResolutionClass::FHD
            } else if pixels >= 1280 * 720 { // 720p
                ResolutionClass::HD
            } else if pixels >= 854 * 480 { // 480p
                ResolutionClass::SD
            } else {
                ResolutionClass::LowRes
            }
        }
        _ => ResolutionClass::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolutionClass {
    Unknown,
    LowRes,   // < 480p
    SD,       // 480p
    HD,       // 720p
    FHD,      // 1080p
    QHD,      // 1440p
    UHD4K,    // 2160p+
}

impl std::fmt::Display for ResolutionClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolutionClass::Unknown => write!(f, "Unknown"),
            ResolutionClass::LowRes => write!(f, "Low Resolution"),
            ResolutionClass::SD => write!(f, "480p (SD)"),
            ResolutionClass::HD => write!(f, "720p (HD)"),
            ResolutionClass::FHD => write!(f, "1080p (Full HD)"),
            ResolutionClass::QHD => write!(f, "1440p (QHD)"),
            ResolutionClass::UHD4K => write!(f, "4K (UHD)"),
        }
    }
}

/// Classify video codec quality/modernity
pub fn classify_codec(codec: &str) -> CodecClass {
    match codec.to_lowercase().as_str() {
        "h264" | "avc1" => CodecClass::H264,
        "h265" | "hevc" => CodecClass::H265,
        "av1" => CodecClass::AV1,
        "vp9" => CodecClass::VP9,
        "vp8" => CodecClass::VP8,
        "mpeg4" | "xvid" | "divx" => CodecClass::MPEG4,
        "mpeg2" => CodecClass::MPEG2,
        "mpeg1" => CodecClass::MPEG1,
        _ => CodecClass::Other(codec.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodecClass {
    MPEG1,
    MPEG2, 
    MPEG4,
    VP8,
    H264,
    VP9,
    H265,
    AV1,
    Other(String),
}

impl CodecClass {
    /// Get relative quality score for codec comparison (higher = better)
    pub fn quality_score(&self) -> u32 {
        match self {
            CodecClass::MPEG1 => 1,
            CodecClass::MPEG2 => 2,
            CodecClass::MPEG4 => 3,
            CodecClass::VP8 => 4,
            CodecClass::H264 => 5,
            CodecClass::VP9 => 6,
            CodecClass::H265 => 7,
            CodecClass::AV1 => 8,
            CodecClass::Other(_) => 3, // Assume similar to MPEG4
        }
    }
}

impl std::fmt::Display for CodecClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecClass::MPEG1 => write!(f, "MPEG-1"),
            CodecClass::MPEG2 => write!(f, "MPEG-2"),
            CodecClass::MPEG4 => write!(f, "MPEG-4"),
            CodecClass::VP8 => write!(f, "VP8"),
            CodecClass::H264 => write!(f, "H.264"),
            CodecClass::VP9 => write!(f, "VP9"),
            CodecClass::H265 => write!(f, "H.265"),
            CodecClass::AV1 => write!(f, "AV1"),
            CodecClass::Other(name) => write!(f, "{}", name),
        }
    }
}

/// Calculate quality score for file comparison
pub fn calculate_quality_score(metadata: &VideoMetadata) -> f64 {
    let mut score = 0.0;
    
    // Resolution score (0-100)
    let resolution_score = match classify_resolution(metadata.width, metadata.height) {
        ResolutionClass::UHD4K => 100.0,
        ResolutionClass::QHD => 80.0,
        ResolutionClass::FHD => 60.0,
        ResolutionClass::HD => 40.0,
        ResolutionClass::SD => 20.0,
        ResolutionClass::LowRes => 10.0,
        ResolutionClass::Unknown => 0.0,
    };
    score += resolution_score * 0.4; // 40% weight
    
    // Codec score (0-100)
    let codec_score = metadata.video_codec
        .as_ref()
        .map(|c| classify_codec(c).quality_score() as f64 * 12.5) // Scale to 0-100
        .unwrap_or(0.0);
    score += codec_score * 0.3; // 30% weight
    
    // Bitrate score (0-100, normalized)
    let bitrate_score = metadata.bitrate
        .map(|br| {
            // Normalize bitrate: assume 1Mbps = 10 points, cap at 50Mbps = 100 points
            let mbps = br as f64 / 1_000_000.0;
            (mbps / 50.0 * 100.0).min(100.0)
        })
        .unwrap_or(50.0); // Default middle score if unknown
    score += bitrate_score * 0.3; // 30% weight
    
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolution_classification() {
        assert_eq!(classify_resolution(Some(1920), Some(1080)), ResolutionClass::FHD);
        assert_eq!(classify_resolution(Some(3840), Some(2160)), ResolutionClass::UHD4K);
        assert_eq!(classify_resolution(Some(1280), Some(720)), ResolutionClass::HD);
        assert_eq!(classify_resolution(None, None), ResolutionClass::Unknown);
    }

    #[test]
    fn test_codec_classification() {
        assert_eq!(classify_codec("h264"), CodecClass::H264);
        assert_eq!(classify_codec("H265"), CodecClass::H265);
        assert_eq!(classify_codec("av1"), CodecClass::AV1);
        
        assert!(classify_codec("h265").quality_score() > classify_codec("h264").quality_score());
        assert!(classify_codec("av1").quality_score() > classify_codec("h265").quality_score());
    }

    #[test]
    fn test_quality_score_calculation() {
        let high_quality = VideoMetadata {
            duration_seconds: 120.0,
            width: Some(3840),
            height: Some(2160),
            video_codec: Some("h265".to_string()),
            audio_codec: Some("aac".to_string()),
            bitrate: Some(20_000_000), // 20 Mbps
            has_audio: true,
            format_name: Some("mp4".to_string()),
        };
        
        let low_quality = VideoMetadata {
            duration_seconds: 120.0,
            width: Some(640),
            height: Some(480),
            video_codec: Some("mpeg4".to_string()),
            audio_codec: Some("mp3".to_string()),
            bitrate: Some(1_000_000), // 1 Mbps
            has_audio: true,
            format_name: Some("avi".to_string()),
        };
        
        assert!(calculate_quality_score(&high_quality) > calculate_quality_score(&low_quality));
    }

    #[test]
    fn test_format_resolution() {
        assert_eq!(format_resolution(Some(1920), Some(1080)), "1920x1080");
        assert_eq!(format_resolution(None, None), "unknown");
        assert_eq!(format_resolution(Some(1920), None), "unknown");
    }
}