# app_flutter

The Tulipix desktop app. The Rust half is `crates/tulipix-bridge`, reached
through flutter_rust_bridge.

## First run

Codegen and the desktop runner scaffolding are build steps, not committed
artefacts — a fresh checkout does not analyze or compile until they have run
once:

```
just scaffold          # linux/ windows/ macos/ runners
just install-codegen   # CLI, version-matched to the crate
just integrate         # cargokit wiring for the bridge
just gen               # lib/src/rust/** + frb_generated.rs
```

Then, always through the sandbox:

```
just dev
```

**Never run a dev build against the real data directory.** Section schemas
apply on open, silently and one-directionally: a dev build that opens the real
`photos.db` migrates it forward under the installed app. `dev` redirects
`XDG_DATA_HOME` / `XDG_CONFIG_HOME` / `XDG_CACHE_HOME` at a throwaway copy
seeded with `cp -n`.

## Layout

```
lib/design/tokens.dart      design tokens: colours, radii, section hues
lib/shell/                  window, sidebar, title row, lock screen
lib/sections/<name>/        one folder per section
lib/src/rust/               generated — not committed, not hand-edited
```
