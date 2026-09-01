//! Favicon and app-icon sets.
//!
//! The PNGs come from ffmpeg, which already scales better than anything worth
//! writing here. What ffmpeg will not do is `.ico`, and that is the whole of
//! this module: the ICO container is a six-byte header, a sixteen-byte
//! directory entry per image, and the images themselves. Since Windows Vista
//! those images may be PNGs verbatim, so there is no bitmap encoder here and
//! no image crate behind it.

use anyhow::{Result, anyhow};

/// The sizes a favicon set is expected to contain.
///
/// 180 is Apple's touch icon, 192 and 512 are the two the web app manifest
/// asks for, and the rest are the classic favicon ladder. Nothing here is
/// invented — every one of them is a size some platform looks for by name.
pub const SIZES: &[u32] = &[16, 32, 48, 64, 128, 180, 192, 256, 512];

/// What goes inside the `.ico`. Above 256 the directory entry cannot express
/// the width — the field is one byte, and 256 is written as zero — so the
/// large sizes stay PNGs beside it.
pub const ICO_SIZES: &[u32] = &[16, 32, 48, 64, 128, 256];

/// The file name for one size, which is also what the manifest snippet and the
/// `<link rel="icon">` tags will refer to.
pub fn png_name(stem: &str, size: u32) -> String {
    format!("{stem}-{size}.png")
}

/// Bundle PNGs into an `.ico`, in the order given.
///
/// Each entry is `(pixel size, PNG bytes)`. The size is what goes in the
/// directory; it is not read back out of the PNG, because the caller asked
/// ffmpeg for that size and a mismatch would mean the scale failed, which is
/// worth surfacing rather than papering over.
pub fn build_ico(images: &[(u32, Vec<u8>)]) -> Result<Vec<u8>> {
    if images.is_empty() {
        return Err(anyhow!("an icon file needs at least one image"));
    }
    if images.len() > u16::MAX as usize {
        return Err(anyhow!("too many images for one icon file"));
    }

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // 1 = icon, 2 = cursor
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());

    // Every image sits after the whole directory, so the first offset is known
    // before any of the entries are written.
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, body) in images {
        // 256 is stored as 0: the field is a byte, and 256 does not fit in one.
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(dim); // width
        out.push(dim); // height
        out.push(0); // palette size, 0 for truecolour
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += body.len() as u32;
    }
    for (_, body) in images {
        out.extend_from_slice(body);
    }
    Ok(out)
}

/// Read the PNGs back off disk and write the `.ico` beside them.
///
/// A size whose PNG is missing is skipped rather than fatal: ffmpeg failing on
/// one scale should still leave a usable icon file with the other five in it.
pub fn write_ico(dir: &str, stem: &str, out: &str) -> Result<usize> {
    let mut images: Vec<(u32, Vec<u8>)> = Vec::new();
    for size in ICO_SIZES {
        let path = format!("{}/{}", dir.trim_end_matches('/'), png_name(stem, *size));
        if let Ok(body) = std::fs::read(&path) {
            images.push((*size, body));
        }
    }
    let count = images.len();
    std::fs::write(out, build_ico(&images)?)?;
    Ok(count)
}

/// The markup someone has to paste into a page for the set to be used. Written
/// beside the icons, because a folder of PNGs with no `<link>` tags is a
/// puzzle rather than a favicon.
pub fn html_snippet(stem: &str) -> String {
    format!(
        "<link rel=\"icon\" href=\"/{stem}.ico\" sizes=\"any\">\n\
         <link rel=\"icon\" type=\"image/png\" href=\"/{a}\" sizes=\"32x32\">\n\
         <link rel=\"apple-touch-icon\" href=\"/{b}\">\n",
        a = png_name(stem, 32),
        b = png_name(stem, 180),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(n: usize) -> Vec<u8> {
        // Not a real PNG — the container never looks inside one.
        vec![0x89, b'P', b'N', b'G', n as u8]
    }

    #[test]
    fn the_directory_points_at_the_images_it_describes() {
        let ico = build_ico(&[(16, png(1)), (32, png(2))]).unwrap();
        assert_eq!(&ico[0..2], &[0, 0]);
        assert_eq!(&ico[2..4], &[1, 0]); // type 1
        assert_eq!(&ico[4..6], &[2, 0]); // two images

        // First entry: 16x16, five bytes, starting after 6 + 2*16 = 38.
        assert_eq!(ico[6], 16);
        assert_eq!(ico[7], 16);
        let size = u32::from_le_bytes(ico[14..18].try_into().unwrap());
        let offset = u32::from_le_bytes(ico[18..22].try_into().unwrap());
        assert_eq!(size, 5);
        assert_eq!(offset, 38);
        assert_eq!(
            &ico[offset as usize..offset as usize + 5],
            png(1).as_slice()
        );

        // Second entry starts where the first one ends.
        let offset2 = u32::from_le_bytes(ico[34..38].try_into().unwrap());
        assert_eq!(offset2, 43);
        assert_eq!(&ico[offset2 as usize..], png(2).as_slice());
    }

    #[test]
    fn two_hundred_and_fifty_six_is_written_as_zero() {
        let ico = build_ico(&[(256, png(1))]).unwrap();
        assert_eq!(ico[6], 0);
        assert_eq!(ico[7], 0);
    }

    #[test]
    fn an_icon_with_no_images_is_refused_rather_than_written_empty() {
        assert!(build_ico(&[]).is_err());
    }

    #[test]
    fn the_snippet_names_files_that_are_actually_in_the_set() {
        let html = html_snippet("logo");
        assert!(html.contains(&png_name("logo", 32)));
        assert!(html.contains(&png_name("logo", 180)));
        assert!(SIZES.contains(&180));
    }
}
