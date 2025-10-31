use clap::Parser;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "obsidian-cli")]
#[command(version)]
#[command(about = "A CLI tool for reading Obsidian vaults")]
#[command(after_help = "EXAMPLES:\n    \
    # List all tags with counts\n    \
    obsidian-cli --tags\n\n    \
    # Show vault statistics\n    \
    obsidian-cli --stats\n\n    \
    # List all files with metadata\n    \
    obsidian-cli --files\n\n    \
    # Find broken links\n    \
    obsidian-cli --links\n\n    \
    # Find orphaned notes\n    \
    obsidian-cli --orphans\n\n    \
    # Find notes with a specific tag\n    \
    obsidian-cli --tag writing\n\n    \
    # Show backlinks to a note\n    \
    obsidian-cli --backlinks \"My Note.md\"")]
struct Cli {
    /// Path to the Obsidian vault (defaults to current directory)
    #[arg(value_name = "VAULT_PATH")]
    #[arg(default_value = ".")]
    vault_path: PathBuf,

    /// List all tags found in the vault with occurrence counts
    #[arg(long)]
    tags: bool,

    /// Show vault statistics
    #[arg(long)]
    stats: bool,

    /// List all markdown files with metadata
    #[arg(long)]
    files: bool,

    /// List all links and show broken links
    #[arg(long)]
    links: bool,

    /// Find orphaned notes (notes with no incoming or outgoing links)
    #[arg(long)]
    orphans: bool,

    /// Find notes containing a specific tag
    #[arg(long, value_name = "TAG")]
    tag: Option<String>,

    /// Show which notes link to a specific note
    #[arg(long, value_name = "FILE")]
    backlinks: Option<String>,
}

#[derive(Serialize)]
struct TagCount {
    tag: String,
    count: usize,
}

#[derive(Serialize)]
struct TagsOutput {
    tags: Vec<TagCount>,
}

#[derive(Serialize)]
struct StatsOutput {
    total_notes: usize,
    total_tags: usize,
    total_links: usize,
    broken_links: usize,
    orphaned_notes: usize,
}

#[derive(Serialize)]
struct FileInfo {
    path: String,
    word_count: usize,
    link_count: usize,
    tag_count: usize,
    modified: String,
}

#[derive(Serialize)]
struct FilesOutput {
    files: Vec<FileInfo>,
}

#[derive(Serialize)]
struct LinkInfo {
    source: String,
    target: String,
    exists: bool,
}

#[derive(Serialize)]
struct LinksOutput {
    links: Vec<LinkInfo>,
    broken_count: usize,
}

#[derive(Serialize)]
struct OrphansOutput {
    orphans: Vec<String>,
}

#[derive(Serialize)]
struct TagSearchOutput {
    tag: String,
    files: Vec<String>,
}

#[derive(Serialize)]
struct BacklinksOutput {
    file: String,
    backlinks: Vec<String>,
}

fn extract_tags_from_file(content: &str) -> Vec<String> {
    let mut tags = Vec::new();

    // Match inline tags like #tag or #tag/subtag
    let inline_tag_regex = Regex::new(r"(?:^|\s)#([a-zA-Z0-9_/-]+)").unwrap();
    for cap in inline_tag_regex.captures_iter(content) {
        if let Some(tag) = cap.get(1) {
            tags.push(tag.as_str().to_string());
        }
    }

    // Match frontmatter tags
    if let Some(frontmatter) = extract_frontmatter(content) {
        if let Some(fm_tags) = parse_frontmatter_tags(&frontmatter) {
            tags.extend(fm_tags);
        }
    }

    tags
}

fn extract_frontmatter(content: &str) -> Option<String> {
    if content.starts_with("---\n") {
        if let Some(end_pos) = content[4..].find("\n---\n") {
            return Some(content[4..4 + end_pos].to_string());
        }
    }
    None
}

fn parse_frontmatter_tags(frontmatter: &str) -> Option<Vec<String>> {
    let mut tags = Vec::new();

    for line in frontmatter.lines() {
        let line = line.trim();

        // Match "tags: tag1" or "tags: [tag1, tag2]"
        if line.starts_with("tags:") {
            let tags_part = line.strip_prefix("tags:").unwrap().trim();

            // Handle array format [tag1, tag2]
            if tags_part.starts_with('[') && tags_part.ends_with(']') {
                let tags_str = &tags_part[1..tags_part.len() - 1];
                for tag in tags_str.split(',') {
                    let tag = tag.trim().trim_matches('"').trim_matches('\'');
                    if !tag.is_empty() {
                        tags.push(tag.to_string());
                    }
                }
            } else if !tags_part.is_empty() {
                // Handle single tag format
                let tag = tags_part.trim_matches('"').trim_matches('\'');
                tags.push(tag.to_string());
            }
        }
        // Handle list format
        else if line.starts_with("- ") && !tags.is_empty() {
            let tag = line.strip_prefix("- ").unwrap().trim().trim_matches('"').trim_matches('\'');
            if !tag.is_empty() {
                tags.push(tag.to_string());
            }
        }
    }

    if tags.is_empty() {
        None
    } else {
        Some(tags)
    }
}

