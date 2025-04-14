extern crate clap;
extern crate indicatif;
extern crate reqwest;
extern crate serde_json;
extern crate serde;
extern crate tempfile;
extern crate walkdir;
extern crate zip;
extern crate semver;
extern crate base64;
extern crate colored;

use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write, BufReader, BufRead};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;
use walkdir::WalkDir;
use zip::read::ZipArchive;
use zip::write::FileOptions;
use zip::ZipWriter;
use semver::{Version, VersionReq};
use colored::Colorize;

// Constants for GitHub repo info and hard-coded token
const OWNER: &str = "dangujba";
const REPO: &str = "bitely-registry";
const BRANCH: &str = "main";
const GITHUB_TOKEN: &str = "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";

// We define weights (in arbitrary progress units) for each phase.
const DOWNLOAD_WEIGHT: u64 = 50;
const EXTRACTION_WEIGHT: u64 = 50;
const TOTAL_PROGRESS: u64 = DOWNLOAD_WEIGHT + EXTRACTION_WEIGHT;

#[derive(Parser)]
#[command(name = "bitely")]
#[command(about = "Bitely - EasyBite Package Manager", long_about = None)]
#[command(author = "Muhammad Baba Goni", version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Publish a package to the Bitely registry
    Publish(PublishArgs),

    /// Install a package or multiple packages
    Install(InstallArgs),

    /// Remove an installed package
    Uninstall {
        /// Name of the package
        name: String,
        /// Force uninstall even if other packages depend on it
        #[arg(long)]
        force: bool,
    },

    /// Show information about a package
    Info {
        /// Package identifier (e.g., pkg_name or pkg_name@version)
        name: String,
        /// Use cached packages only
        #[arg(long)]
        offline: bool,
    },

    /// List all available packages
    Search {
        /// Optional keyword to filter by
        keyword: Option<String>,
        /// Use cached packages only
        #[arg(long)]
        offline: bool,
    },

    /// Display current Bitely config
    Config,
}

#[derive(Args)]
struct PublishArgs {
    #[arg(short, long)]
    name: Option<String>,

    #[arg(short, long, default_value = ".")]
    path: String,

    #[arg(short, long)]
    version: Option<String>,

    #[arg(short, long)]
    description: Option<String>,

    #[arg(long)]
    dry_run: bool,
}

/// Representation of package metadata read from bitely.json.
#[derive(Debug, Deserialize)]
struct PackageMetadata {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    /// Map of dependency name to version constraint (e.g., ">=1.0.0").
    dependencies: Option<HashMap<String, String>>,
    /// Extra metadata fields.
    author: Option<String>,
    license: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
}

/// Represents an installed package in the local database.
#[derive(Serialize, Deserialize)]
struct InstalledPackage {
    name: String,
    version: String,
    dependencies: HashMap<String, String>,
}

#[derive(Args)]
struct InstallArgs {
    /// Install a specific package.
    /// Provide either a package identifier in the format pkg_name@version (e.g. my_package@1.0.0),
    /// or just a package name (e.g. my_package) to install the latest version.
    name: Option<String>,

    /// Install from a file (list of packages, one per line; each line can be pkg or pkg@version).
    #[arg(short, long)]
    file: Option<String>,

    /// Draw the complete dependency tree for the package.
    #[arg(long)]
    tree: bool,

    /// Install packages offline (use cached packages only).
    #[arg(long)]
    offline: bool,

    /// Download the package(s) without installing (save the package archive to cache/download folder).
    #[arg(long)]
    download: bool,
}

/// Returns the installation directory based on the BITE_MODULES environment variable or "modules" in current directory.
/// If the directory does not exist, it is created.
fn get_install_dir() -> PathBuf {
    let path = if let Ok(dir) = std::env::var("BITE_MODULES") {
        PathBuf::from(dir)
    } else {
        PathBuf::from("modules")
    };
    if !path.exists() {
        fs::create_dir_all(&path).expect("Failed to create modules directory");
    }
    path
}

/// Returns the local cache directory (creates it if necessary).
fn get_cache_dir() -> Result<PathBuf, Box<dyn Error>> {
    let cache_dir = PathBuf::from(".bitely_cache");
    if !cache_dir.exists() {
        fs::create_dir_all(&cache_dir)?;
    }
    Ok(cache_dir)
}

/// Returns the path to the local installed packages database.
fn get_db_path() -> Result<PathBuf, Box<dyn Error>> {
    let db_dir = get_cache_dir()?;
    Ok(db_dir.join("installed.json"))
}

