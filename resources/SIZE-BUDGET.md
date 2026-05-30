# Installer size budget

| OS | Budget |
|----|--------|
| macOS (dmg) | 350 MB |
| Windows (msi/MSIX) | 350 MB |
| Linux (AppImage) | 400 MB |
| Linux (deb/rpm) | 400 MB |

## Per-binary expected size (approx)

| Binary | Size |
|--------|------|
| ffmpeg + ffprobe | ~120 MB |
| whisper-cli | ~5 MB |
| ggml-tiny.bin | ~75 MB |
| yt-dlp | ~30 MB |
| rclone | ~50 MB |
| exiftool (Perl bundle) | ~25 MB |
| **Bundled total** | **~305 MB** |

App binary (Slint UI + crates, release LTO): ~25-40 MB → total stays under cap.

## Audit
```bash
cargo run --bin fetch-resources -- --audit
```
Exits with code 2 if total exceeds OS budget.