fn extract_links_from_file(content: &str) -> Vec<String> {
    let mut links = Vec::new();

    // Match [[link]] and [[link|alias]]
    let link_regex = Regex::new(r"\[\[([^\]|]+)(?:\|[^\]]*)?\]\]").unwrap();
    for cap in link_regex.captures_iter(content) {
        if let Some(link) = cap.get(1) {
            links.push(link.as_str().to_string());
        }
    }

    links
}

fn normalize_path(_vault_path: &Path, note_path: &str) -> String {
    // Remove .md extension if present for comparison
    let normalized = if note_path.ends_with(".md") {
        &note_path[..note_path.len() - 3]
    } else {
        note_path
    };
    normalized.to_string()
}

fn find_note_path(vault_path: &Path, link: &str, all_notes: &HashSet<String>) -> Option<String> {
    // Try exact match first
    let link_normalized = normalize_path(vault_path, link);

    for note in all_notes {
        let note_normalized = normalize_path(vault_path, note);

        // Check if the link matches the note name (with or without path)
        if note_normalized == link_normalized || note_normalized.ends_with(&format!("/{}", link_normalized)) {
            return Some(note.clone());
        }
    }

    None
}

fn collect_all_tags(vault_path: &PathBuf) -> Result<BTreeMap<String, usize>, String> {
    let mut tag_counts = BTreeMap::new();

    for entry in WalkDir::new(vault_path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        // Only process markdown files
        if path.is_file() && path.extension().map_or(false, |ext| ext == "md") {
            match fs::read_to_string(path) {
                Ok(content) => {
                    let tags = extract_tags_from_file(&content);
                    for tag in tags {
                        *tag_counts.entry(tag).or_insert(0) += 1;
                    }
                }
                Err(_) => {
                    continue;
                }
            }
        }
    }

    Ok(tag_counts)
}

fn collect_all_files(vault_path: &PathBuf) -> Result<Vec<FileInfo>, String> {
    let mut files = Vec::new();

    for entry in WalkDir::new(vault_path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.is_file() && path.extension().map_or(false, |ext| ext == "md") {
            match fs::read_to_string(path) {
                Ok(content) => {
                    let word_count = content.split_whitespace().count();
                    let links = extract_links_from_file(&content);
                    let tags = extract_tags_from_file(&content);

                    let relative_path = path.strip_prefix(vault_path)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .to_string();

                    let modified = if let Ok(metadata) = fs::metadata(path) {
                        if let Ok(modified) = metadata.modified() {
                            format!("{:?}", modified)
                        } else {
                            "unknown".to_string()
                        }
                    } else {
                        "unknown".to_string()
                    };

                    files.push(FileInfo {
                        path: relative_path,
                        word_count,
                        link_count: links.len(),
                        tag_count: tags.len(),
                        modified,
                    });
                }
                Err(_) => {
                    continue;
                }
            }
        }
    }

    Ok(files)
}

