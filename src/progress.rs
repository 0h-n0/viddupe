use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use std::time::Duration;
use log::debug;

/// Create a progress bar for file scanning
pub fn create_scan_progress(total: usize) -> ProgressBar {
    let pb = ProgressBar::new(total as u64);
    
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} files ({per_sec}) | ETA: {eta} | {msg}"
        )
        .unwrap()
        .progress_chars("#>-")
    );
    
    pb.set_message("Scanning files...");
    pb.enable_steady_tick(Duration::from_millis(100));
    
    debug!("Created scan progress bar for {} files", total);
    pb
}

/// Create a progress bar for metadata extraction
pub fn create_metadata_progress(total: usize) -> ProgressBar {
    let pb = ProgressBar::new(total as u64);
    
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.yellow} [{elapsed_precise}] [{wide_bar:.yellow/orange}] {pos}/{len} metadata | ETA: {eta} | {msg}"
        )
        .unwrap()
        .progress_chars("=>-")
    );
    
    pb.set_message("Extracting metadata...");
    pb.enable_steady_tick(Duration::from_millis(100));
    
    debug!("Created metadata progress bar for {} files", total);
    pb
}

/// Create a progress bar for hash computation
pub fn create_hash_progress(total: usize, hash_type: &str) -> ProgressBar {
    let pb = ProgressBar::new(total as u64);
    
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.magenta} [{elapsed_precise}] [{wide_bar:.magenta/purple}] {pos}/{len} hashes ({per_sec}) | ETA: {eta} | {msg}"
        )
        .unwrap()
        .progress_chars("=>#")
    );
    
    pb.set_message(format!("Computing {} hashes...", hash_type));
    pb.enable_steady_tick(Duration::from_millis(100));
    
    debug!("Created {} hash progress bar for {} files", hash_type, total);
    pb
}

/// Create a progress bar for perceptual hash computation
pub fn create_phash_progress(total: usize) -> ProgressBar {
    create_hash_progress(total, "perceptual")
}

/// Create a progress bar for coarse hash computation  
pub fn create_coarse_hash_progress(total: usize) -> ProgressBar {
    create_hash_progress(total, "coarse")
}

/// Create a progress bar for chromaprint computation
pub fn create_chromaprint_progress(total: usize) -> ProgressBar {
    let pb = ProgressBar::new(total as u64);
    
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.blue} [{elapsed_precise}] [{wide_bar:.blue/cyan}] {pos}/{len} fingerprints | ETA: {eta} | {msg}"
        )
        .unwrap()
        .progress_chars("=>-")
    );
    
    pb.set_message("Computing audio fingerprints...");
    pb.enable_steady_tick(Duration::from_millis(100));
    
    debug!("Created chromaprint progress bar for {} files", total);
    pb
}

/// Create a progress bar for cluster analysis
pub fn create_cluster_progress(total: usize) -> ProgressBar {
    let pb = ProgressBar::new(total as u64);
    
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.green/yellow}] {pos}/{len} comparisons | ETA: {eta} | {msg}"
        )
        .unwrap()
        .progress_chars("=>-")
    );
    
    pb.set_message("Finding duplicate clusters...");
    pb.enable_steady_tick(Duration::from_millis(100));
    
    debug!("Created cluster progress bar for {} comparisons", total);
    pb
}

/// Create a progress bar for file deletion
pub fn create_deletion_progress(total: usize) -> ProgressBar {
    let pb = ProgressBar::new(total as u64);
    
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.red} [{elapsed_precise}] [{wide_bar:.red/yellow}] {pos}/{len} files | ETA: {eta} | {msg}"
        )
        .unwrap()
        .progress_chars("=>-")
    );
    
    pb.set_message("Deleting files...");
    pb.enable_steady_tick(Duration::from_millis(100));
    
    debug!("Created deletion progress bar for {} files", total);
    pb
}

/// Multi-progress manager for complex operations
pub struct MultiStageProgress {
    multi: MultiProgress,
    bars: Vec<ProgressBar>,
}

impl MultiStageProgress {
    /// Create a new multi-stage progress manager
    pub fn new() -> Self {
        Self {
            multi: MultiProgress::new(),
            bars: Vec::new(),
        }
    }
    
    /// Add a new progress bar for a stage
    pub fn add_stage(&mut self, total: usize, stage_name: &str, style_template: &str) -> usize {
        let pb = ProgressBar::new(total as u64);
        
        pb.set_style(
            ProgressStyle::with_template(style_template)
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("#>-")
        );
        
        pb.set_message(format!("{} (0/{})", stage_name, total));
        pb.enable_steady_tick(Duration::from_millis(100));
        
        let pb = self.multi.add(pb);
        let index = self.bars.len();
        self.bars.push(pb);
        
        debug!("Added progress stage '{}' with {} items", stage_name, total);
        index
    }
    