/// Reads the installed packages database.
fn read_installed_db() -> Result<Vec<InstalledPackage>, Box<dyn Error>> {
    let db_path = get_db_path()?;
    if db_path.exists() {
        let file = File::open(&db_path)?;
        let packages: Vec<InstalledPackage> = serde_json::from_reader(file)?;
        Ok(packages)
    } else {
        Ok(Vec::new())
    }
}

/// Writes to the installed packages database.
fn write_installed_db(packages: &[InstalledPackage]) -> Result<(), Box<dyn Error>> {
    let db_path = get_db_path()?;
    let file = File::create(&db_path)?;
    serde_json::to_writer_pretty(file, packages)?;
    Ok(())
}

/// Archives a directory into a ZIP file and returns the ZIP archive as a Vec<u8>.
fn archive_directory(dir: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut buffer: Vec<u8> = Vec::new();
    {
        let cursor = Cursor::new(&mut buffer);
        let mut zip = ZipWriter::new(cursor);
        let options = FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o755);

        for entry in WalkDir::new(dir) {
            let entry = entry?;
            let path = entry.path();
            let name = path.strip_prefix(dir)?.to_string_lossy().replace("\\", "/");

            if path.is_file() {
                zip.start_file(name, options)?;
                let mut f = File::open(path)?;
                io::copy(&mut f, &mut zip)?;
            } else if path.is_dir() && !name.is_empty() {
                zip.add_directory(name, options)?;
            }
        }
        zip.finish()?;
    }
    Ok(buffer)
}

/// Lists all available versions for a package from the registry or cache.
fn list_package_versions(package_name: &str, offline: bool) -> Result<Vec<Version>, Box<dyn Error>> {
    let mut versions = Vec::new();

    if offline {
        let cache_dir = get_cache_dir()?;
        for entry in fs::read_dir(&cache_dir)? {
            let entry = entry?;
            let file_name = entry.file_name().into_string().unwrap_or_default();
            if file_name.starts_with(&format!("{}@", package_name)) && file_name.ends_with(".bitely") {
                let ver_str = &file_name[package_name.len() + 1..file_name.len() - 7];
                if let Ok(ver) = Version::parse(ver_str) {
                    versions.push(ver);
                }
            }
        }
    } else {
        let url = format!(
            "https://api.github.com/repos/{}/{}/contents/Packages?ref={}",
            OWNER, REPO, BRANCH
        );
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("bitely-cli"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", GITHUB_TOKEN))?,
        );
        let client = Client::builder().default_headers(headers).build()?;
        let resp = client.get(&url).send()?;
        if !resp.status().is_success() {
            return Err(format!("Failed to list packages in registry: {}", resp.status()).into());
        }
        let dirs: Vec<serde_json::Value> = resp.json()?;
        for item in dirs {
            if item["type"].as_str() != Some("dir") {
                continue;
            }
            let dir_name = item["name"].as_str().unwrap_or("");
            if dir_name.starts_with(package_name) {
                let ver_str = &dir_name[package_name.len()..];
                if let Ok(ver) = Version::parse(ver_str) {
                    versions.push(ver);
                }
            }
        }
    }

    versions.sort_by(|a, b| b.cmp(a)); // Sort descending (highest first)
    Ok(versions)
}

/// Lists all available packages from the registry or cache.
fn list_all_packages(offline: bool) -> Result<Vec<String>, Box<dyn Error>> {
    let mut packages = HashSet::new();

    if offline {
        let cache_dir = get_cache_dir()?;
        for entry in fs::read_dir(&cache_dir)? {
            let entry = entry?;
            let file_name = entry.file_name().into_string().unwrap_or_default();
            if file_name.ends_with(".bitely") && file_name.contains('@') {
                let pkg_name = file_name.split('@').next().unwrap_or("");
                if !pkg_name.is_empty() {
                    packages.insert(pkg_name.to_string());
                }
            }
        }
    } else {
        let url = format!(
            "https://api.github.com/repos/{}/{}/contents/Packages?ref={}",
            OWNER, REPO, BRANCH
        );
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("bitely-cli"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", GITHUB_TOKEN))?,
        );
        let client = Client::builder().default_headers(headers).build()?;
        let resp = client.get(&url).send()?;
        if !resp.status().is_success() {
            return Err(format!("Failed to list packages in registry: {}", resp.status()).into());
        }
        let dirs: Vec<serde_json::Value> = resp.json()?;
        for item in dirs {
            if item["type"].as_str() != Some("dir") {
                continue;
            }
            let dir_name = item["name"].as_str().unwrap_or("");
            // Extract package name by removing version part (e.g., "bitwise1.0.0" -> "bitwise")
            let pkg_name: String = dir_name.chars().take_while(|c| c.is_alphabetic()).collect();
            if !pkg_name.is_empty() {
                packages.insert(pkg_name);
            }

        }
    }

    let mut result: Vec<String> = packages.into_iter().collect();
    result.sort();
    Ok(result)
}

