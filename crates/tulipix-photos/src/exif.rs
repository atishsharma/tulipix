//! Read EXIF metadata via `kamadak-exif`.
//!
//! Pull a fixed set of fields out of any photo with an EXIF segment and write
//! them into `photo_meta`. No mutation, no allocations of the raw image data.

use anyhow::{Context, Result};
use exif::{In, Reader, Tag, Value};
use sqlx::SqlitePool;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifFacts {
    pub taken_at:     Option<i64>,
    pub camera_make:  Option<String>,
    pub camera_model: Option<String>,
    pub lens:         Option<String>,
    pub iso:          Option<i64>,
    pub f_number:     Option<f64>,
    pub exposure_s:   Option<f64>,
    pub focal_mm:     Option<f64>,
    pub gps_lat:      Option<f64>,
    pub gps_lon:      Option<f64>,
    pub orientation:  Option<i64>,
    pub width:        Option<i64>,
    pub height:       Option<i64>,
}

pub fn read(path: &Path) -> Result<ExifFacts> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut bufreader = BufReader::new(&file);
    let reader = Reader::new();
    let exif = match reader.read_from_container(&mut bufreader) {
        Ok(e) => e,
        Err(_) => return Ok(ExifFacts::default()), // no EXIF → empty (e.g. PNG/BMP)
    };

    let mut f = ExifFacts::default();
    f.taken_at     = parse_datetime(&exif, Tag::DateTimeOriginal)
        .or_else(|| parse_datetime(&exif, Tag::DateTime));
    f.camera_make  = str_value(&exif, Tag::Make);
    f.camera_model = str_value(&exif, Tag::Model);
    f.lens         = str_value(&exif, Tag::LensModel);
    f.iso          = exif.get_field(Tag::PhotographicSensitivity, In::PRIMARY)
        .or_else(|| exif.get_field(Tag::ISOSpeed, In::PRIMARY))
        .and_then(|x| x.value.get_uint(0))
        .map(|n| n as i64);
    f.f_number     = rational(&exif, Tag::FNumber);
    f.exposure_s   = rational(&exif, Tag::ExposureTime);
    f.focal_mm     = rational(&exif, Tag::FocalLength);
    f.orientation  = exif.get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|x| x.value.get_uint(0))
        .map(|n| n as i64);
    f.width        = exif.get_field(Tag::PixelXDimension, In::PRIMARY)
        .and_then(|x| x.value.get_uint(0)).map(|n| n as i64);
    f.height       = exif.get_field(Tag::PixelYDimension, In::PRIMARY)
        .and_then(|x| x.value.get_uint(0)).map(|n| n as i64);
    let (lat, lon) = parse_gps(&exif);
    f.gps_lat = lat;
    f.gps_lon = lon;
    Ok(f)
}

fn str_value(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Ascii(v) => {
            let bytes: Vec<u8> = v.iter().flatten().copied().collect();
            let s = String::from_utf8_lossy(&bytes).trim_end_matches('\0').trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        _ => Some(field.display_value().to_string()),
    }
}

fn rational(exif: &exif::Exif, tag: Tag) -> Option<f64> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Rational(v) => v.first().map(|r| r.to_f64()),
        Value::SRational(v) => v.first().map(|r| r.to_f64()),
        _ => None,
    }
}

fn parse_datetime(exif: &exif::Exif, tag: Tag) -> Option<i64> {
    let s = str_value(exif, tag)?;
    // EXIF stores "YYYY:MM:DD HH:MM:SS" in local time.
    let bytes = s.as_bytes();
    if bytes.len() < 19 { return None; }
    let year:  i64 = std::str::from_utf8(&bytes[0..4]).ok()?.parse().ok()?;
    let month: i64 = std::str::from_utf8(&bytes[5..7]).ok()?.parse().ok()?;
    let day:   i64 = std::str::from_utf8(&bytes[8..10]).ok()?.parse().ok()?;
    let hour:  i64 = std::str::from_utf8(&bytes[11..13]).ok()?.parse().ok()?;
    let min:   i64 = std::str::from_utf8(&bytes[14..16]).ok()?.parse().ok()?;
    let sec:   i64 = std::str::from_utf8(&bytes[17..19]).ok()?.parse().ok()?;
    Some(naive_to_unix(year, month, day, hour, min, sec))
}

