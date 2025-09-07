use anyhow::Result;
use chrono::Utc;
use dialoguer::{Confirm, Select};
use log::{info, warn};
use std::path::Path;

use crate::{
    cli::AutoKeepStrategy,
    cluster::{ClusterFile, DuplicateCluster},
    progress,
    scan,
};

#[derive(Debug)]
pub struct DeletionPlan {
    pub files_to_delete: Vec<DeletionTarget>,
    pub total_size_to_free: u64,
    pub clusters_processed: usize,
}

#[derive(Debug)]
pub struct DeletionTarget {
    pub file: ClusterFile,
    pub cluster_id: usize,
    pub reason: String,
}

/// Create an interactive deletion plan from duplicate clusters
pub async fn create_deletion_plan(
    clusters: Vec<DuplicateCluster>,
    auto_keep: Option<AutoKeepStrategy>,
    confirm_each: bool,
) -> Result<DeletionPlan> {
    let mut files_to_delete = Vec::new();
    let mut total_size_to_free = 0u64;

    info!("Creating deletion plan for {} clusters", clusters.len());

    for (cluster_idx, cluster) in clusters.iter().enumerate() {
        println!("\n=== Duplicate Cluster {} ===", cluster_idx + 1);
        println!("Detection: {} (confidence: {:.1}%)", 
                 cluster.detection_method, 
                 cluster.confidence_score * 100.0);
        
        // Display cluster files
        crate::util::display_cluster_summary(&cluster);
        
        let selected_deletions = if let Some(ref strategy) = auto_keep {
            // Auto-select based on strategy
            let kept_file = select_file_to_keep_auto(&cluster, strategy);
            let to_delete: Vec<&ClusterFile> = cluster.files
                .iter()
                .filter(|f| f.id != kept_file.id)
                .collect();
            
            println!("🤖 Auto-keeping: {} ({})", 
                     kept_file.path.display(), 
                     strategy);
            
            if confirm_each && !confirm_cluster_deletions(&cluster, &to_delete)? {
                continue; // Skip this cluster
            }
            
            to_delete
        } else {
            // Interactive selection
            let selected = select_files_to_delete_interactive(&cluster)?;
            if selected.is_empty() {
                continue; // User chose to keep all files
            }
            selected
        };

        // Add selected files to deletion plan
        for file in selected_deletions {
            let reason = format!("Duplicate in cluster {} ({})", 
                               cluster_idx + 1, 
                               cluster.detection_method);
            
            total_size_to_free += file.size;
            
            files_to_delete.push(DeletionTarget {
                file: file.clone(),
                cluster_id: cluster_idx,
                reason,
            });
        }
    }

    Ok(DeletionPlan {
        files_to_delete,
        total_size_to_free,
        clusters_processed: clusters.len(),
    })
}

/// Select which files to delete interactively
fn select_files_to_delete_interactive(cluster: &DuplicateCluster) -> Result<Vec<&ClusterFile>> {
    println!("\nWhich files would you like to keep? (Select multiple with space, confirm with Enter)");
    
    let mut file_options: Vec<String> = cluster.files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let quality = if let Some(recommended) = cluster.get_recommended_to_keep() {
                if f.id == recommended.id { " ⭐ (recommended)" } else { "" }
            } else { "" };
            
            format!("{}. {} ({}, {}){}", 
                   i + 1,
                   f.path.display(),
                   scan::format_size(f.size),
                   f.modified.format("%Y-%m-%d"),
                   quality)
        })
        .collect();

    file_options.push("Keep all files (skip this cluster)".to_string());
    
    let selection = Select::new()
        .with_prompt("Which file do you want to KEEP?")
        .items(&file_options)
        .default(0)
        .interact()?;

    if selection == file_options.len() - 1 {
        // User chose "keep all"
        return Ok(vec![]);
    }

    // Return all files except the selected one for keeping
    Ok(cluster.files
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != selection)
        .map(|(_, f)| f)
        .collect())
}

/// Automatically select which file to keep based on strategy
fn select_file_to_keep_auto<'a>(cluster: &'a DuplicateCluster, strategy: &AutoKeepStrategy) -> &'a ClusterFile {
    match strategy {
        AutoKeepStrategy::Best => {
            cluster.get_recommended_to_keep().unwrap_or(&cluster.files[0])
        }
        AutoKeepStrategy::Smallest => {
            cluster.files.iter().min_by_key(|f| f.size).unwrap()
        }
        AutoKeepStrategy::Oldest => {
            cluster.files.iter().min_by_key(|f| f.modified).unwrap()
        }
        AutoKeepStrategy::Newest => {
            cluster.files.iter().max_by_key(|f| f.modified).unwrap()
        }
    }
}

/// Confirm deletions for a specific cluster
fn confirm_cluster_deletions(cluster: &DuplicateCluster, to_delete: &[&ClusterFile]) -> Result<bool> {
    if to_delete.is_empty() {
        return Ok(false);
    }

    println!("\n📋 Files to be deleted from this cluster:");
    for file in to_delete {
        println!("  ❌ {} ({})", 
                 file.path.display(), 
                 scan::format_size(file.size));
    }

    let total_size: u64 = to_delete.iter().map(|f| f.size).sum();
    println!("  💾 Total space to free: {}", scan::format_size(total_size));

    Ok(Confirm::new()
        .with_prompt("Proceed with deleting these files?")
        .default(false)
        .interact()?)
}