/// Resolves a compatible version for a package based on the version constraint.
fn resolve_version(package_name: &str, version_constraint: &str, offline: bool) -> Result<String, Box<dyn Error>> {
    let req = if version_constraint.is_empty() {
        VersionReq::parse("*")?
    } else {
        VersionReq::parse(version_constraint)?
    };
    let versions = list_package_versions(package_name, offline)?;
    for ver in versions {
        if req.matches(&ver) {
            return Ok(ver.to_string());
        }
    }
    Err(format!("No compatible version found for {} with constraint {}", package_name, version_constraint).into())
}

/// Draws the dependency tree by extracting package metadata from the zip archive.
/// Recursively fetches dependencies from the registry (or cache if offline).
fn draw_dependency_tree(package_name: &str, version: &str, indent: usize, visited: &mut HashSet<String>, offline: bool) -> Result<(), Box<dyn Error>> {
    let id = format!("{}@{}", package_name, version);
    if visited.contains(&id) {
        println!("{:indent$}- {} (cycle)", "", id.white(), indent = indent);
        return Ok(());
    }
    visited.insert(id.clone());
    println!("{:indent$}- {}", "", id.white(), indent = indent);

    // Fetch package zip (using cache or online).
    let file_data = fetch_package_data_with_progress(package_name, version, offline, &ProgressBar::hidden())?;
    let reader = Cursor::new(file_data);
    let mut zip_archive = ZipArchive::new(reader)?;
    let mut metadata_json = String::new();
    if let Ok(mut file) = zip_archive.by_name("bitely.json") {
        file.read_to_string(&mut metadata_json)?;
        let meta: PackageMetadata = serde_json::from_str(&metadata_json)?;
        if let Some(deps) = meta.dependencies {
            for (dep_name, dep_version_constraint) in deps {
                // Resolve the dependency version based on the constraint.
                let dep_version = resolve_version(&dep_name, &dep_version_constraint, offline)?;
                draw_dependency_tree(&dep_name, &dep_version, indent + 2, visited, offline)?;
            }
        }
    }
    Ok(())
}

/// Fetches package data either from cache or from GitHub while updating the provided unified progress bar.
/// The progress bar's range for the download phase is [0, DOWNLOAD_WEIGHT].
fn fetch_package_data_with_progress(package_name: &str, version: &str, offline: bool, pb: &ProgressBar) -> Result<Vec<u8>, Box<dyn Error>> {
    let cache_dir = get_cache_dir()?;
    let cache_file = cache_dir.join(format!("{}@{}.bitely", package_name, version));

    if offline {
        if cache_file.exists() {
            let metadata = fs::metadata(&cache_file)?;
            let total_size = metadata.len();
            let mut file = File::open(&cache_file)?;
            let mut data = Vec::with_capacity(total_size as usize);
            let mut buffer = [0; 8192];
            while let Ok(n) = file.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..n]);
                pb.inc((n as u64 * DOWNLOAD_WEIGHT) / total_size);
            }
            pb.set_position(DOWNLOAD_WEIGHT);
            pb.set_message("Loaded from cache");
            return Ok(data);
        } else {
            return Err("Offline mode enabled but package is not in cache".into());
        }
    }

    // Online: fetch from GitHub API.
    let pkg_id = format!("{}{}", package_name, version);
    let filename = format!("{}.bitely", pkg_id);
    let repo_path = format!("Packages/{}/{}", pkg_id, filename);
    let url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
        OWNER, REPO, repo_path, BRANCH
    );

    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("bitely-cli"));
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", GITHUB_TOKEN))?);
    let client = Client::builder().default_headers(headers).build()?;
    let mut resp = client.get(&url).send()?;

    if !resp.status().is_success() {
        return Err(format!("Failed to fetch package: {}", resp.status()).into());
    }

    let total_size = resp.content_length().unwrap_or(0);
    let mut downloaded = Vec::with_capacity(total_size as usize);
    let mut buffer = [0; 8192];
    let mut total_read: u64 = 0;
    while let Ok(n) = resp.read(&mut buffer) {
        if n == 0 {
            break;
        }
        downloaded.extend_from_slice(&buffer[..n]);
        total_read += n as u64;
        if total_size > 0 {
            let progress = (total_read * DOWNLOAD_WEIGHT) / total_size;
            pb.set_position(progress);
        }
    }
    pb.set_position(DOWNLOAD_WEIGHT);

    // GitHub returns a JSON response with base64-encoded content.
    let resp_text = String::from_utf8(downloaded)?;
    let resp_json: serde_json::Value = serde_json::from_str(&resp_text)?;
    let encoded_content = resp_json["content"]
        .as_str()
        .ok_or("Missing content field in response")?
        .replace("\n", "");
    let file_data = base64::decode(&encoded_content)?;

    // Write to cache for future offline use.
    fs::write(&cache_file, &file_data)?;
    Ok(file_data)
}

