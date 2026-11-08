//! The pictures in a folder, its subfolders with a few of theirs, and small copies of them for a
//! grid, kept in a cache folder so a folder of 6K pictures opens quickly the second time.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What both programs show (awww has more behind build options; hyprpaper checks these).
const EXTENSIONS: [&str; 7] = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "jfif"];

/// A picture file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub path: PathBuf,
    pub name: String,
    pub modified: SystemTime,
}

impl Picture {
    /// An animated GIF moves only with awww.
    pub fn gif(&self) -> bool {
        self.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"))
    }
}

/// A folder in the folder: how many pictures it has, and its first four for its tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subfolder {
    pub path: PathBuf,
    pub name: String,
    pub count: usize,
    pub first: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folder {
    pub path: PathBuf,
    pub subfolders: Vec<Subfolder>,
    pub pictures: Vec<Picture>,
    /// Files that aren't pictures (videos, documents), for a folder that has nothing else.
    pub others: usize,
}

pub fn is_picture(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

fn hidden(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The pictures directly in `dir` (or, with `deep`, in its subfolders too, up to four down),
/// sorted by name.
fn pictures_in(dir: &Path, deep: bool, depth: usize, out: &mut Vec<Picture>, others: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| !hidden(p)).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            if deep && depth < 4 {
                pictures_in(&p, deep, depth + 1, out, others);
            }
        } else if is_picture(&p) {
            let modified = p.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(Picture { name: name(&p), path: p, modified });
        } else {
            *others += 1;
        }
    }
}

/// What's in a folder: its subfolders that have pictures (unless `deep` takes their pictures in
/// with the folder's own), and its pictures.
pub fn read_folder(dir: &Path, deep: bool) -> std::io::Result<Folder> {
    std::fs::read_dir(dir)?;
    let mut pictures = Vec::new();
    let mut others = 0;
    pictures_in(dir, deep, 0, &mut pictures, &mut others);
    let mut subfolders = Vec::new();
    if !deep {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !hidden(p))
            .collect();
        dirs.sort();
        for d in dirs {
            let mut inside = Vec::new();
            let mut skipped = 0;
            pictures_in(&d, true, 1, &mut inside, &mut skipped);
            if !inside.is_empty() {
                subfolders.push(Subfolder {
                    name: name(&d),
                    count: inside.len(),
                    first: inside.iter().take(4).map(|p| p.path.clone()).collect(),
                    path: d,
                });
            }
        }
    }
    Ok(Folder { path: dir.to_path_buf(), subfolders, pictures, others })
}

/// A picture's size in pixels, read from its header.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

/// A small copy of a picture, `width` × `height`, cropped to fill it, as a PNG in `cache`; made
/// once for each version of the file.
pub fn thumbnail(path: &Path, cache: &Path, width: u32, height: u32) -> Result<PathBuf, String> {
    let meta = path.metadata().map_err(|e| e.to_string())?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (path, meta.len(), meta.modified().ok(), width, height).hash(&mut hasher);
    let out = cache.join(format!("{:016x}.png", hasher.finish()));
    if out.exists() {
        return Ok(out);
    }
    let img = image::open(path).map_err(|e| e.to_string())?;
    let small = img.resize_to_fill(width, height, image::imageops::FilterType::Triangle);
    std::fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    // Through a temporary file, so a half-written one is never taken for done.
    let tmp = out.with_extension("tmp");
    small.save_with_format(&tmp, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_with_subfolders_and_thumbnails() {
        let d = std::env::temp_dir().join(format!("lituus-wallpaper-pictures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for sub in ["Nature", "Space/deep", "Empty", ".hidden"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        let png =
            |p: &Path, w: u32, h: u32| image::RgbImage::from_pixel(w, h, image::Rgb([10, 120, 200])).save(p).unwrap();
        png(&d.join("b.png"), 64, 36);
        png(&d.join("a.jpg"), 32, 18);
        png(&d.join("Nature/fjord.png"), 8, 8);
        png(&d.join("Space/deep/nebula.png"), 8, 8);
        png(&d.join(".hidden/x.png"), 8, 8);
        std::fs::write(d.join("clip.mp4"), "").unwrap();
        let f = read_folder(&d, false).unwrap();
        assert_eq!(f.pictures.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["a.jpg", "b.png"]);
        assert_eq!(
            f.subfolders.iter().map(|s| (s.name.as_str(), s.count)).collect::<Vec<_>>(),
            [("Nature", 1), ("Space", 1)]
        );
        assert_eq!(f.others, 1);
        let deep = read_folder(&d, true).unwrap();
        assert_eq!(deep.pictures.len(), 4);
        assert!(deep.subfolders.is_empty());
        assert_eq!(dimensions(&d.join("b.png")), Some((64, 36)));
        let cache = d.join("cache");
        let t = thumbnail(&d.join("b.png"), &cache, 32, 18).unwrap();
        assert_eq!(dimensions(&t), Some((32, 18)));
        // Made once.
        assert_eq!(thumbnail(&d.join("b.png"), &cache, 32, 18).unwrap(), t);
        assert!(thumbnail(&d.join("clip.mp4"), &cache, 32, 18).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
