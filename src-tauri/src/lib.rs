use serde::Serialize;
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::State;
use zip::ZipArchive;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif"];
const PAGE_CACHE_ROOT: &str = "/tmp/mr-pages";

#[derive(Default)]
struct ReaderState {
    current_volume: Option<OpenedVolumeState>,
}

struct OpenedVolumeState {
    path: PathBuf,
    pages: Vec<String>,
    cache_dir: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MangaSeries {
    name: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VolumeSummary {
    name: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenedVolume {
    title: String,
    path: String,
    page_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageImage {
    image_path: String,
    page_index: usize,
    page_count: usize,
}

#[tauri::command]
fn list_manga_series(root_path: &str) -> Result<Vec<MangaSeries>, String> {
    let root = PathBuf::from(root_path);
    let mut series_directories = Vec::new();
    collect_cbz_directories(&root, &mut series_directories)
        .map_err(|error| format!("failed to scan manga library: {error}"))?;

    let mut series = series_directories
        .into_iter()
        .map(|path| MangaSeries {
            name: path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("Untitled")
                .to_owned(),
            path: path.to_string_lossy().into_owned(),
        })
        .collect::<Vec<_>>();

    series.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(series)
}

#[tauri::command]
fn list_volumes(root_path: &str) -> Result<Vec<VolumeSummary>, String> {
    let mut volumes = Vec::new();
    let entries = fs::read_dir(root_path)
        .map_err(|error| format!("failed to read volume directory: {error}"))?;

    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read volume entry: {error}"))?;
        let path = entry.path();
        if !is_cbz_path(&path) {
            continue;
        }

        let Some(name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
            continue;
        };

        volumes.push(VolumeSummary {
            name: name.to_owned(),
            path: path.to_string_lossy().into_owned(),
        });
    }

    volumes.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(volumes)
}

#[tauri::command]
fn open_volume(
    volume_path: &str,
    state: State<'_, Mutex<ReaderState>>,
) -> Result<OpenedVolume, String> {
    let path = PathBuf::from(volume_path);
    if !is_cbz_path(&path) {
        return Err("selected file is not a .cbz archive".into());
    }

    let pages = read_archive_pages(&path)?;
    if pages.is_empty() {
        return Err("the selected volume does not contain any supported image files".into());
    }

    let cache_dir = cache_dir_for_volume(&path);
    recreate_cache_dir(&cache_dir)?;

    let title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Untitled Volume")
        .to_owned();

    let opened = OpenedVolume {
        title: title.clone(),
        path: path.to_string_lossy().into_owned(),
        page_count: pages.len(),
    };

    let mut guard = state
        .lock()
        .map_err(|_| String::from("reader state is unavailable"))?;
    guard.current_volume = Some(OpenedVolumeState {
        path,
        pages,
        cache_dir,
    });

    Ok(opened)
}

#[tauri::command]
fn get_page_image(
    page_index: usize,
    state: State<'_, Mutex<ReaderState>>,
) -> Result<PageImage, String> {
    let guard = state
        .lock()
        .map_err(|_| String::from("reader state is unavailable"))?;
    let current_volume = guard
        .current_volume
        .as_ref()
        .ok_or_else(|| String::from("no volume is currently open"))?;

    if page_index >= current_volume.pages.len() {
        return Err(format!(
            "page index {page_index} is outside the current volume page range"
        ));
    }

    let archive_path = current_volume.path.clone();
    let entry_name = current_volume.pages[page_index].clone();
    let cache_dir = current_volume.cache_dir.clone();
    let page_count = current_volume.pages.len();
    drop(guard);

    let image_path = extract_page_to_cache(&archive_path, &entry_name, &cache_dir, page_index)?;

    Ok(PageImage {
        image_path: image_path.to_string_lossy().into_owned(),
        page_index,
        page_count,
    })
}

fn sanitize_filename_component(name: &str) -> String {
    name.chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect()
}

#[tauri::command]
fn default_export_directory() -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|_| String::from("HOME is not set"))?;
    let path = PathBuf::from(home).join("Pictures").join("mr-captures");
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
fn save_page_image_export(
    page_index: usize,
    destination_dir: &str,
    state: State<'_, Mutex<ReaderState>>,
) -> Result<String, String> {
    let (archive_path, entry_name, cache_dir, volume_stem) = {
        let guard = state
            .lock()
            .map_err(|_| String::from("reader state is unavailable"))?;
        let current_volume = guard
            .current_volume
            .as_ref()
            .ok_or_else(|| String::from("no volume is currently open"))?;

        if page_index >= current_volume.pages.len() {
            return Err(format!(
                "page index {page_index} is outside the current volume page range"
            ));
        }

        let stem = current_volume
            .path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("volume")
            .to_owned();

        (
            current_volume.path.clone(),
            current_volume.pages[page_index].clone(),
            current_volume.cache_dir.clone(),
            stem,
        )
    };

    let image_path = extract_page_to_cache(
        &archive_path,
        &entry_name,
        &cache_dir,
        page_index,
    )?;

    let destination = PathBuf::from(destination_dir);
    fs::create_dir_all(&destination)
        .map_err(|error| format!("failed to create export directory: {error}"))?;

    let extension = image_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("img");
    let safe_stem = sanitize_filename_component(&volume_stem);
    let filename = format!("{}_p{:04}.{}", safe_stem, page_index + 1, extension);
    let output_path = destination.join(filename);

    fs::copy(&image_path, &output_path).map_err(|error| format!("failed to save page image: {error}"))?;

    Ok(output_path.to_string_lossy().into_owned())
}

fn is_cbz_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cbz"))
}

fn is_supported_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

fn collect_cbz_directories(path: &Path, directories: &mut Vec<PathBuf>) -> io::Result<bool> {
    let mut contains_cbz_here = false;
    let mut child_contains_cbz = false;

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();

        if entry_path.is_dir() {
            if collect_cbz_directories(&entry_path, directories)? {
                child_contains_cbz = true;
            }
            continue;
        }

        if is_cbz_path(&entry_path) {
            contains_cbz_here = true;
        }
    }