/// Fetches package metadata for a given package and version.
fn fetch_package_metadata(package_name: &str, version: &str, offline: bool) -> Result<PackageMetadata, Box<dyn Error>> {
    let file_data = fetch_package_data_with_progress(package_name, version, offline, &ProgressBar::hidden())?;
    let reader = Cursor::new(file_data);
    let mut zip_archive = ZipArchive::new(reader)?;

    // Separate the borrow
    let file_result = zip_archive.by_name("bitely.json");

    let mut metadata_json = String::new();
    match file_result {
        Ok(mut file) => {
            file.read_to_string(&mut metadata_json)?;
            let meta: PackageMetadata = serde_json::from_str(&metadata_json)?;
            Ok(meta)
        },
        Err(_) => Err("bitely.json not found in package archive".into()),
    }
}

/// Resolves and installs dependencies recursively, then installs the main package.
/// Uses a unified progress bar for both download and extraction phases.
fn install_package(package_input: &str, download_only: bool, offline: bool, tree: bool) -> Result<(), Box<dyn Error>> {
    // Parse package identifier: either "pkg" (latest version) or "pkg@version".
    let (package_name, version) = if package_input.contains('@') {
        let parts: Vec<&str> = package_input.split('@').collect();
        if parts.len() != 2 {
            return Err("Invalid package identifier format. Use pkg or pkg@version.".into());
        }
        (parts[0].to_string(), parts[1].to_string())
    } else {
        let pkg_name = package_input.to_string();
        let ver = resolve_version(&pkg_name, "*", offline)?;
        (pkg_name, ver)
    };

    // Validate version format.
    Version::parse(&version).map_err(|e| format!("Invalid version {}: {}", version, e))?;

    // If tree flag is specified, draw dependency tree and exit.
    if tree {
        println!("{}", format!("Dependency tree for {}@{}:", package_name, version).white());
        let mut visited = HashSet::new();
        draw_dependency_tree(&package_name, &version, 0, &mut visited, offline)?;
        return Ok(());
    }

    // Create a unified progress bar for the package and its dependencies.
    let pb = ProgressBar::new(TOTAL_PROGRESS);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} - {msg}")
            .unwrap(),
    );

    // Resolve and install dependencies recursively.
    let mut visited = HashSet::new();
    let dependencies = install_dependencies(&package_name, &version, offline, &pb, &mut visited)?;

    // Skip main package installation if download_only is true (dependencies are still fetched).
    if download_only {
        pb.finish_with_message("Package downloaded");
        println!("{}", format!("Package {}@{} downloaded to cache", package_name, version).green());
        return Ok(());
    }

    // Install the main package.
    pb.set_message(format!("Installing {}@{}", package_name, version));
    install_single_package(&package_name, &version, offline, &pb)?;

    // Update the installed packages database.
    let mut installed = read_installed_db()?;
    installed.retain(|pkg| pkg.name != package_name); // Remove old entry if exists.
    installed.push(InstalledPackage {
        name: package_name.clone(),
        version: version.clone(),
        dependencies,
    });
    write_installed_db(&installed)?;

    pb.finish_with_message("Installation complete");
    println!("{}", format!("Package {}@{} installed successfully", package_name, version).green());
    Ok(())
}

