# app_flutter

The Dart half of the Flutter port. See `docs/flutter-port-plan.html` for the
plan this follows, and `docs/flutter-port-parity.md` for what does not match the
Slint build yet.

Nothing in here is on `beta`. Nothing in here edits a file that is.

## First run

Codegen and the desktop runner scaffolding are build steps, not committed
artefacts — a fresh checkout does not analyze or compile until they have run
once:

```
just --justfile justfile.flutter scaffold          # linux/ windows/ macos/ runners
just --justfile justfile.flutter install-codegen   # CLI, version-matched to the crate
just --justfile justfile.flutter integrate         # cargokit wiring for the bridge
just --justfile justfile.flutter gen               # lib/src/rust/** + frb_generated.rs
```

Then, always through the sandbox:

```
just --justfile justfile.flutter dev
```

**Never run the Flutter build against the real data directory.** Section schemas
apply on open, silently and one-directionally: a Flutter build that opens the
real `photos.db` migrates it forward under the shipping Slint app. `dev`
redirects `XDG_DATA_HOME` / `XDG_CONFIG_HOME` / `XDG_CACHE_HOME` at a throwaway
copy seeded with `cp -n`.

Side by side with the Slint build on the same seeded copy:

```
just --justfile justfile.flutter slint-sandboxed
```

## Layout

```
lib/design/tokens.dart      ui/tokens.slint, transcribed literally
lib/sections/photos/        ports ui/page_photos.slint + tulipix-sec-photos
lib/src/rust/               generated — not committed, not hand-edited
```
