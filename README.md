# Tulipix

Privacy-first, native, all-in-one media manager (Photos · Videos · Music · Books · Cloud · Tools).
Rust + Slint, zero webview. See `tulipix_plan.html` for the phased plan.

## Dev quickstart

```bash
rustup default stable
cargo install just cargo-deb cargo-wix cargo-bundle cargo-generate-rpm
just fetch          # download bundled binaries (ffmpeg, whisper.cpp, yt-dlp, rclone, exiftool)
just run            # native window, Slint UI
just run-dev        # with hot Slint reload
just trace          # with --trace-caps capability tracing
just lint           # clippy -D warnings + fmt --check
```

## Workspace layout

```
crates/
  tulipix-app/        Slint UI + main event loop (binary)
  tulipix-core/       db pool, settings, keyring, fs, ipc, prefs, caps
  tulipix-photos/     photos.db + scanner + EXIF + AI hooks
  tulipix-videos/     videos.db + scanner + TMDB/TVDB + subs
  tulipix-music/      music.db + scanner + TagLib/symphonia
  tulipix-books/      books.db + epub/cbz parser + comicvine
  tulipix-cloud/      cloud.db + rclone driver
  tulipix-player/     libmpv wrapper + GPU surface bridge
  tulipix-ai/         ONNX wrappers (CLIP, SCRFD, YOLO, LaMa, SAM, RealESRGAN, DeOldify)
  tulipix-whisper/    whisper.cpp subprocess + FIFO queue
  tulipix-sync/       optional multi-device sync client
  tulipix-platform/   per-OS code (menus, tray, notifications, vibrancy)
  tulipix-plugins/    WASM (wasmtime) + Lua (mlua) sandboxed extension host
  tulipix-cli/        headless companion CLI
tools/
  fetch-resources/    binary fetcher with SHA-256 verification
ui/                   Slint .slint files, tokens, glass/orb assets
resources/            bundled binaries + ONNX model manifest
```