/// Installs a single package (without its dependencies).
fn install_single_package(package_name: &str, version: &str, offline: bool, pb: &ProgressBar) -> Result<(), Box<dyn Error>> {
    pb.set_message(format!("Downloading {}@{}", package_name, version));
    let file_data = fetch_package_data_with_progress(package_name, version, offline, pb)?;

    // Proceed to extraction.
    pb.set_message(format!("Extracting {}@{}", package_name, version));
    let install_root = get_install_dir();
    fs::create_dir_all(&install_root)?;
    // Extract into a folder named by the package (without the version appended).
    let install_dir = install_root.join(package_name);
    if install_dir.exists() {
        fs::remove_dir_all(&install_dir)?;
    }
    fs::create_dir_all(&install_dir)?;

    let reader = Cursor::new(file_data);
    let mut zip_archive = ZipArchive::new(reader)?;
    let total_files = zip_archive.len() as u64;
    let per_file = if total_files > 0 { EXTRACTION_WEIGHT / total_files } else { EXTRACTION_WEIGHT };

    for i in 0..zip_archive.len() {
        let mut file = zip_archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => install_dir.join(path),
            None => continue,
        };
        if file.name().ends_with('/') {
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(parent) = outpath.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let mut outfile = File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;
        }
        pb.inc(per_file);
    }
    Ok(())
}

/// Recursively installs dependencies for a package, returning the resolved dependencies.
fn install_dependencies(package_name: &str, version: &str, offline: bool, pb: &ProgressBar, visited: &mut HashSet<String>) -> Result<HashMap<String, String>, Box<dyn Error>> {
    let id = format!("{}@{}", package_name, version);
    if visited.contains(&id) {
        return Ok(HashMap::new());
    }
    visited.insert(id);

    // Fetch package data to get metadata.
    let file_data = fetch_package_data_with_progress(package_name, version, offline, &ProgressBar::hidden())?;
    let reader = Cursor::new(file_data);
    let mut zip_archive = ZipArchive::new(reader)?;
    let mut metadata_json = String::new();
    let mut resolved_deps = HashMap::new();
    if let Ok(mut file) = zip_archive.by_name("bitely.json") {
        file.read_to_string(&mut metadata_json)?;
        let meta: PackageMetadata = serde_json::from_str(&metadata_json)?;
        if let Some(deps) = meta.dependencies {
            for (dep_name, dep_version_constraint) in deps {
                // Resolve the dependency version based on the constraint.
                let dep_version = resolve_version(&dep_name, &dep_version_constraint, offline)?;
                // Recursively install dependencies first.
                let sub_deps = install_dependencies(&dep_name, &dep_version, offline, pb, visited)?;
                // Install the dependency package.
                pb.set_message(format!("Installing dependency {}@{}", dep_name, dep_version));
                install_single_package(&dep_name, &dep_version, offline, pb)?;
                // Record the resolved dependency.
                resolved_deps.insert(dep_name.clone(), dep_version.clone());
                // Update the installed packages database for the dependency.
                let mut installed = read_installed_db()?;
                installed.retain(|pkg| pkg.name != dep_name);
                installed.push(InstalledPackage {
                    name: dep_name.clone(),
                    version: dep_version,
                    dependencies: sub_deps,
                });
                write_installed_db(&installed)?;
            }
        }
    }
    Ok(resolved_deps)
}

/// Handles the install command, processing either a single package or a file with multiple packages.
fn handle_install(args: &InstallArgs) -> Result<(), Box<dyn Error>> {
    if let Some(file_path) = &args.file {
        let content = fs::read_to_string(file_path)?;
        for line in content.lines() {
            let pkg = line.trim();
            if !pkg.is_empty() {
                install_package(pkg, args.download, args.offline, args.tree)?;
            }
        }
    } else if let Some(pkg) = &args.name {
        install_package(pkg, args.download, args.offline, args.tree)?;
    } else {
        return Err("Please specify a package or use --file".into());
    }
    Ok(())
}

/// Prompts the user for confirmation and returns true if they confirm with 'y' or 'Y'.
fn confirm_action(prompt: &str) -> bool {
    print!("{}", format!("{} [y/N]: ", prompt).white());
    io::stdout().flush().unwrap();
    let mut input = String::new();
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin);
    reader.read_line(&mut input).unwrap();
    matches!(input.trim().to_lowercase().as_str(), "y")
}