    /// Update progress for a specific stage
    pub fn update_stage(&self, stage_index: usize, progress: u64, message: Option<&str>) {
        if let Some(pb) = self.bars.get(stage_index) {
            pb.set_position(progress);
            if let Some(msg) = message {
                pb.set_message(msg.to_string());
            }
        }
    }
    
    /// Increment progress for a specific stage
    pub fn inc_stage(&self, stage_index: usize, delta: u64) {
        if let Some(pb) = self.bars.get(stage_index) {
            pb.inc(delta);
        }
    }
    
    /// Finish a specific stage
    pub fn finish_stage(&self, stage_index: usize, message: &str) {
        if let Some(pb) = self.bars.get(stage_index) {
            pb.finish_with_message(message.to_string());
        }
    }
    
    /// Finish all stages
    pub fn finish_all(&self, message: &str) {
        for pb in &self.bars {
            pb.finish_with_message(message.to_string());
        }
    }
}

impl Default for MultiStageProgress {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a comprehensive multi-stage progress for full analysis pipeline
pub fn create_full_analysis_progress(
    file_count: usize,
    frame_samples: u32,
) -> (MultiStageProgress, Vec<usize>) {
    let mut multi_progress = MultiStageProgress::new();
    
    let mut stage_indices = Vec::new();
    
    // Stage 1: File scanning
    stage_indices.push(multi_progress.add_stage(
        file_count,
        "File Discovery",
        "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} files | {msg}"
    ));
    
    // Stage 2: Metadata extraction
    stage_indices.push(multi_progress.add_stage(
        file_count,
        "Metadata Extraction", 
        "{spinner:.yellow} [{elapsed_precise}] [{wide_bar:.yellow/orange}] {pos}/{len} metadata | {msg}"
    ));
    
    // Stage 3: Coarse hashing
    stage_indices.push(multi_progress.add_stage(
        file_count,
        "Coarse Hashing",
        "{spinner:.blue} [{elapsed_precise}] [{wide_bar:.blue/cyan}] {pos}/{len} coarse hashes | {msg}"
    ));
    
    // Stage 4: Perceptual hashing
    stage_indices.push(multi_progress.add_stage(
        file_count * frame_samples as usize,
        "Perceptual Hashing",
        "{spinner:.magenta} [{elapsed_precise}] [{wide_bar:.magenta/purple}] {pos}/{len} frame hashes | {msg}"
    ));
    
    // Stage 5: Audio fingerprinting
    stage_indices.push(multi_progress.add_stage(
        file_count,
        "Audio Fingerprinting",
        "{spinner:.red} [{elapsed_precise}] [{wide_bar:.red/pink}] {pos}/{len} fingerprints | {msg}"
    ));
    
    // Stage 6: Clustering
    let comparison_count = (file_count * (file_count - 1)) / 2; // n choose 2 
    stage_indices.push(multi_progress.add_stage(
        comparison_count,
        "Duplicate Detection",
        "{spinner:.green} [{elapsed_precise}] [{wide_bar:.green/yellow}] {pos}/{len} comparisons | {msg}"
    ));
    
    (multi_progress, stage_indices)
}

/// Update progress with current file information
pub fn set_current_file(pb: &ProgressBar, file_path: &std::path::Path) {
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    
    pb.set_message(format!("Processing: {}", file_name));
}

/// Create a simple spinner for indeterminate progress
pub fn create_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈")
    );
    
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    
    pb
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_progress_bar_creation() {
        let pb = create_scan_progress(100);
        assert_eq!(pb.length(), Some(100));
        pb.finish();
    }

    #[test] 
    fn test_multi_stage_progress() {
        let mut multi = MultiStageProgress::new();
        
        let stage1 = multi.add_stage(50, "Test Stage 1", "{pos}/{len} | {msg}");
        let stage2 = multi.add_stage(25, "Test Stage 2", "{pos}/{len} | {msg}");
        
        multi.inc_stage(stage1, 10);
        multi.update_stage(stage2, 5, Some("Testing"));
        
        multi.finish_stage(stage1, "Stage 1 complete");
        multi.finish_stage(stage2, "Stage 2 complete");
        
        // Just ensure no panics occur
        assert_eq!(multi.bars.len(), 2);
    }

    #[test]
    fn test_spinner_creation() {
        let spinner = create_spinner("Testing...");
        thread::sleep(Duration::from_millis(50));
        spinner.finish_with_message("Test complete");
    }
}