/// Tiny calendar — converts a (Gregorian) UTC date to a Unix timestamp.
/// Accuracy is to the second; leap seconds are ignored, same as EXIF.
fn naive_to_unix(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> i64 {
    let days = days_from_civil(year, month, day);
    days * 86_400 + hour * 3_600 + min * 60 + sec
}

/// Howard Hinnant's days_from_civil — Unix-epoch days from y-m-d, valid for
/// the full Gregorian range. Returns negative for dates before 1970-01-01.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn parse_gps(exif: &exif::Exif) -> (Option<f64>, Option<f64>) {
    let lat = gps_dms(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef);
    let lon = gps_dms(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef);
    (lat, lon)
}

fn gps_dms(exif: &exif::Exif, val_tag: Tag, ref_tag: Tag) -> Option<f64> {
    let field = exif.get_field(val_tag, In::PRIMARY)?;
    let parts = match &field.value {
        Value::Rational(v) => v,
        _ => return None,
    };
    if parts.len() < 3 { return None; }
    let deg = parts[0].to_f64();
    let min = parts[1].to_f64();
    let sec = parts[2].to_f64();
    let mut decimal = deg + min / 60.0 + sec / 3600.0;
    let sign = exif.get_field(ref_tag, In::PRIMARY)
        .and_then(|f| match &f.value {
            Value::Ascii(v) => v.first().and_then(|b| b.first()).copied(),
            _ => None,
        })
        .map(|b| match b {
            b'S' | b'W' | b's' | b'w' => -1.0,
            _ => 1.0,
        })
        .unwrap_or(1.0);
    decimal *= sign;
    Some(decimal)
}

/// Read EXIF for one item id, write into `photo_meta`. Idempotent.
pub async fn ingest(pool: &SqlitePool, item_id: i64) -> Result<()> {
    let path: Option<String> = sqlx::query_scalar(
        "SELECT abs_path FROM items WHERE id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some(path) = path else { return Ok(()) };
    let facts = read(Path::new(&path)).unwrap_or_default();
    sqlx::query(
        "INSERT INTO photo_meta (item_id, taken_at, camera_make, camera_model, lens, iso, f_number, exposure_s, focal_mm, gps_lat, gps_lon, orientation, width, height)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET
            taken_at = excluded.taken_at,
            camera_make = excluded.camera_make,
            camera_model = excluded.camera_model,
            lens = excluded.lens,
            iso = excluded.iso,
            f_number = excluded.f_number,
            exposure_s = excluded.exposure_s,
            focal_mm = excluded.focal_mm,
            gps_lat = excluded.gps_lat,
            gps_lon = excluded.gps_lon,
            orientation = excluded.orientation,
            width = excluded.width,
            height = excluded.height",
    )
    .bind(item_id)
    .bind(facts.taken_at)
    .bind(facts.camera_make)
    .bind(facts.camera_model)
    .bind(facts.lens)
    .bind(facts.iso)
    .bind(facts.f_number)
    .bind(facts.exposure_s)
    .bind(facts.focal_mm)
    .bind(facts.gps_lat)
    .bind(facts.gps_lon)
    .bind(facts.orientation)
    .bind(facts.width)
    .bind(facts.height)
    .execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn days_from_civil_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 1, 1), 10957);
        assert_eq!(days_from_civil(2024, 2, 29), 19782); // leap year handled
    }
    #[test]
    fn missing_exif_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("not-a-real-photo.png");
        std::fs::write(&p, b"not png bytes").unwrap();
        let r = read(&p).unwrap();
        assert_eq!(r, ExifFacts::default());
    }
}