/// Handles the uninstall command by removing the package's installation directory, checking for dependencies.
fn handle_uninstall(package_name: &str, force: bool) -> Result<(), Box<dyn Error>> {
    let install_root = get_install_dir();
    let install_dir = install_root.join(package_name);

    // Check if the package is installed.
    if !install_dir.exists() {
        println!("{}", format!("Package {} is not installed at {}", package_name, install_dir.display()).white());
        return Ok(());
    }

    // Read the installed packages database.
    let installed = read_installed_db()?;

    // Check if other packages depend on this one, unless forced.
    if !force {
        for pkg in &installed {
            if pkg.name == package_name {
                continue;
            }
            for (dep_name, _) in &pkg.dependencies {
                if dep_name == package_name {
                    eprintln!("{}", format!(
                        "Cannot uninstall {}: it is a dependency of {}@{}",
                        package_name, pkg.name, pkg.version
                    ).red());
                    return Err(format!(
                        "Cannot uninstall {}: it is a dependency of {}@{}",
                        package_name, pkg.name, pkg.version
                    ).into());
                }
            }
        }
    }

    // Prompt for confirmation.
    let prompt = format!("Are you sure you want to uninstall {}?", package_name);
    if !confirm_action(&prompt) {
        println!("{}", "Uninstall cancelled.".white());
        return Ok(());
    }

    // Remove the installation directory.
    fs::remove_dir_all(&install_dir)?;
    println!("{}", format!("Package {} uninstalled successfully from {}", package_name, install_dir.display()).green());

    // Update the installed packages database.
    let mut updated = installed;
    updated.retain(|pkg| pkg.name != package_name);
    write_installed_db(&updated)?;

    Ok(())
}

/// Handles the info command, displaying details about a package.
fn handle_info(package_input: &str, offline: bool) -> Result<(), Box<dyn Error>> {
    // Parse package identifier: either "pkg" (latest version) or "pkg@version".
    let (package_name, version) = if package_input.contains('@') {
        let parts: Vec<&str> = package_input.split('@').collect();
        if parts.len() != 2 {
            return Err("Invalid package identifier format. Use pkg or pkg@version.".into());
        }
        (parts[0].to_string(), parts[1].to_string())
    } else {
        let pkg_name = package_input.to_string();
        let ver = resolve_version(&pkg_name, "*", offline)?;
        (pkg_name, ver)
    };

    // Validate version format.
    Version::parse(&version).map_err(|e| format!("Invalid version {}: {}", version, e))?;

    // Fetch package metadata.
    let meta = fetch_package_metadata(&package_name, &version, offline)?;

    // Check installed status.
    let installed = read_installed_db()?;
    let installed_info = installed.iter().find(|pkg| pkg.name == package_name);

    // Display package information.
    println!("{}", format!("Package: {}", package_name).white());
    println!("{}", format!("Version: {}", version).white());
    println!("{}", format!("Description: {}", meta.description.unwrap_or("N/A".to_string())).white());
    if let Some(deps) = meta.dependencies {
        println!("{}", "Dependencies:".white());
        if deps.is_empty() {
            println!("{}", "  None".white());
        } else {
            for (dep_name, constraint) in deps {
                println!("{}", format!("  - {} ({})", dep_name, constraint).white());
            }
        }
    } else {
        println!("{}", "Dependencies: None".white());
    }
    println!("{}", format!("Author: {}", meta.author.unwrap_or("N/A".to_string())).white());
    println!("{}", format!("License: {}", meta.license.unwrap_or("N/A".to_string())).white());
    println!("{}", format!("Repository: {}", meta.repository.unwrap_or("N/A".to_string())).white());
    println!("{}", format!("Homepage: {}", meta.homepage.unwrap_or("N/A".to_string())).white());
    match installed_info {
        Some(pkg) => println!("{}", format!("Installed: Yes (version {})", pkg.version).green()),
        None => println!("{}", "Installed: No".white()),
    }
    println!("{}", format!("Available Versions: {:?}", list_package_versions(&package_name, offline)?).white());

    Ok(())
}