fn collect_all_links(vault_path: &PathBuf) -> Result<(Vec<LinkInfo>, HashSet<String>), String> {
    let mut all_links = Vec::new();
    let mut all_notes = HashSet::new();

    // First pass: collect all note paths
    for entry in WalkDir::new(vault_path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() && path.extension().map_or(false, |ext| ext == "md") {
            let relative_path = path.strip_prefix(vault_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            all_notes.insert(relative_path);
        }
    }

    // Second pass: collect all links
    for entry in WalkDir::new(vault_path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.is_file() && path.extension().map_or(false, |ext| ext == "md") {
            match fs::read_to_string(path) {
                Ok(content) => {
                    let source = path.strip_prefix(vault_path)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .to_string();

                    let links = extract_links_from_file(&content);
                    for link in links {
                        let target_path = find_note_path(vault_path, &link, &all_notes);
                        let exists = target_path.is_some();
                        let target = target_path.unwrap_or(link);

                        all_links.push(LinkInfo {
                            source: source.clone(),
                            target,
                            exists,
                        });
                    }
                }
                Err(_) => {
                    continue;
                }
            }
        }
    }

    Ok((all_links, all_notes))
}

fn find_orphans(vault_path: &PathBuf) -> Result<Vec<String>, String> {
    let (links, all_notes) = collect_all_links(vault_path)?;

    let mut has_outgoing = HashSet::new();
    let mut has_incoming = HashSet::new();

    for link in &links {
        has_outgoing.insert(link.source.clone());
        if link.exists {
            has_incoming.insert(link.target.clone());
        }
    }

    let orphans: Vec<String> = all_notes
        .iter()
        .filter(|note| !has_outgoing.contains(*note) && !has_incoming.contains(*note))
        .cloned()
        .collect();

    Ok(orphans)
}

fn find_notes_with_tag(vault_path: &PathBuf, target_tag: &str) -> Result<Vec<String>, String> {
    let mut matching_files = Vec::new();

    for entry in WalkDir::new(vault_path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.is_file() && path.extension().map_or(false, |ext| ext == "md") {
            match fs::read_to_string(path) {
                Ok(content) => {
                    let tags = extract_tags_from_file(&content);
                    if tags.iter().any(|t| t == target_tag) {
                        let relative_path = path.strip_prefix(vault_path)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .to_string();
                        matching_files.push(relative_path);
                    }
                }
                Err(_) => {
                    continue;
                }
            }
        }
    }

    Ok(matching_files)
}

fn find_backlinks(vault_path: &PathBuf, target_file: &str) -> Result<Vec<String>, String> {
    let (links, _all_notes) = collect_all_links(vault_path)?;

    // Normalize the target file path
    let target_normalized = normalize_path(vault_path, target_file);

    let mut backlinks = Vec::new();

    for link in links {
        let link_target_normalized = normalize_path(vault_path, &link.target);

        // Check if this link points to our target file
        if link_target_normalized == target_normalized ||
           link_target_normalized.ends_with(&format!("/{}", target_normalized)) ||
           target_normalized.ends_with(&format!("/{}", link_target_normalized)) {
            backlinks.push(link.source);
        }
    }

    backlinks.sort();
    backlinks.dedup();

    Ok(backlinks)
}

fn calculate_stats(vault_path: &PathBuf) -> Result<StatsOutput, String> {
    let tag_counts = collect_all_tags(vault_path)?;
    let (links, all_notes) = collect_all_links(vault_path)?;
    let orphans = find_orphans(vault_path)?;

    let broken_links = links.iter().filter(|l| !l.exists).count();

    Ok(StatsOutput {
        total_notes: all_notes.len(),
        total_tags: tag_counts.len(),
        total_links: links.len(),
        broken_links,
        orphaned_notes: orphans.len(),
    })
}

fn main() {
    let cli = Cli::parse();

    if cli.tags {
        match collect_all_tags(&cli.vault_path) {
            Ok(tag_counts) => {
                let tags: Vec<TagCount> = tag_counts
                    .into_iter()
                    .map(|(tag, count)| TagCount { tag, count })
                    .collect();
                let output = TagsOutput { tags };
                match serde_json::to_string_pretty(&output) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Error serializing to JSON: {}", e),
                }
            }
            Err(e) => eprintln!("Error collecting tags: {}", e),
        }
    } else if cli.stats {
        match calculate_stats(&cli.vault_path) {
            Ok(stats) => {
                match serde_json::to_string_pretty(&stats) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Error serializing to JSON: {}", e),
                }
            }
            Err(e) => eprintln!("Error calculating stats: {}", e),
        }
    } else if cli.files {
        match collect_all_files(&cli.vault_path) {
            Ok(files) => {
                let output = FilesOutput { files };
                match serde_json::to_string_pretty(&output) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Error serializing to JSON: {}", e),
                }
            }
            Err(e) => eprintln!("Error collecting files: {}", e),
        }
    } else if cli.links {
        match collect_all_links(&cli.vault_path) {
            Ok((links, _)) => {
                let broken_count = links.iter().filter(|l| !l.exists).count();
                let output = LinksOutput { links, broken_count };
                match serde_json::to_string_pretty(&output) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Error serializing to JSON: {}", e),
                }
            }
            Err(e) => eprintln!("Error collecting links: {}", e),
        }
    } else if cli.orphans {
        match find_orphans(&cli.vault_path) {
            Ok(orphans) => {
                let output = OrphansOutput { orphans };
                match serde_json::to_string_pretty(&output) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Error serializing to JSON: {}", e),
                }
            }
            Err(e) => eprintln!("Error finding orphans: {}", e),
        }
    } else if let Some(tag) = &cli.tag {
        match find_notes_with_tag(&cli.vault_path, tag) {
            Ok(files) => {
                let output = TagSearchOutput {
                    tag: tag.clone(),
                    files,
                };
                match serde_json::to_string_pretty(&output) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Error serializing to JSON: {}", e),
                }
            }
            Err(e) => eprintln!("Error finding notes with tag: {}", e),
        }
    } else if let Some(file) = &cli.backlinks {
        match find_backlinks(&cli.vault_path, file) {
            Ok(backlinks) => {
                let output = BacklinksOutput {
                    file: file.clone(),
                    backlinks,
                };
                match serde_json::to_string_pretty(&output) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Error serializing to JSON: {}", e),
                }
            }
            Err(e) => eprintln!("Error finding backlinks: {}", e),
        }
    } else {
        // Default: show stats
        match calculate_stats(&cli.vault_path) {
            Ok(stats) => {
                match serde_json::to_string_pretty(&stats) {
                    Ok(json) => println!("{}", json),
                    Err(e) => eprintln!("Error serializing to JSON: {}", e),
                }
            }
            Err(e) => eprintln!("Error calculating stats: {}", e),
        }
    }
}
