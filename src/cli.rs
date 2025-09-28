use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "viddupe",
    version = env!("CARGO_PKG_VERSION"),
    author = "viddupe developers",
    about = "High-performance video duplicate detection and safe removal tool",
    long_about = "Detects duplicate videos using multi-stage analysis (metadata, perceptual hashing, chromaprint) with SQLite caching for large collections (5TB+/1000+ files)"
)]
pub struct Cli {
    /// Enable debug logging
    #[arg(short, long, global = true)]
    pub debug: bool,

    /// Quiet mode - minimal output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Scan directory and build/update cache (no deletion)
    Scan {
        /// Root directory to scan
        root: PathBuf,
        #[command(flatten)]
        scan_options: ScanOptions,
        #[command(flatten)]
        common_options: CommonOptions,
    },

    /// Show duplicate clusters found in cache
    Dupes {
        /// Root directory to analyze
        root: PathBuf,
        /// Show detailed file information
        #[arg(long)]
        details: bool,
        #[command(flatten)]
        common_options: CommonOptions,
    },

    /// Interactively select and remove duplicates
    Prune {
        /// Root directory to process
        root: PathBuf,
        /// Actually delete files (default: dry-run)
        #[arg(long)]
        apply: bool,
        /// Permanent deletion instead of trash
        #[arg(long)]
        hard_delete: bool,
        /// Auto-select files to keep based on strategy
        #[arg(long, value_enum)]
        auto_keep: Option<AutoKeepStrategy>,
        /// Skip individual confirmation prompts (dangerous!)
        #[arg(long)]
        no_confirm: bool,
        /// Always confirm each deletion (default: true)
        #[arg(long, conflicts_with = "no_confirm")]
        confirm_each: bool,
        #[command(flatten)]
        scan_options: ScanOptions,
        #[command(flatten)]
        common_options: CommonOptions,
    },

    /// Clear cache database
    ClearCache {
        #[command(flatten)]
        common_options: CommonOptions,
    },
}

#[derive(Parser, Clone)]
pub struct ScanOptions {
    /// Video file extensions to process
    #[arg(long, default_value = "mp4")]
    pub extensions: String,

    /// Include hidden files/directories
    #[arg(long)]
    pub include_hidden: bool,

    /// Regex pattern to exclude files
    #[arg(long)]
    pub exclude: Vec<String>,

    /// Number of frame samples for perceptual hashing
    #[arg(long, default_value = "10")]
    pub frame_samples: u32,

    /// MB of data to hash from head/tail of files
    #[arg(long, default_value = "16")]
    pub head_tail_mib: u32,
}

#[derive(Parser, Clone)]
pub struct CommonOptions {
    /// Database file path
    #[arg(long, default_value = "viddupe.db")]
    pub db: PathBuf,

    /// Number of parallel file processing jobs
    #[arg(long, default_value = "4")]
    pub jobs: usize,

    /// Number of parallel ffmpeg processes
    #[arg(long, default_value = "2")]
    pub ffmpeg_par: usize,

    /// Average perceptual hash distance threshold
    #[arg(long, default_value = "6")]
    pub threshold_phash_avg: u32,

    /// Maximum perceptual hash distance threshold
    #[arg(long, default_value = "12")]
    pub threshold_phash_max: u32,

    /// Minimum file size in MB to process
    #[arg(long, default_value = "1")]
    pub min_size_mb: u64,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum AutoKeepStrategy {
    /// Keep highest quality file (resolution → bitrate → codec → date)
    Best,
    /// Keep smallest file by size
    Smallest,
    /// Keep oldest file by modification time
    Oldest,
    /// Keep newest file by modification time
    Newest,
}

impl Default for AutoKeepStrategy {
    fn default() -> Self {
        Self::Best
    }
}

impl std::fmt::Display for AutoKeepStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutoKeepStrategy::Best => write!(f, "best quality"),
            AutoKeepStrategy::Smallest => write!(f, "smallest size"),
            AutoKeepStrategy::Oldest => write!(f, "oldest file"),
            AutoKeepStrategy::Newest => write!(f, "newest file"),
        }
    }
}