/// Handles the search command, listing all packages or filtering by keyword.
fn handle_search(keyword: &Option<String>, offline: bool) -> Result<(), Box<dyn Error>> {
    let packages = list_all_packages(offline)?;
    if packages.is_empty() {
        println!("{}", "No packages found.".white());
        return Ok(());
    }

    // Get installed packages.
    let installed = read_installed_db()?;
    let installed_names: HashSet<String> = installed.iter().map(|pkg| pkg.name.clone()).collect();

    // Filter packages by keyword if provided.
    let filtered = match keyword {
        Some(kw) => {
            let kw_lower = kw.to_lowercase();
            packages.into_iter().filter(|pkg| pkg.to_lowercase().contains(&kw_lower)).collect::<Vec<_>>()
        }
        None => packages,
    };

    if filtered.is_empty() {
        println!("{}", format!("No packages match the keyword '{}'.", keyword.as_ref().unwrap()).white());
        return Ok(());
    }

    // Display results.
    println!("{}", format!("Found {} package(s):", filtered.len()).white());
    for pkg in filtered {
        let status = if installed_names.contains(&pkg) {
            let version = installed.iter().find(|p| p.name == pkg).unwrap().version.clone();
            format!(" (installed: {})", version).green()
        } else {
            "".white()
        };
        println!("{}", format!("- {}{}", pkg, status).white());
    }

    Ok(())
}

/// Handles the config command, displaying Bitely configuration.
fn handle_config() -> Result<(), Box<dyn Error>> {
    println!("{}", "Bitely Configuration:".white());
    println!("{}", format!("Installation Directory: {}", get_install_dir().display()).white());
    println!("{}", format!("Cache Directory: {}", get_cache_dir()?.display()).white());
    println!("{}", "Offline Mode: Not enabled (set via --offline for specific commands)".white());
    println!("{}", format!("Registry Owner: {}", OWNER).white());
    println!("{}", format!("Registry Repository: {}", REPO).white());
    println!("{}", format!("Registry Branch: {}", BRANCH).white());
    Ok(())
}

fn read_package_metadata(dir: &Path) -> Result<PackageMetadata, Box<dyn Error>> {
    let metadata_path = dir.join("bitely.json");
    let file = File::open(&metadata_path)?;
    let metadata: PackageMetadata = serde_json::from_reader(file)?;
    Ok(metadata)
}

