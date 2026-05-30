//! HTML Web Gallery export (Picasa parity).
//!
//! Generates a static folder with:
//!   * `index.html` — gallery grid, lightbox via vanilla JS (no framework).
//!   * `thumbs/`    — 512px JPEGs copied from the existing thumb cache.
//!   * `full/`      — copy or symlink of the originals (selectable).
//!   * `data.json`  — manifest the JS reads at runtime.
//!
//! Output is fully self-contained — open `index.html` in a browser or upload
//! the whole folder to any static host.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GallerySpec {
    pub album_id: i64,
    pub title: String,
    pub out_dir: PathBuf,
    pub include_originals: bool,
    pub theme: GalleryTheme,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GalleryTheme { Light, Dark }

#[derive(Debug, Clone, Serialize)]
struct ManifestPhoto<'a> {
    id: i64,
    file: &'a str,
    thumb: &'a str,
    caption: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct Manifest<'a> {
    title: &'a str,
    photos: Vec<ManifestPhoto<'a>>,
}

pub async fn export(pool: &SqlitePool, spec: &GallerySpec) -> Result<PathBuf> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT items.id, items.abs_path
         FROM album_items
         JOIN items ON items.id = album_items.item_id
         WHERE album_items.album_id = ?
           AND items.missing_since IS NULL
         ORDER BY album_items.sort_key",
    ).bind(spec.album_id).fetch_all(pool).await?;

    std::fs::create_dir_all(spec.out_dir.join("thumbs"))?;
    if spec.include_originals { std::fs::create_dir_all(spec.out_dir.join("full"))?; }

    let mut copies: Vec<(String, String, String)> = Vec::with_capacity(rows.len());
    for (id, abs) in &rows {
        let src = Path::new(abs);
        let file_name = src.file_name().and_then(|s| s.to_str()).unwrap_or("photo").to_string();
        let thumb_name = format!("{id}.jpg");
        let thumb_dest = spec.out_dir.join("thumbs").join(&thumb_name);
        // Use the existing thumb pipeline output if cached, else copy original.
        let mtime = std::fs::metadata(src).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64).unwrap_or(0);
        let size  = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
        let cached = crate::thumbs::thumb_path(src, mtime, size, 512);
        match cached.filter(|p| p.exists()) {
            Some(c) => { std::fs::copy(&c, &thumb_dest).with_context(|| format!("copy thumb {}", c.display()))?; }
            None => { let _ = std::fs::copy(src, &thumb_dest); }
        }
        if spec.include_originals {
            let dest = spec.out_dir.join("full").join(&file_name);
            std::fs::copy(src, &dest).with_context(|| format!("copy original {}", src.display()))?;
        }
        copies.push((id.to_string(), file_name, thumb_name));
    }

    let manifest = Manifest {
        title: &spec.title,
        photos: copies.iter().map(|(id, file, thumb)| ManifestPhoto {
            id: id.parse().unwrap_or(0), file, thumb, caption: "",
        }).collect(),
    };
    let data_json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(spec.out_dir.join("data.json"), data_json)?;
    std::fs::write(spec.out_dir.join("index.html"), index_html(&spec.title, spec.theme))?;
    Ok(spec.out_dir.clone())
}

fn index_html(title: &str, theme: GalleryTheme) -> String {
    let bg = match theme { GalleryTheme::Light => "#f7f5ef", GalleryTheme::Dark => "#0d0f1a" };
    let fg = match theme { GalleryTheme::Light => "#1a1040", GalleryTheme::Dark => "#f0f4ff" };
    format!(r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width,initial-scale=1" />
<title>{title}</title>
<style>
  body {{ background: {bg}; color: {fg}; font-family: system-ui, sans-serif; margin: 0; padding: 24px; }}
  h1 {{ margin: 0 0 16px; }}
  .grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 12px; }}
  .grid a {{ display: block; aspect-ratio: 1; overflow: hidden; border-radius: 10px; }}
  .grid img {{ width: 100%; height: 100%; object-fit: cover; transition: transform .25s; }}
  .grid a:hover img {{ transform: scale(1.05); }}
  dialog#lb {{ background: rgba(0,0,0,0.92); border: 0; max-width: 100vw; max-height: 100vh; }}
  dialog#lb img {{ max-width: 90vw; max-height: 90vh; }}
</style>
</head>
<body>
<h1>{title}</h1>
<div class="grid" id="g"></div>
<dialog id="lb"><img id="lbi" alt="" /></dialog>
<script>
fetch("data.json").then(r => r.json()).then(d => {{
  const g = document.getElementById("g");
  d.photos.forEach(p => {{
    const a = document.createElement("a");
    a.href = "full/" + p.file;
    a.dataset.full = "full/" + p.file;
    a.innerHTML = '<img loading="lazy" src="thumbs/' + p.thumb + '" alt="" />';
    a.addEventListener("click", e => {{
      e.preventDefault();
      const lb = document.getElementById("lb");
      document.getElementById("lbi").src = a.dataset.full;
      lb.showModal();
    }});
    g.appendChild(a);
  }});
}});
document.getElementById("lb").addEventListener("click", e => e.currentTarget.close());
</script>
</body>
</html>
"##)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn export_writes_index_data_thumbs() {
        let (_t, pool) = open_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("p.jpg");
        image::RgbImage::from_pixel(64, 64, image::Rgb([10, 200, 50])).save(&src).unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(src.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let item_id: i64 = sqlx::query_scalar("SELECT id FROM items").fetch_one(&pool).await.unwrap();
        // create album + add the photo
        sqlx::query("INSERT INTO albums (name, created, updated) VALUES ('Trip', 0, 0)").execute(&pool).await.unwrap();
        let album_id: i64 = sqlx::query_scalar("SELECT id FROM albums").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO album_items (album_id, item_id, sort_key) VALUES (?, ?, 0)").bind(album_id).bind(item_id).execute(&pool).await.unwrap();

        let out = tmp.path().join("gallery");
        let spec = GallerySpec {
            album_id, title: "Trip".into(), out_dir: out.clone(),
            include_originals: true, theme: GalleryTheme::Dark,
        };
        export(&pool, &spec).await.unwrap();
        assert!(out.join("index.html").exists());
        assert!(out.join("data.json").exists());
        assert!(out.join("thumbs").join(format!("{item_id}.jpg")).exists());
        assert!(out.join("full").join("p.jpg").exists());
    }
}