/// Show final deletion summary
pub fn show_deletion_summary(plan: &DeletionPlan, hard_delete: bool) {
    println!("\n🗂️  DELETION SUMMARY");
    println!("=====================================");
    println!("Files to delete: {}", plan.files_to_delete.len());
    println!("Space to free: {}", scan::format_size(plan.total_size_to_free));
    println!("Clusters processed: {}", plan.clusters_processed);
    println!("Deletion method: {}", 
             if hard_delete { "⚠️  PERMANENT DELETION" } else { "🗑️  Move to trash" });

    if !plan.files_to_delete.is_empty() {
        println!("\nFiles to be deleted:");
        for (i, target) in plan.files_to_delete.iter().enumerate() {
            println!("  {}. {} ({})", 
                     i + 1,
                     target.file.path.display(),
                     scan::format_size(target.file.size));
        }
    }
}

/// Final confirmation before deletion
pub fn confirm_final_deletion() -> Result<bool> {
    println!("\n⚠️  FINAL CONFIRMATION");
    println!("This action cannot be easily undone!");
    
    Ok(Confirm::new()
        .with_prompt("Are you absolutely sure you want to proceed?")
        .default(false)
        .interact()?)
}

/// Execute the deletion plan
pub async fn execute_deletions(
    plan: &DeletionPlan,
    hard_delete: bool,
    db_path: &Path,
) -> Result<()> {
    info!("Executing deletion plan: {} files", plan.files_to_delete.len());

    let progress = progress::create_deletion_progress(plan.files_to_delete.len());
    let mut success_count = 0;
    let mut error_count = 0;

    // Create deletion log
    let log_path = "viddupe_deletions.csv";
    let mut csv_content = String::from("timestamp,action,file_path,size_bytes,cluster_id,reason,status\n");

    for target in &plan.files_to_delete {
        progress::set_current_file(&progress, &target.file.path);
        
        let result = if hard_delete {
            delete_file_permanently(&target.file.path).await
        } else {
            move_file_to_trash(&target.file.path).await
        };

        let (action, status) = match result {
            Ok(()) => {
                success_count += 1;
                (if hard_delete { "hard_delete" } else { "trash" }, "success")
            }
            Err(ref e) => {
                error_count += 1;
                warn!("Failed to delete {}: {}", target.file.path.display(), e);
                (if hard_delete { "hard_delete" } else { "trash" }, "error")
            }
        };

        // Log the action
        csv_content.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
            action,
            target.file.path.display(),
            target.file.size,
            target.cluster_id,
            target.reason,
            status
        ));

        progress.inc(1);
    }

    progress.finish_with_message("Deletion completed");

    // Write deletion log
    if let Err(e) = std::fs::write(log_path, csv_content) {
        warn!("Failed to write deletion log: {}", e);
    } else {
        info!("Deletion log written to: {}", log_path);
    }

    println!("\n✅ Deletion Results:");
    println!("  Successful: {}", success_count);
    if error_count > 0 {
        println!("  ❌ Failed: {}", error_count);
    }
    println!("  📄 Log saved to: {}", log_path);

    Ok(())
}

/// Move file to trash (safe deletion)
async fn move_file_to_trash(file_path: &Path) -> Result<()> {
    trash::delete(file_path)
        .map_err(|e| anyhow::anyhow!("Failed to move to trash: {}", e))?;
    Ok(())
}

/// Permanently delete file
async fn delete_file_permanently(file_path: &Path) -> Result<()> {
    std::fs::remove_file(file_path)
        .map_err(|e| anyhow::anyhow!("Failed to delete file: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::ClusterFile;
    use chrono::Utc;
    use std::path::PathBuf;

    fn create_test_cluster() -> DuplicateCluster {
        DuplicateCluster {
            files: vec![
                ClusterFile {
                    id: 1,
                    path: PathBuf::from("/test/file1.mp4"),
                    size: 1000000,
                    modified: Utc::now(),
                    width: Some(1920),
                    height: Some(1080),
                    video_codec: Some("h264".to_string()),
                    bitrate: Some(5000000),
                    quality_score: 75.0,
                },
                ClusterFile {
                    id: 2,
                    path: PathBuf::from("/test/file2.mp4"),
                    size: 800000,
                    modified: Utc::now(),
                    width: Some(1920),
                    height: Some(1080),
                    video_codec: Some("h265".to_string()),
                    bitrate: Some(4000000),
                    quality_score: 85.0, // Higher quality
                },
            ],
            confidence_score: 0.95,
            detection_method: "test".to_string(),
        }
    }

    #[test]
    fn test_auto_keep_strategies() {
        let cluster = create_test_cluster();

        // Best quality should pick file2 (higher quality_score)
        let best = select_file_to_keep_auto(&cluster, &AutoKeepStrategy::Best);
        assert_eq!(best.id, 2);

        // Smallest should pick file2 (smaller size)
        let smallest = select_file_to_keep_auto(&cluster, &AutoKeepStrategy::Smallest);
        assert_eq!(smallest.id, 2);

        // Newest should pick the most recently modified file
        let newest = select_file_to_keep_auto(&cluster, &AutoKeepStrategy::Newest);
        assert!(newest.id == 1 || newest.id == 2); // Both have same timestamp in test
    }

    #[test]
    fn test_deletion_plan_creation() {
        let plan = DeletionPlan {
            files_to_delete: vec![
                DeletionTarget {
                    file: create_test_cluster().files[0].clone(),
                    cluster_id: 0,
                    reason: "test deletion".to_string(),
                }
            ],
            total_size_to_free: 1000000,
            clusters_processed: 1,
        };

        assert_eq!(plan.files_to_delete.len(), 1);
        assert_eq!(plan.total_size_to_free, 1000000);
        assert_eq!(plan.clusters_processed, 1);
    }
}