    if contains_cbz_here {
        directories.push(path.to_path_buf());
    }

    Ok(contains_cbz_here || child_contains_cbz)
}

fn read_archive_pages(path: &Path) -> Result<Vec<String>, String> {
    let file = File::open(path).map_err(|error| format!("failed to open archive: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("failed to read archive: {error}"))?;
    let mut pages = Vec::new();

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("failed to inspect archive entry: {error}"))?;

        if entry.is_dir() {
            continue;
        }

        let name = entry.name().to_owned();
        if is_supported_image(&name) {
            pages.push(name);
        }
    }

    pages.sort();
    Ok(pages)
}

fn cache_dir_for_volume(volume_path: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    volume_path.hash(&mut hasher);
    let volume_hash = hasher.finish();

    PathBuf::from(PAGE_CACHE_ROOT).join(format!("volume-{volume_hash}"))
}

fn recreate_cache_dir(cache_dir: &Path) -> Result<(), String> {
    if cache_dir.exists() {
        fs::remove_dir_all(cache_dir)
            .map_err(|error| format!("failed to clear page cache directory: {error}"))?;
    }

    fs::create_dir_all(cache_dir)
        .map_err(|error| format!("failed to create page cache directory: {error}"))
}

fn extract_page_to_cache(
    archive_path: &Path,
    entry_name: &str,
    cache_dir: &Path,
    page_index: usize,
) -> Result<PathBuf, String> {
    let extension = Path::new(entry_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("img");
    let output_path = cache_dir.join(format!("page-{page_index:04}.{extension}"));

    if output_path.exists() {
        return Ok(output_path);
    }

    let file = File::open(archive_path)
        .map_err(|error| format!("failed to reopen archive for page extraction: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("failed to read archive: {error}"))?;
    let mut entry = archive
        .by_name(entry_name)
        .map_err(|error| format!("failed to locate page in archive: {error}"))?;

    let mut output_file = File::create(&output_path)
        .map_err(|error| format!("failed to create cached page image: {error}"))?;
    io::copy(&mut entry, &mut output_file)
        .map_err(|error| format!("failed to extract page image: {error}"))?;

    Ok(output_path)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(ReaderState::default()))
        .invoke_handler(tauri::generate_handler![
            list_manga_series,
            list_volumes,
            open_volume,
            get_page_image,
            default_export_directory,
            save_page_image_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