/// Handles the publish command, checking if the version exists, packaging, and uploading to GitHub with retries.
fn handle_publish(args: &PublishArgs) -> Result<(), Box<dyn Error>> {
    // Determine the package source path
    let package_path = Path::new(&args.path);
    if !package_path.exists() {
        return Err("The provided package path does not exist.".into());
    }

    // Attempt to read metadata from bitely.json if any CLI field is missing
    let mut name = args.name.clone();
    let mut version = args.version.clone();
    let mut description = args.description.clone();
    let mut dependencies: Option<HashMap<String, String>> = None;
    let mut extra_meta: HashMap<&str, Option<String>> = HashMap::new();

    if name.is_none() || version.is_none() || description.is_none() {
        let meta = read_package_metadata(package_path)?;
        if name.is_none() {
            name = meta.name;
        }
        if version.is_none() {
            version = meta.version;
        }
        if description.is_none() {
            description = meta.description;
        }
        dependencies = meta.dependencies;
        extra_meta.insert("author", meta.author);
        extra_meta.insert("license", meta.license);
        extra_meta.insert("repository", meta.repository);
        extra_meta.insert("homepage", meta.homepage);
    }

    // Ensure that required fields have been provided
    let name = name.ok_or("Package name not provided and not found in bitely.json")?;
    let version = version.ok_or("Package version not provided and not found in bitely.json")?;

    // Validate version format
    Version::parse(&version).map_err(|e| format!("Invalid version {}: {}", version, e))?;

    // Compute pkg_id as the combination of name and version
    let pkg_id = format!("{}{}", name, version);

    // Set the filename to be used in the repository
    let original_filename = format!("{}.bitely", pkg_id);

    // Construct the repository file path: Packages/<pkg_id>/<original_filename>
    let repo_path = format!("Packages/{}/{}", pkg_id, original_filename);

    // Use the hard-coded GitHub token
    let github_token = GITHUB_TOKEN;

    // Prepare HTTP headers
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("bitely-cli"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", github_token))?,
    );

    let client = Client::builder().default_headers(headers).build()?;

    // Check if the package version already exists in the repository
    let check_url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
        OWNER, REPO, repo_path, BRANCH
    );

    let check_response = client.get(&check_url).send()?;

    if check_response.status().is_success() {
        println!("{}", format!("Package version {} already exists in the repository.", version).red());
        return Err("Package version already exists.".into());
    } else if check_response.status() == 404 {
        println!("{}", format!("Package version {} does not exist, proceeding with upload.", version).white());
    } else {
        // Handle other status codes
        let status = check_response.status();
        let text = check_response.text().unwrap_or_default();
        return Err(format!("Failed to check if package exists. Status: {}. Response: {}", status, text).into());
    }

    // Prepare file_data based on whether the input is a file or directory
    let file_data: Vec<u8>;
    if package_path.is_dir() {
        println!(
            "{}",
            format!("Archiving directory {} into {}", package_path.display(), original_filename).white()
        );
        file_data = archive_directory(package_path)?;
    } else if package_path.is_file() {
        println!("{}", format!("Using provided file: {}", package_path.display()).white());
        file_data = fs::read(package_path)?;
    } else {
        return Err("Provided package path is neither a file nor a directory.".into());
    }

    // Display a progress bar while processing the file data
    let file_size = file_data.len() as u64;
    let pb = ProgressBar::new(file_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")?,
    );
    let chunk_size = 8192;
    let mut pos = 0;
    while pos < file_data.len() {
        let step = std::cmp::min(chunk_size, file_data.len() - pos);
        pb.inc(step as u64);
        pos += step;
    }
    pb.finish_with_message("Package data ready for upload");

    // Base64-encode the file data
    let encoded_content = base64::encode(&file_data);

    // Build the JSON payload for the GitHub Contents API
    let mut payload = json!({
        "message": format!("Publish {} version {}", name, version),
        "content": encoded_content,
        "branch": BRANCH,
    });

    // Add dependencies if available
    if let Some(deps) = dependencies {
        payload["dependencies"] = json!(deps);
    }

    // Add any extra metadata fields if available
    for (key, value) in extra_meta {
        if let Some(val) = value {
            payload[key] = json!(val);
        }
    }

    // If dry_run flag is set, print out the payload and exit
    if args.dry_run {
        println!("{}", "Dry run enabled.".white());
        println!("{}", format!("The payload would be sent to path: {}", repo_path).white());
        println!("{}", format!("Payload: {}", payload).white());
        return Ok(());
    }

    // Prepare the GitHub API URL for upload
    let upload_url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}",
        OWNER, REPO, repo_path
    );

    // Set up retry parameters
    let max_retries = 3;
    let mut attempt = 0;
    let mut backoff = Duration::from_secs(2);

    loop {
        attempt += 1;
        println!("{}", format!("Attempt {} to upload...", attempt).white());
        let response = client.put(&upload_url).json(&payload).send();

        match response {
            Ok(resp) => {
                if resp.status().is_success() {
                    println!("{}", "Package published successfully!".green());
                    break;
                } else {
                    let status = resp.status();
                    let text = resp.text().unwrap_or_default();
                    eprintln!("{}", format!("Upload failed. Status: {}. Response: {}", status, text).red());
                }
            }
            Err(e) => {
                eprintln!("{}", format!("Network error on attempt {}: {}", attempt, e).red());
            }
        }

        if attempt >= max_retries {
            return Err("Exceeded maximum number of retries. Upload failed.".into());
        } else {
            println!("{}", format!("Retrying after {} seconds...", backoff.as_secs()).white());
            sleep(backoff);
            backoff *= 2; // Exponential backoff
        }
    }
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    match &cli.command {
        Some(Commands::Install(args)) => {
            if let Err(e) = handle_install(args) {
                eprintln!("{}", format!("Error installing package(s): {}", e).red());
            }
        }
        Some(Commands::Publish(args)) => {
            if let Err(e) = handle_publish(args) {
                eprintln!("{}", format!("Error publishing package: {}", e).red());
            }
        }
        Some(Commands::Uninstall { name, force }) => {
            if let Err(e) = handle_uninstall(name, *force) {
                eprintln!("{}", format!("Error uninstalling package {}: {}", name, e).red());
            }
        }
        Some(Commands::Info { name, offline }) => {
            if let Err(e) = handle_info(name, *offline) {
                eprintln!("{}", format!("Error retrieving info for package {}: {}", name, e).red());
            }
        }
        Some(Commands::Search { keyword, offline }) => {
            if let Err(e) = handle_search(keyword, *offline) {
                eprintln!("{}", format!("Error searching packages: {}", e).red());
            }
        }
        Some(Commands::Config) => {
            if let Err(e) = handle_config() {
                eprintln!("{}", format!("Error displaying config: {}", e).red());
            }
        }
        None => {
            println!("{}", "Use `bitely --help` to see available commands".white());
        }
    }
}