mod cli;
mod scan;
mod meta;
mod coarse;
mod phash;
mod chroma;
mod db;
mod cluster;
mod interact;
mod progress;
mod util;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use log::{info, warn, error};
use std::process;
use tokio::runtime::Runtime;

fn main() {
    // Parse command line arguments
    let cli = Cli::parse();

    // Setup logging
    setup_logging(cli.debug, cli.quiet);

    // Check external tool availability
    if let Err(e) = util::check_external_tools() {
        error!("External tool check failed: {}", e);
        process::exit(1);
    }

    // Create tokio runtime for async operations
    let rt = Runtime::new().expect("Failed to create tokio runtime");

    // Execute command
    let result = rt.block_on(async {
        match &cli.command {
            Commands::Scan { root, scan_options, common_options } => {
                execute_scan(root, scan_options, common_options).await
            }
            Commands::Dupes { root, details, common_options } => {
                execute_dupes(root, *details, common_options).await
            }
            Commands::Prune { 
                root, apply, hard_delete, auto_keep, no_confirm, confirm_each, 
                scan_options, common_options 
            } => {
                execute_prune(
                    root, *apply, *hard_delete, auto_keep.clone(), 
                    *no_confirm, *confirm_each, scan_options, common_options
                ).await
            }
            Commands::ClearCache { common_options } => {
                execute_clear_cache(common_options).await
            }
        }
    });

    if let Err(e) = result {
        error!("Command failed: {}", e);
        process::exit(1);
    }
}

fn setup_logging(debug: bool, quiet: bool) {
    let log_level = if quiet {
        log::LevelFilter::Warn
    } else if debug {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    env_logger::Builder::from_default_env()
        .filter_level(log_level)
        .format_timestamp_secs()
        .init();

    if debug {
        info!("Debug logging enabled");
    }
}

async fn execute_scan(
    root: &std::path::Path,
    scan_options: &cli::ScanOptions,
    common_options: &cli::CommonOptions,
) -> Result<()> {
    info!("Scanning directory: {}", root.display());
    
    // Open/create database
    let db_conn = db::open_database(&common_options.db).await?;
    
    // Configure scan parameters
    let scan_config = scan::ScanConfig {
        extensions: parse_extensions(&scan_options.extensions),
        include_hidden: scan_options.include_hidden,
        exclude_patterns: scan_options.exclude.clone(),
        min_size_bytes: common_options.min_size_mb * 1024 * 1024,
    };
    
    // Discover video files
    let files = scan::discover_video_files(root, &scan_config)?;
    info!("Found {} video files", files.len());
    
    if files.is_empty() {
        warn!("No video files found matching criteria");
        return Ok(());
    }
    
    // Create progress bar
    let progress = progress::create_scan_progress(files.len());
    
    // Process files in parallel with progress tracking
    let processed = util::process_files_parallel(
        files,
        &db_conn,
        scan_options,
        common_options,
        progress,
    ).await?;
    
    info!("Successfully processed {} files", processed);
    Ok(())
}

async fn execute_dupes(
    root: &std::path::Path,
    details: bool,
    common_options: &cli::CommonOptions,
) -> Result<()> {
    info!("Analyzing duplicates in: {}", root.display());
    
    let db_conn = db::open_database(&common_options.db).await?;
    let clusters = cluster::find_duplicate_clusters(&db_conn, common_options).await?;
    
    if clusters.is_empty() {
        println!("No duplicate clusters found!");
        return Ok(());
    }
    
    println!("Found {} duplicate clusters:", clusters.len());
    
    for (i, cluster) in clusters.iter().enumerate() {
        println!("\n--- Cluster {} ({} files) ---", i + 1, cluster.files.len());
        
        if details {
            util::display_cluster_details(cluster);
        } else {
            util::display_cluster_summary(cluster);
        }
    }
    
    Ok(())
}

async fn execute_prune(
    root: &std::path::Path,
    apply: bool,
    hard_delete: bool,
    auto_keep: Option<cli::AutoKeepStrategy>,
    no_confirm: bool,
    confirm_each: bool,
    scan_options: &cli::ScanOptions,
    common_options: &cli::CommonOptions,
) -> Result<()> {
    if !apply {
        info!("DRY RUN: Use --apply to actually delete files");
    }
    
    info!("Pruning duplicates in: {}", root.display());
    
    let db_conn = db::open_database(&common_options.db).await?;
    
    // First ensure we have up-to-date scan data
    execute_scan(root, scan_options, common_options).await?;
    
    // Find duplicate clusters
    let clusters = cluster::find_duplicate_clusters(&db_conn, common_options).await?;
    
    if clusters.is_empty() {
        println!("No duplicates found to prune!");
        return Ok(());
    }
    
    // Interactive deletion process
    let deletion_plan = interact::create_deletion_plan(
        clusters,
        auto_keep,
        !no_confirm && confirm_each,
    ).await?;
    
    if deletion_plan.files_to_delete.is_empty() {
        info!("No files selected for deletion");
        return Ok(());
    }
    
    // Show final confirmation
    interact::show_deletion_summary(&deletion_plan, hard_delete);
    
    if apply {
        if !no_confirm && !interact::confirm_final_deletion()? {
            info!("Deletion cancelled by user");
            return Ok(());
        }
        
        // Execute deletions
        interact::execute_deletions(&deletion_plan, hard_delete, &common_options.db).await?;
        info!("Deletion completed successfully");
    } else {
        info!("DRY RUN: {} files would be deleted", deletion_plan.files_to_delete.len());
    }
    
    Ok(())
}

async fn execute_clear_cache(common_options: &cli::CommonOptions) -> Result<()> {
    info!("Clearing cache database: {}", common_options.db.display());
    db::clear_cache(&common_options.db).await?;
    info!("Cache cleared successfully");
    Ok(())
}

fn parse_extensions(extensions_str: &str) -> Vec<String> {
    extensions_str
        .split(',')
        .map(|ext| ext.trim().to_lowercase())
        .map(|ext| if ext.starts_with('.') { ext } else { format!(".{}", ext) })
        .collect()
}
