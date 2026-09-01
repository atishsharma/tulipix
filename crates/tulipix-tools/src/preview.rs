//! Preview planner — the sibling of [`crate::exec::plan`].
//!
//! `plan()` answers "what commands does this job run"; this answers "what is
//! that job about to do to my files", for a form that is still being filled in.
//! It lives beside the planner rather than in the bridge for one reason: a
//! preview that disagrees with the job it previews is worse than no preview,
//! and the only way to keep them honest is to have them read the same code.
//!
//! Nothing here spawns a process or writes a file. A preview that needs
//! either is *declared* here as a [`RenderPlan`] and executed by the caller,
//! which is what keeps the budget, the cancellation and the process count in
//! one place instead of scattered through the planner.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::exec::Step;

// ------------------------------------------------------------------ budget ---

/// What stops a preview becoming the job.
///
/// A parameter rather than a set of constants, so a future "preview at full
/// quality" toggle costs nothing.
#[derive(Debug, Clone)]
pub struct Budget {
    /// Dry-run rows returned. The rest are counted, not listed.
    pub rows: usize,
    /// Subtitle cues returned.
    pub cues: usize,
    /// Files visited by a directory walk, across the whole preview.
    pub walk: usize,
    /// Page chips drawn for a document. Past a few hundred it is a texture,
    /// not a list.
    pub pages: usize,
    /// Longest edge of a rendered preview.
    pub max_px: u32,
    /// Where rendered artifacts land. An empty path means this caller cannot
    /// run anything, so only the pure previews are planned.
    pub cache: PathBuf,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            rows: 200,
            cues: 120,
            walk: 20_000,
            pages: 400,
            max_px: 1280,
            cache: PathBuf::new(),
        }
    }
}

// --------------------------------------------------------------------- data ---

/// What a row means, which is all the UI needs to colour it. The words go in
/// `note`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Plain,
    Add,
    Remove,
    Change,
    Warn,
}

impl RowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RowKind::Plain => "plain",
            RowKind::Add => "add",
            RowKind::Remove => "remove",
            RowKind::Change => "change",
            RowKind::Warn => "warn",
        }
    }
}

/// One line of a dry run. `right` is empty for the lists that are just a list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub left: String,
    pub right: String,
    pub note: String,
}

impl Row {
    fn plain(left: impl Into<String>, right: impl Into<String>) -> Row {
        Row {
            kind: RowKind::Plain,
            left: left.into(),
            right: right.into(),
            note: String::new(),
        }
    }
}

/// One subtitle cue, in milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// One page of a document: whether the operation keeps it, and how far it ends
/// up turned.
#[derive(Debug, Clone, PartialEq)]
pub struct PageMark {
    pub page: u32,
    /// False for a page the operation removes.
    pub kept: bool,
    /// Degrees this page ends up turned by, on top of what it already carried.
    pub turned: i64,
}

/// The answer, flattened. `kind` is the discriminator the UI switches on; the
/// collections not named by it are empty.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewData {
    /// `dryrun` | `cues` | `waiting` | `none`.
    pub kind: &'static str,
    pub title: String,
    /// One line under the title: a count, a caveat, a warning.
    pub note: String,
    pub rows: Vec<Row>,
    pub cues: Vec<Cue>,
    pub pages: Vec<PageMark>,
    /// Items the budget cut. `rows.len() + more` is the real total.
    pub more: usize,
}

impl PreviewData {
    fn empty(kind: &'static str) -> PreviewData {
        PreviewData {
            kind,
            title: String::new(),
            note: String::new(),
            rows: Vec::new(),
            cues: Vec::new(),
            pages: Vec::new(),
            more: 0,
        }
    }

    /// A field the preview needs is still blank. Says which one.
    pub fn waiting(note: impl Into<String>) -> PreviewData {
        PreviewData {
            note: note.into(),
            ..PreviewData::empty("waiting")
        }
    }

    /// This operation has no preview planned yet.
    pub fn none() -> PreviewData {
        PreviewData::empty("none")
    }

    /// A [`RenderPlan`] that the caller has finished executing. The artifact
    /// itself travels beside this, not in it.
    pub fn rendered(kind: &'static str, note: impl Into<String>) -> PreviewData {
        PreviewData {
            note: note.into(),
            ..PreviewData::empty(kind)
        }
    }

    fn dryrun(
        title: impl Into<String>,
        rows: Vec<Row>,
        more: usize,
        note: impl Into<String>,
    ) -> PreviewData {
        PreviewData {
            title: title.into(),
            note: note.into(),
            rows,
            more,
            ..PreviewData::empty("dryrun")
        }
    }
}

/// What one ffprobe said about a source. Filled in by the caller and cached
/// per file, so a slider drag costs no probes at all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    pub duration_s: f64,
    pub width: u32,
    pub height: u32,
    pub v_codec: String,
    pub a_codec: String,
    pub bytes: u64,
}

impl Probe {
    pub fn has_video(&self) -> bool {
        !self.v_codec.is_empty()
    }

    pub fn dims(&self) -> String {
        if self.width == 0 {
            String::new()
        } else {
            format!("{}×{}", self.width, self.height)
        }
    }
}

/// A small artifact in the preview cache, and the commands that make it.
///
/// The commands come from the job's own `exec::plan` wherever they can, run
/// against a pre-scaled frame instead of the source. That is the whole trick:
/// the preview and the job cannot disagree about what the operation does,
/// because they are the same argv with a smaller picture in it.
pub struct RenderPlan {
    /// `image` — a picture to show. `wave` — raw mono PCM the caller turns
    /// into peaks.
    pub kind: &'static str,
    /// What the last step writes. Empty `steps` means it is already there.
    pub out: String,
    /// The untouched side of a before/after. Empty when the render stands
    /// alone, as it does for a thumbnail or a contact sheet.
    pub before: String,
    pub steps: Vec<Step>,
    pub note: String,
}

/// How a preview gets made.
pub enum PreviewPlan {
    /// Computed here and now. No subprocess.
    Pure(PreviewData),
    /// One or more short commands into the preview cache.
    Render(RenderPlan),
}

// ------------------------------------------------------------------ planner ---

/// Plan the preview for a kind and the form's current (often incomplete) spec.
///
/// Never errors and never panics on a half-filled form: an operation with
/// nothing to say yet answers `waiting`, one with no preview written answers
/// `none`, and the UI shows what it can.
pub fn plan_preview(
    kind: &str,
    spec: &Value,
    budget: &Budget,
    probe: Option<&Probe>,
) -> PreviewPlan {
    match kind {
        // Answered here and now.
        "rename" => pure(rename_preview(spec, budget)),
        "folder_diff" => pure(folder_diff_preview(spec, budget)),
        "hash" => pure(hash_preview(spec)),
        "merge" => pure(merge_preview(spec)),
        "split" => pure(split_preview(spec, probe)),
        "cache_clean" => pure(cache_clean_preview(budget)),
        "mirror" => pure(mirror_preview(spec, budget)),
        "archive_create" => pure(archive_create_preview(spec, budget)),
        "archive_extract" | "archive_repack" => pure(archive_read_preview(spec, budget)),
        "pdf_pages" | "pdf_delete" | "pdf_rotate" | "pdf_stamp" => {
            pure(pages_preview(kind, spec, budget))
        }
        "burn_subs" | "add_subs" | "subs_sync" => pure(cue_preview(spec, "sub", budget)),
        "convert" | "audio_convert" => pure(convert_preview(spec, probe)),
        "doc_convert" | "ebook_convert" => pure(document_preview(spec)),
        "stems" => pure(stems_preview(spec)),

        // Answered by a short render into the cache.
        "compress_photo" | "watermark" | "resize" | "denoise" | "image_convert" => {
            image_render(kind, spec, budget, probe)
        }
        "crop" | "rotate" => geometry_render(kind, spec, budget, probe),
        "compress_video" => video_render(spec, budget, probe),
        "thumbnail" => job_render(kind, spec, budget, "The frame it will save."),
        "contact_sheet" => sheet_render(spec, budget, probe),
        "trim" => trim_render(spec, budget),
        "compress_audio" | "normalize" | "extract" | "denoise_audio" => wave_render(spec, budget),

        _ => pure(PreviewData::none()),
    }
}

fn pure(data: PreviewData) -> PreviewPlan {
    PreviewPlan::Pure(data)
}

/// Does this caller have somewhere to put a render? Tests and the CLI do not.
fn can_render(b: &Budget) -> bool {
    !b.cache.as_os_str().is_empty()
}

fn rename_preview(spec: &Value, b: &Budget) -> PreviewData {
    let Some(dir) = text(spec, "dir") else {
        return PreviewData::waiting("Choose a folder to see what gets renamed.");
    };
    let Some(pattern) = text(spec, "pattern") else {
        return PreviewData::waiting("Type a pattern to see the new names.");
    };
    let start = number(spec, "start").unwrap_or(1.0).max(0.0) as usize;

    // The same listing the job uses, in the same order — `{n}` must number the
    // preview and the run identically.
    let files = match crate::rename::scan(&dir) {
        Ok(f) => f,
        Err(e) => return PreviewData::waiting(format!("{dir}: {e}")),
    };
    let total = files.len();
    if total == 0 {
        return PreviewData::waiting("That folder has no files in it.");
    }
    let shown: Vec<_> = files.into_iter().take(b.rows).collect();
    let more = total - shown.len();

    let run = crate::rename::dry_run(&shown, &pattern, start);
    let rows = run
        .renames
        .iter()
        .map(|(from, to)| {
            let clash = run.collisions.iter().any(|c| c == to);
            Row {
                kind: if clash {
                    RowKind::Warn
                } else {
                    RowKind::Change
                },
                left: base_name(from),
                right: to.clone(),
                note: if clash {
                    "name used twice".into()
                } else {
                    String::new()
                },
            }
        })
        .collect();

    let note = if run.collisions.is_empty() {
        format!("{total} files")
    } else {
        format!(
            "{} of {total} names collide — the run is refused rather than half-applied.",
            run.collisions.len()
        )
    };
    PreviewData::dryrun("Rename", rows, more, note)
}

fn folder_diff_preview(spec: &Value, budget: &Budget) -> PreviewData {
    let (Some(dir_a), Some(dir_b)) = (text(spec, "a"), text(spec, "b")) else {
        return PreviewData::waiting("Choose both folders to see what differs.");
    };
    let left = walk_sizes(&dir_a, budget.walk);
    let right = walk_sizes(&dir_b, budget.walk);
    if left.is_empty() && right.is_empty() {
        return PreviewData::waiting("Neither folder has any files in it.");
    }

    let mut rows = Vec::new();
    for (rel, size) in &left {
        match right.get(rel) {
            None => rows.push(Row {
                kind: RowKind::Add,
                left: rel.clone(),
                right: human(*size),
                note: "only in A".into(),
            }),
            Some(other) if other != size => rows.push(Row {
                kind: RowKind::Change,
                left: rel.clone(),
                right: format!("{} → {}", human(*size), human(*other)),
                note: "different size".into(),
            }),
            Some(_) => {}
        }
    }
    for (rel, size) in &right {
        if !left.contains_key(rel) {
            rows.push(Row {
                kind: RowKind::Remove,
                left: rel.clone(),
                right: human(*size),
                note: "only in B".into(),
            });
        }
    }
    rows.sort_by(|x, y| x.left.cmp(&y.left));
    let total = rows.len();
    rows.truncate(budget.rows);
    let more = total - rows.len();

    let note = if total == 0 {
        format!("Identical by size — {} files each.", left.len())
    } else {
        // The job hashes contents; this compares sizes, because a preview that
        // reads every byte on every keystroke is not a preview.
        format!("{total} differences by size. The run compares contents.")
    };
    PreviewData::dryrun("Folder diff", rows, more, note)
}

fn hash_preview(spec: &Value) -> PreviewData {
    // `plan()` falls back to the single `input` field when the list is empty, so
    // the preview has to as well.
    let mut files = strings(spec, "files");
    if files.is_empty() {
        if let Some(one) = text(spec, "input") {
            files.push(one);
        }
    }
    if files.is_empty() {
        return PreviewData::waiting("Choose files to see what will be read.");
    }
    let mut total = 0u64;
    let rows = files
        .iter()
        .map(|f| {
            let size = file_size(f);
            total += size;
            Row::plain(base_name(f), human(size))
        })
        .collect();
    PreviewData::dryrun(
        "Hash",
        rows,
        0,
        format!("SHA-256 over {} files, {}.", files.len(), human(total)),
    )
}

fn merge_preview(spec: &Value) -> PreviewData {
    let inputs = strings(spec, "inputs");
    if inputs.is_empty() {
        return PreviewData::waiting("Choose the files to join, in the order you want them.");
    }
    let mut total = 0u64;
    let mut rows: Vec<Row> = inputs
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let size = file_size(f);
            total += size;
            Row {
                kind: RowKind::Plain,
                left: format!("{}. {}", i + 1, base_name(f)),
                right: human(size),
                note: String::new(),
            }
        })
        .collect();

    let note = if inputs.len() < 2 {
        rows.push(Row {
            kind: RowKind::Warn,
            left: "Needs a second file".into(),
            right: String::new(),
            note: "merge joins two or more".into(),
        });
        "One file is not a merge.".to_string()
    } else if extensions(&inputs).len() > 1 {
        // The copy is stream-level. Mixed containers is the failure people hit.
        rows.push(Row {
            kind: RowKind::Warn,
            left: "Mixed formats".into(),
            right: extensions(&inputs).join(", "),
            note: "streams are copied, not re-encoded".into(),
        });
        format!(
            "{} files, {} — these may not share a codec.",
            inputs.len(),
            human(total)
        )
    } else {
        format!("{} files, {} in order.", inputs.len(), human(total))
    };
    PreviewData::dryrun("Merge", rows, 0, note)
}

fn split_preview(spec: &Value, probe: Option<&Probe>) -> PreviewData {
    let Some(template) = text(spec, "output") else {
        return PreviewData::waiting(
            "Give an output template to see what the segments get called.",
        );
    };
    let every = number(spec, "every_s").unwrap_or(60.0).max(0.001);
    // How many segments there are needs the source duration, which needs a
    // probe. What they are called does not, and the template is the part people
    // get wrong.
    // With a probe the count is exact; without one, name the first few and say
    // the rest depends on how long the video turns out to be.
    let segments = probe
        .map(|p| ((p.duration_s / every).ceil() as usize).max(1))
        .unwrap_or(4);
    let shown = segments.min(12);
    let rows = (0..shown)
        .map(|i| {
            let from = i as f64 * every;
            let to = match probe {
                Some(p) if i + 1 == segments => p.duration_s,
                _ => from + every,
            };
            Row::plain(
                expand_index(&template, i),
                format!("{} – {}", clock(from), clock(to)),
            )
        })
        .collect();
    let note = if !template.contains('%') {
        "No %03d in the template — every segment writes over the last one.".to_string()
    } else if let Some(p) = probe {
        format!(
            "{segments} files of {} from {}.",
            clock(every),
            clock(p.duration_s)
        )
    } else {
        format!("One file every {}, until the video ends.", clock(every))
    };
    PreviewData::dryrun("Split", rows, segments - shown, note)
}

/// Every copy and every deletion, by name, before Start exists.
///
/// The spec asks for this by name: a destructive tool previews as a dry run
/// the user must read. It is the same `plan_mirror` the run carries out, so
/// what is listed here is exactly what happens.
fn mirror_preview(spec: &Value, b: &Budget) -> PreviewData {
    let (Some(from), Some(to)) = (text(spec, "a"), text(spec, "b")) else {
        return PreviewData::waiting("Choose both folders to see what will change.");
    };
    let delete_extra = flag(spec, "delete_extra");
    let actions = crate::folder_diff::plan_mirror(&from, &to, delete_extra, b.walk);
    if actions.is_empty() {
        return PreviewData::dryrun(
            "Mirror",
            Vec::new(),
            0,
            "Already identical — nothing to do.",
        );
    }

    let copies = actions
        .iter()
        .filter(|a| matches!(a, crate::folder_diff::MirrorAction::Copy { .. }))
        .count();
    let deletes = actions.len() - copies;
    let bytes: u64 = actions.iter().map(|a| a.bytes()).sum();
    let total = actions.len();

    let rows: Vec<Row> = actions
        .into_iter()
        .take(b.rows)
        .map(|a| match a {
            crate::folder_diff::MirrorAction::Copy { rel, bytes, .. } => Row {
                kind: RowKind::Add,
                left: rel,
                right: human(bytes),
                note: "copy".into(),
            },
            crate::folder_diff::MirrorAction::Delete { rel, bytes, .. } => Row {
                kind: RowKind::Remove,
                left: rel,
                right: human(bytes),
                note: "delete".into(),
            },
        })
        .collect();
    let more = total - rows.len();

    let note = if deletes > 0 {
        format!(
            "{copies} to copy and {deletes} to delete, {}. The deletions are permanent.",
            human(bytes)
        )
    } else {
        format!("{copies} to copy, {}. Nothing is deleted.", human(bytes))
    };
    PreviewData::dryrun("Mirror", rows, more, note)
}

/// What is about to go into an archive, from the same `members` the run uses.
fn archive_create_preview(spec: &Value, b: &Budget) -> PreviewData {
    let folder = text(spec, "folder").unwrap_or_default();
    let files = strings(spec, "files");
    if folder.trim().is_empty() && files.is_empty() {
        return PreviewData::waiting("Choose a folder, or some files, to archive.");
    }
    let members = crate::archive::members(&folder, &files, b.walk);
    if members.is_empty() {
        return PreviewData::waiting("There is nothing in there to archive.");
    }
    let total = members.len();
    // The whole weight, not just the listed part: `take` on a lazy iterator
    // would have stopped the adding as well as the listing.
    let bytes: u64 = members.iter().map(|(path, _)| file_size(path)).sum();
    let rows: Vec<Row> = members
        .iter()
        .take(b.rows)
        .map(|(path, name)| Row {
            kind: RowKind::Add,
            left: name.clone(),
            right: human(file_size(path)),
            note: String::new(),
        })
        .collect();
    let more = total - rows.len();
    let note = if folder.trim().is_empty() {
        format!("{total} files, {}.", human(bytes))
    } else {
        format!(
            "{total} files from {}, {}.",
            base_name(&folder),
            human(bytes)
        )
    };
    PreviewData::dryrun("Create archive", rows, more, note)
}

/// What is inside an archive, read from its directory without unpacking a byte.
fn archive_read_preview(spec: &Value, b: &Budget) -> PreviewData {
    let Some(input) = text(spec, "input") else {
        return PreviewData::waiting("Choose an archive to see what is in it.");
    };
    let entries = match crate::archive::list(&input) {
        Ok(entries) => entries,
        Err(e) => return PreviewData::waiting(e.to_string()),
    };
    let files: Vec<&crate::archive::Entry> = entries.iter().filter(|e| !e.is_dir).collect();
    if files.is_empty() {
        return PreviewData::waiting("That archive has no files in it.");
    }
    let unsafe_count = files.iter().filter(|e| e.safe.is_none()).count();
    let bytes: u64 = files.iter().map(|e| e.bytes).sum();
    let packed: u64 = files.iter().map(|e| e.packed).sum();
    let total = files.len();

    let rows: Vec<Row> = files
        .iter()
        .take(b.rows)
        .map(|e| Row {
            kind: if e.safe.is_some() {
                RowKind::Plain
            } else {
                RowKind::Warn
            },
            left: e.name.clone(),
            right: human(e.bytes),
            note: if e.safe.is_some() {
                String::new()
            } else {
                "skipped — this name climbs out of the folder".into()
            },
        })
        .collect();
    let more = total - rows.len();

    let note = if unsafe_count > 0 {
        format!(
            "{total} files, {} unpacked. {unsafe_count} will be skipped.",
            human(bytes)
        )
    } else {
        format!(
            "{total} files, {} packed into {}.",
            human(bytes),
            human(packed)
        )
    };
    PreviewData::dryrun(base_name(&input), rows, more, note)
}

/// Every page of a document, lit or struck through.
///
/// No thumbnails: rasterising a page needs a renderer this crate does not
/// have, and the question people actually get wrong is which pages `1-3,5,8-10`
/// means — which is a numbered strip, not a picture.
fn pages_preview(kind: &str, spec: &Value, b: &Budget) -> PreviewData {
    let Some(input) = text(spec, "input") else {
        return PreviewData::waiting("Choose a PDF to see its pages.");
    };
    let Some(count) = crate::pdf::page_count(&input) else {
        return PreviewData::waiting(format!("{}: not a PDF this can read.", base_name(&input)));
    };
    let ranges = text(spec, "ranges").unwrap_or_default();
    // Blank means every page when turning them, and means nothing at all when
    // keeping or deleting — where "everything" would be a disaster rather than
    // a shorthand.
    if ranges.trim().is_empty() && matches!(kind, "pdf_pages" | "pdf_delete") {
        return PreviewData::waiting(format!(
            "{count} pages. Name the ones you want — 1-3,5,8-10."
        ));
    }
    let chosen = crate::pdf::selected(&ranges, count);
    if chosen.is_empty() {
        return PreviewData::waiting(format!(
            "Nothing in that range is a page of this {count}-page document."
        ));
    }
    let turn = crate::pdf::normalise_turn(number(spec, "turn").unwrap_or(0.0) as i64);

    let shown = (count as usize).min(b.pages);
    let pages: Vec<PageMark> = (1..=shown as u32)
        .map(|page| {
            let named = chosen.contains(&page);
            PageMark {
                page,
                kept: match kind {
                    "pdf_delete" => !named,
                    "pdf_pages" => named,
                    _ => true,
                },
                turned: if kind == "pdf_rotate" && named {
                    turn
                } else {
                    0
                },
            }
        })
        .collect();

    let note = match kind {
        "pdf_delete" => format!("{count} pages, deleting {}.", chosen.len()),
        "pdf_pages" => format!("{count} pages, keeping {}.", chosen.len()),
        "pdf_stamp" => {
            // What it will actually say on the first page it stamps, rather
            // than the pattern the form holds.
            let pattern = text(spec, "pattern").unwrap_or_default();
            let first = chosen.first().copied().unwrap_or(1);
            format!(
                "{count} pages, stamping {} with “{}”.",
                chosen.len(),
                crate::pdf::stamp_text(&pattern, first, count)
            )
        }
        _ => format!("{count} pages, turning {} by {turn}°.", chosen.len()),
    };
    PreviewData {
        title: base_name(&input),
        note,
        more: count as usize - shown,
        pages,
        ..PreviewData::empty("pages")
    }
}

/// What goes in against what comes out, from one cached probe.
/// Documents and ebooks. Same shape as [`convert_preview`] and none of its
/// machinery: ffprobe knows nothing about a .docx, so the honest preview is the
/// name it goes in as, the name it comes out as, and how big the source is.
fn document_preview(spec: &Value) -> PreviewData {
    let Some(input) = text(spec, "input") else {
        return PreviewData::waiting("Choose a source to see what it becomes.");
    };
    let Some(target) = text(spec, "format") else {
        return PreviewData::waiting("Pick a format to convert to.");
    };
    let target = target.trim_start_matches('.').to_lowercase();
    let path = Path::new(&input);
    let from = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let mut rows = vec![Row {
        kind: RowKind::Change,
        left: from.to_uppercase(),
        right: target.to_uppercase(),
        note: "format".into(),
    }];
    let stem = path
        .file_stem()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or_default();
    rows.push(Row {
        kind: RowKind::Change,
        left: base_name(&input),
        right: format!("{stem}.{target}"),
        note: "name".into(),
    });
    let bytes = file_size(&input);
    if bytes > 0 {
        rows.push(Row::plain(human(bytes), "on disk".to_string()));
    }
    if from == target {
        rows.push(Row {
            kind: RowKind::Warn,
            left: format!("already {}", target.to_uppercase()),
            right: String::new(),
            note: "the source and the target are the same format".into(),
        });
    }
    PreviewData::dryrun("Convert", rows, 0, base_name(&input))
}

/// demucs writes a tree rather than a file, and where it lands is the question
/// people actually ask. So the preview is the tree: `<folder>/htdemucs/<track>/`
/// and one row per stem, named exactly as demucs names them.
fn stems_preview(spec: &Value) -> PreviewData {
    let Some(input) = text(spec, "input") else {
        return PreviewData::waiting("Choose a track to see where the stems land.");
    };
    let path = Path::new(&input);
    let track = path
        .file_stem()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = text(spec, "output").unwrap_or_else(|| {
        path.parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".into())
    });
    let ext = if flag(spec, "mp3") { "mp3" } else { "wav" };
    // The model name is demucs' own default and part of the path it writes.
    let root = format!("{dir}/htdemucs/{track}");
    let stems: &[&str] = if text(spec, "mode").as_deref() == Some("vocals") {
        &["vocals", "no_vocals"]
    } else {
        &["vocals", "drums", "bass", "other"]
    };
    let mut rows: Vec<Row> = stems
        .iter()
        .map(|stem| Row {
            kind: RowKind::Add,
            left: format!("{stem}.{ext}"),
            right: String::new(),
            note: root.clone(),
        })
        .collect();
    if ext == "wav" {
        rows.push(Row {
            kind: RowKind::Warn,
            left: "wav".into(),
            right: String::new(),
            note: "each stem is as long as the track and uncompressed".into(),
        });
    }
    PreviewData::dryrun("Separate stems", rows, 0, base_name(&input))
}

fn convert_preview(spec: &Value, probe: Option<&Probe>) -> PreviewData {
    let Some(input) = text(spec, "input") else {
        return PreviewData::waiting("Choose a source to see what it becomes.");
    };
    let Some(target) = text(spec, "target_ext") else {
        return PreviewData::waiting("Pick a format to convert to.");
    };
    let target = target.trim_start_matches('.').to_lowercase();
    let from = Path::new(&input)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let mut rows = vec![Row {
        kind: RowKind::Change,
        left: from.to_uppercase(),
        right: target.to_uppercase(),
        note: "container".into(),
    }];
    let Some(p) = probe else {
        return PreviewData::dryrun("Convert", rows, 0, base_name(&input));
    };

    let codec = crate::convert::default_codec(&target).unwrap_or("copied through");
    if p.has_video() {
        rows.push(Row {
            kind: RowKind::Change,
            left: p.v_codec.clone(),
            right: codec.to_string(),
            note: "video".into(),
        });
    }
    if !p.a_codec.is_empty() {
        rows.push(Row {
            kind: if crate::convert::media_of(&target) == crate::convert::Media::Audio {
                RowKind::Change
            } else {
                RowKind::Plain
            },
            left: p.a_codec.clone(),
            right: if crate::convert::media_of(&target) == crate::convert::Media::Audio {
                codec.to_string()
            } else {
                p.a_codec.clone()
            },
            note: "audio".into(),
        });
    }
    if !p.dims().is_empty() {
        rows.push(Row::plain(p.dims(), clock(p.duration_s)));
    }
    if !crate::convert::is_valid(&from, &target) {
        rows.push(Row {
            kind: RowKind::Warn,
            left: format!("{} to {}", from.to_uppercase(), target.to_uppercase()),
            right: String::new(),
            note: "not a conversion ffmpeg can make sense of".into(),
        });
    }
    PreviewData::dryrun("Convert", rows, 0, base_name(&input))
}

fn cache_clean_preview(b: &Budget) -> PreviewData {
    let Some(dir) = tulipix_core::paths::cache_dir() else {
        return PreviewData::waiting("No cache directory on this machine.");
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return PreviewData::waiting("The cache is empty.");
    };
    let mut rows = Vec::new();
    let mut total = 0u64;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let size = if path.is_dir() {
            walk_sizes(&path.to_string_lossy(), b.walk)
                .values()
                .sum::<u64>()
        } else {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        };
        total += size;
        rows.push(Row {
            kind: RowKind::Remove,
            left: base_name(&path.to_string_lossy()),
            right: human(size),
            note: String::new(),
        });
    }
    if rows.is_empty() {
        return PreviewData::waiting("The cache is already empty.");
    }
    rows.sort_by(|x, y| x.left.cmp(&y.left));
    PreviewData::dryrun("Clean cache", rows, 0, format!("{} to free.", human(total)))
}

fn cue_preview(spec: &Value, key: &str, b: &Budget) -> PreviewData {
    let Some(path) = text(spec, key) else {
        return PreviewData::waiting("Choose a subtitle file to see its cues.");
    };
    let Ok(body) = std::fs::read_to_string(&path) else {
        return PreviewData::waiting(format!("{}: cannot be read as text.", base_name(&path)));
    };
    let (cues, total) = parse_cues(&body, b.cues);
    if total == 0 {
        return PreviewData::waiting(format!("{}: no cues found in it.", base_name(&path)));
    }
    let last = cues.last().map(|c| c.end_ms).unwrap_or(0);
    PreviewData {
        title: base_name(&path),
        note: format!("{total} cues, to {}.", clock(last as f64 / 1000.0)),
        more: total - cues.len(),
        cues,
        ..PreviewData::empty("cues")
    }
}

// ------------------------------------------------------------------ renders ---

/// The picture every rendered preview starts from: one frame of the source,
/// capped to the budget's longest edge.
///
/// Cached against the source rather than the settings, so dragging a slider
/// never decodes the source twice. It is also the "before" of the before/after
/// split, which means both sides are the same geometry and the divider
/// actually compares like with like.
fn frame_prep(src: &str, at_s: f64, b: &Budget) -> (String, Vec<Step>) {
    let key = digest(&format!(
        "{}\u{1}{}\u{1}{at_s:.2}",
        source_stamp(src),
        b.max_px
    ));
    let out = cache_path(b, &format!("src-{key}.png"));
    if Path::new(&out).exists() {
        return (out, Vec::new());
    }
    let mut args: Vec<String> = Vec::new();
    // Seeking before -i is the fast one; it costs accuracy that a preview
    // frame does not need.
    if at_s > 0.25 {
        args.extend(["-ss".to_string(), format!("{at_s:.3}")]);
    }
    args.extend([
        "-i".to_string(),
        src.to_string(),
        "-frames:v".to_string(),
        "1".to_string(),
        "-vf".to_string(),
        cap_filter(b.max_px),
        out.clone(),
    ]);
    (out, vec![ffmpeg(args)])
}

/// The operations whose preview is the operation itself, run on one frame.
fn image_render(kind: &str, spec: &Value, b: &Budget, probe: Option<&Probe>) -> PreviewPlan {
    let Some(src) = text(spec, "input") else {
        return pure(PreviewData::waiting("Choose a source to watch it change."));
    };
    if !can_render(b) {
        return pure(PreviewData::none());
    }
    let (before, mut steps) = frame_prep(&src, sample_at(probe), b);
    let out = cache_path(
        b,
        &format!(
            "{}.{}",
            render_key(kind, &src, spec, b),
            out_ext(kind, spec)
        ),
    );

    if Path::new(&out).exists() && Path::new(&before).exists() {
        steps.clear();
    } else {
        match crate::exec::plan(kind, &respec(spec, Some(&before), &out)) {
            Ok(more) => steps.extend(more),
            Err(e) => return pure(PreviewData::waiting(e.to_string())),
        }
    }
    PreviewPlan::Render(RenderPlan {
        kind: "image",
        out,
        before,
        steps,
        note: format!("One frame at {} px, at these settings.", b.max_px),
    })
}

/// Crop and rotate, which change the frame's shape.
///
/// These cannot run on the pre-scaled frame the others use: a crop box is in
/// source pixels, and 1920×800 out of a 1280-wide frame is an ffmpeg error
/// rather than a picture. So the job's own filter runs against the source and
/// the cap goes on the end of its chain — one command, one decode, and the
/// numbers on the form mean what they say. There is no before/after either,
/// because the two sides would not be the same shape.
fn geometry_render(kind: &str, spec: &Value, b: &Budget, probe: Option<&Probe>) -> PreviewPlan {
    let Some(src) = text(spec, "input") else {
        return pure(PreviewData::waiting("Choose a source to see the result."));
    };
    if !can_render(b) {
        return pure(PreviewData::none());
    }
    let out = cache_path(b, &format!("{}.png", render_key(kind, &src, spec, b)));
    let steps = if Path::new(&out).exists() {
        Vec::new()
    } else {
        let mut steps = match crate::exec::plan(kind, &respec(spec, None, &out)) {
            Ok(steps) => steps,
            Err(e) => return pure(PreviewData::waiting(e.to_string())),
        };
        let at = sample_at(probe);
        for step in &mut steps {
            let Step::Ffmpeg { args, .. } = step else {
                continue;
            };
            if let Some(i) = args.iter().position(|a| a == "-vf") {
                args[i + 1] = format!("{},{}", args[i + 1], cap_filter(b.max_px));
            }
            if at > 0.25 {
                args.splice(0..0, ["-ss".to_string(), format!("{at:.3}")]);
            }
            // Before the output, which `plan` always puts last.
            let last = args.len().saturating_sub(1);
            args.splice(last..last, ["-frames:v".to_string(), "1".to_string()]);
        }
        steps
    };
    PreviewPlan::Render(RenderPlan {
        kind: "image",
        out,
        before: String::new(),
        steps,
        note: "One frame, cropped and turned as the form says.".into(),
    })
}

/// Compress video, previewed by putting a single frame through the encoder the
/// job would use, at the CRF the job would use, and decoding it back.
///
/// A whole clip would be truer and twenty times slower. The blocking and the
/// smearing a CRF buys you show up on one frame.
fn video_render(spec: &Value, b: &Budget, probe: Option<&Probe>) -> PreviewPlan {
    let Some(src) = text(spec, "input") else {
        return pure(PreviewData::waiting(
            "Choose a video to watch the quality change.",
        ));
    };
    if !can_render(b) {
        return pure(PreviewData::none());
    }
    let (before, mut steps) = frame_prep(&src, sample_at(probe), b);
    let key = render_key("compress_video", &src, spec, b);
    let encoded = cache_path(b, &format!("enc-{key}.mkv"));
    let out = cache_path(b, &format!("{key}.png"));

    if Path::new(&out).exists() && Path::new(&before).exists() {
        steps.clear();
    } else {
        match crate::exec::plan("compress_video", &respec(spec, Some(&before), &encoded)) {
            Ok(more) => steps.extend(more),
            Err(e) => return pure(PreviewData::waiting(e.to_string())),
        }
        steps.push(ffmpeg(vec![
            "-i".into(),
            encoded,
            "-frames:v".into(),
            "1".into(),
            out.clone(),
        ]));
    }
    PreviewPlan::Render(RenderPlan {
        kind: "image",
        out,
        before,
        steps,
        note: "One frame through the encoder, at this CRF.".into(),
    })
}

/// Operations whose output is already small enough to just make: a thumbnail
/// is a thumbnail. The job's own plan, pointed at the cache.
fn job_render(kind: &str, spec: &Value, b: &Budget, note: &str) -> PreviewPlan {
    let Some(src) = text(spec, "input") else {
        return pure(PreviewData::waiting("Choose a source to see the result."));
    };
    if !can_render(b) {
        return pure(PreviewData::none());
    }
    let out = cache_path(b, &format!("{}.png", render_key(kind, &src, spec, b)));
    let steps = if Path::new(&out).exists() {
        Vec::new()
    } else {
        match crate::exec::plan(kind, &respec(spec, None, &out)) {
            Ok(steps) => steps,
            Err(e) => return pure(PreviewData::waiting(e.to_string())),
        }
    };
    PreviewPlan::Render(RenderPlan {
        kind: "image",
        out,
        before: String::new(),
        steps,
        note: note.to_string(),
    })
}

/// The contact sheet, which is one ffmpeg command once its duration is known —
/// the same tile filter the job builds.
fn sheet_render(spec: &Value, b: &Budget, probe: Option<&Probe>) -> PreviewPlan {
    let Some(src) = text(spec, "input") else {
        return pure(PreviewData::waiting("Choose a video to see the sheet."));
    };
    let Some(p) = probe.filter(|p| p.duration_s > 0.0) else {
        return pure(PreviewData::waiting("Reading the video…"));
    };
    if !can_render(b) {
        return pure(PreviewData::none());
    }
    let cols = number(spec, "cols").unwrap_or(4.0).max(1.0) as u32;
    let rows = number(spec, "rows").unwrap_or(4.0).max(1.0) as u32;
    let out = cache_path(
        b,
        &format!("{}.png", render_key("contact_sheet", &src, spec, b)),
    );
    let steps = if Path::new(&out).exists() {
        Vec::new()
    } else {
        let tile = crate::thumbnail::contact_sheet_filter(
            p.duration_s,
            cols,
            rows,
            (b.max_px / cols.max(1)).max(64),
        );
        vec![ffmpeg(vec![
            "-i".into(),
            src,
            "-vf".into(),
            tile,
            "-frames:v".into(),
            "1".into(),
            out.clone(),
        ])]
    };
    PreviewPlan::Render(RenderPlan {
        kind: "image",
        out,
        before: String::new(),
        steps,
        note: format!("{cols}×{rows} across the whole file."),
    })
}

/// Trim has no "after" worth drawing — the cut is the same picture, shorter.
/// What is worth drawing is where it starts.
fn trim_render(spec: &Value, b: &Budget) -> PreviewPlan {
    let Some(src) = text(spec, "input") else {
        return pure(PreviewData::waiting(
            "Choose a video to see where the cut starts.",
        ));
    };
    if !can_render(b) {
        return pure(PreviewData::none());
    }
    let start = number(spec, "start_s").unwrap_or(0.0).max(0.0);
    let end = number(spec, "end_s");
    let (out, steps) = frame_prep(&src, start, b);
    let note = match end {
        Some(e) if e > start => format!(
            "The first frame of the cut — {} to {}, {} long.",
            clock(start),
            clock(e),
            clock(e - start)
        ),
        _ => format!(
            "The frame at {}. Set an end to see the length.",
            clock(start)
        ),
    };
    PreviewPlan::Render(RenderPlan {
        kind: "image",
        out,
        before: String::new(),
        steps,
        note,
    })
}

/// Audio, decoded once per source to a tiny mono stream the caller turns into
/// peaks. 1 kHz mono is 2 KB a second — a whole album is smaller than one of
/// the frames above.
fn wave_render(spec: &Value, b: &Budget) -> PreviewPlan {
    let Some(src) = text(spec, "input") else {
        return pure(PreviewData::waiting("Choose a file to see its waveform."));
    };
    if !can_render(b) {
        return pure(PreviewData::none());
    }
    let out = cache_path(b, &format!("pcm-{}.raw", digest(&source_stamp(&src))));
    let steps = if Path::new(&out).exists() {
        Vec::new()
    } else {
        vec![ffmpeg(vec![
            "-i".into(),
            src,
            "-vn".into(),
            "-ac".into(),
            "1".into(),
            "-ar".into(),
            PEAK_RATE.to_string(),
            "-t".into(),
            WAVE_SECONDS.to_string(),
            "-f".into(),
            "s16le".into(),
            out.clone(),
        ])]
    };
    PreviewPlan::Render(RenderPlan {
        kind: "wave",
        out,
        before: String::new(),
        steps,
        note: "The source, as it is now.".into(),
    })
}

/// Sample rate the waveform is decoded at. Peaks, not audio — 1 kHz is plenty
/// for a few hundred pixels of drawing.
const PEAK_RATE: u32 = 1000;
/// Longest stretch of audio a waveform reads. Two hours at 1 kHz mono is 14 MB.
const WAVE_SECONDS: u32 = 7200;

/// The job's own spec, pointed at a frame and at the cache. Everything else —
/// the CRF, the quality, the filter — is exactly what the user chose, which is
/// what makes the render a preview of the job rather than a picture of its own.
fn respec(spec: &Value, input: Option<&str>, output: &str) -> Value {
    let mut v = spec.clone();
    if let Some(o) = v.as_object_mut() {
        if let Some(i) = input {
            o.insert("input".to_string(), json!(i));
        }
        o.insert("output".to_string(), json!(output));
    }
    v
}

fn ffmpeg(args: Vec<String>) -> Step {
    Step::Ffmpeg {
        args,
        duration_input: None,
        duration_s: None,
    }
}

/// Cap the longest edge without distorting. Sources smaller than the cap are
/// scaled up to it, which costs nothing and keeps both sides of a before/after
/// the same size.
fn cap_filter(px: u32) -> String {
    format!("scale={px}:{px}:force_original_aspect_ratio=decrease:force_divisible_by=2")
}

/// A tenth of the way in. The first frame of a video is very often black.
fn sample_at(probe: Option<&Probe>) -> f64 {
    probe.map(|p| p.duration_s * 0.1).unwrap_or(0.0)
}

fn out_ext(kind: &str, spec: &Value) -> &'static str {
    if !matches!(kind, "compress_photo" | "image_convert") {
        // Everything else renders to a PNG: the artifact is a picture to look
        // at, not the file the job would write.
        return "png";
    }
    match text(spec, "format")
        .or_else(|| text(spec, "target_ext"))
        .as_deref()
    {
        Some("webp") => "webp",
        Some("avif") => "avif",
        Some("png") => "png",
        _ => "jpg",
    }
}

fn cache_path(b: &Budget, name: &str) -> String {
    b.cache.join(name).to_string_lossy().to_string()
}

/// What makes one render different from another: the operation, the source as
/// it is on disk right now, every setting except where the output lands, and
/// the size we drew it at.
fn render_key(kind: &str, src: &str, spec: &Value, b: &Budget) -> String {
    digest(&format!(
        "{kind}\u{1}{}\u{1}{}\u{1}{}",
        source_stamp(src),
        spec_stamp(spec),
        b.max_px
    ))
}

/// A source's identity for cache purposes: where it is, how big it is, and
/// when it last changed. Editing a file in place invalidates its renders.
fn source_stamp(path: &str) -> String {
    let meta = std::fs::metadata(path);
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{path}|{len}|{mtime}")
}

/// Every field the user chose except where the result lands. Moving the output
/// path must not throw away a render that is already correct.
fn spec_stamp(spec: &Value) -> String {
    let mut parts: Vec<String> = spec
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "output" | "manifest"))
                .map(|(k, v)| format!("{k}={v}"))
                .collect()
        })
        .unwrap_or_default();
    parts.sort();
    parts.join("&")
}

fn digest(parts: &str) -> String {
    let mut h = Sha256::new();
    h.update(parts.as_bytes());
    // Sixteen hex characters is 64 bits. A collision would show the wrong
    // picture; at the number of previews one session makes, it will not happen.
    hex16(&h.finalize())
}

fn hex16(bytes: &[u8]) -> String {
    bytes.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

// -------------------------------------------------------------------- peaks ---

/// Signed 16-bit mono PCM → one 0..1 peak per bucket. The caller draws them.
pub fn peaks_from_pcm(bytes: &[u8], buckets: usize) -> Vec<f32> {
    let samples = bytes.len() / 2;
    if samples == 0 || buckets == 0 {
        return Vec::new();
    }
    let per = (samples / buckets).max(1);
    let mut out = Vec::with_capacity(buckets.min(samples));
    for chunk in bytes.chunks_exact(2).collect::<Vec<_>>().chunks(per) {
        let mut peak = 0i32;
        for s in chunk {
            let v = i16::from_le_bytes([s[0], s[1]]) as i32;
            peak = peak.max(v.abs());
        }
        out.push(peak as f32 / 32768.0);
        if out.len() == buckets {
            break;
        }
    }
    out
}

// -------------------------------------------------------------------- cache ---

/// Trim the preview cache to `max_bytes`, oldest first.
///
/// Swept when the section opens rather than after every render: a render that
/// pauses to delete things is a render the user is waiting on.
pub fn sweep_cache(dir: &Path, max_bytes: u64) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((
                meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                meta.len(),
                e.path(),
            ))
        })
        .collect();
    let total: u64 = files.iter().map(|(_, len, _)| len).sum();
    if total <= max_bytes {
        return 0;
    }
    files.sort_by_key(|(t, _, _)| *t);
    let mut freed = 0u64;
    for (_, len, path) in files {
        if total - freed <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            freed += len;
        }
    }
    freed
}

// ---------------------------------------------------------------- estimates ---

/// What the run is likely to produce. A ratio table and one file size — it
/// never spawns anything, and it is often wrong, which is why the UI says
/// "Estimated" and replaces it with the job's real numbers when it finishes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub src: u64,
    pub out: u64,
    /// Encode time. Zero until a probe supplies the source duration.
    pub secs: f64,
}

/// Estimate the output size for a kind, given its spec, the source's size on
/// disk, and one probe where there is one. `None` when the operation's output
/// size is not a function of those.
pub fn estimate(kind: &str, spec: &Value, src: u64, probe: Option<&Probe>) -> Option<Estimate> {
    if src == 0 {
        return None;
    }
    // Audio is the one estimate that is not a ratio: a bitrate times a running
    // time is the answer, not a guess at it.
    if kind == "audio_convert" {
        let secs = probe.map(|p| p.duration_s).unwrap_or(0.0);
        let target = text(spec, "target_ext").unwrap_or_default();
        if matches!(target.as_str(), "flac" | "wav" | "aiff") {
            let ratio = if target == "flac" { 0.55 } else { 1.6 };
            return Some(Estimate {
                src,
                out: (src as f64 * ratio) as u64,
                secs: secs * 0.05,
            });
        }
        if secs > 0.0 {
            let kbps = number(spec, "kbps").unwrap_or(192.0).clamp(32.0, 512.0);
            return Some(Estimate {
                src,
                out: (kbps * 125.0 * secs) as u64,
                secs: secs * 0.05,
            });
        }
    }
    let ratio = match kind {
        "compress_video" => {
            // A CRF step of 6 is about a doubling, and 23 is ffmpeg's default.
            let crf = number(spec, "crf").unwrap_or(23.0);
            let codec = match text(spec, "codec").as_deref() {
                Some("h265") => 0.62,
                Some("av1") => 0.50,
                _ => 1.0,
            };
            0.45 * 2f64.powf((23.0 - crf) / 6.0) * codec
        }
        "compress_photo" | "image_convert" => {
            let q = number(spec, "quality").unwrap_or(82.0).clamp(1.0, 100.0);
            let fmt = match text(spec, "format")
                .or_else(|| text(spec, "target_ext"))
                .as_deref()
            {
                Some("webp") => 0.70,
                Some("avif") => 0.50,
                // Lossless, and almost always larger than the JPEG that went in.
                Some("png") => 2.5,
                _ => 1.0,
            };
            // Quality is not linear in bytes; 82 is roughly break-even against a
            // camera JPEG, and it falls away fast below 60.
            0.85 * (q / 82.0).powf(2.2) * fmt
        }
        "convert" => {
            // A re-encode into a modern codec is usually smaller; into an
            // older one, usually not. Both are within a factor the footer is
            // honest about.
            match text(spec, "target_ext").as_deref() {
                Some("webm") => 0.7,
                Some("mkv") => 0.8,
                Some("mp3") | Some("m4a") | Some("aac") | Some("opus") => 0.12,
                Some("flac") | Some("wav") => 1.0,
                _ => 1.0,
            }
        }
        _ => return None,
    };
    let ratio = ratio.clamp(0.02, 3.0);
    // Encode time is roughly a multiple of the running time, and the codec is
    // what decides the multiple. Without a probe there is no running time and
    // the footer simply does not mention it.
    let secs = probe.map(|p| p.duration_s).unwrap_or(0.0)
        * match (kind, text(spec, "codec").as_deref()) {
            ("compress_video", Some("h265")) => 1.2,
            ("compress_video", Some("av1")) => 4.0,
            ("compress_video", _) => 0.35,
            ("convert", _) => 0.3,
            _ => 0.0,
        };
    Some(Estimate {
        src,
        out: (src as f64 * ratio) as u64,
        secs,
    })
}

// ------------------------------------------------------------------ parsing ---

/// Parse SRT, WebVTT or ASS into cues. Returns the first `limit` and the real
/// total.
pub fn parse_cues(body: &str, limit: usize) -> (Vec<Cue>, usize) {
    let mut found: Vec<(u64, u64, String)> = Vec::new();

    if body
        .lines()
        .any(|l| l.trim_start().starts_with("Dialogue:"))
    {
        for line in body.lines() {
            let Some(rest) = line.trim_start().strip_prefix("Dialogue:") else {
                continue;
            };
            // Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
            let parts: Vec<&str> = rest.splitn(10, ',').collect();
            if parts.len() < 10 {
                continue;
            }
            let (Some(a), Some(b)) = (parse_ts(parts[1]), parse_ts(parts[2])) else {
                continue;
            };
            found.push((a, b, strip_ass(parts[9])));
        }
    } else {
        // SRT and VTT are the same shape: a timing line, then the text under it.
        let mut lines = body.lines().peekable();
        while let Some(line) = lines.next() {
            let Some((a, b)) = split_timing(line) else {
                continue;
            };
            let mut text = String::new();
            while let Some(next) = lines.peek() {
                if next.trim().is_empty() {
                    break;
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(next.trim());
                lines.next();
            }
            found.push((a, b, text));
        }
    }

    let total = found.len();
    let cues = found
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(i, (start_ms, end_ms, text))| Cue {
            index: i + 1,
            start_ms,
            end_ms,
            text,
        })
        .collect();
    (cues, total)
}

fn split_timing(line: &str) -> Option<(u64, u64)> {
    let (a, b) = line.split_once("-->")?;
    // VTT puts cue settings after the end time: `00:01.000 --> 00:03.000 line:90%`
    let b = b.trim().split_whitespace().next()?;
    Some((parse_ts(a)?, parse_ts(b)?))
}

/// `01:02:03,450`, `01:02:03.450`, `02:03.45` → milliseconds.
fn parse_ts(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    let mut secs = 0f64;
    let parts: Vec<&str> = s.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for p in &parts {
        secs = secs * 60.0 + p.trim().parse::<f64>().ok()?;
    }
    Some((secs * 1000.0) as u64)
}

/// Drop ASS override blocks (`{\pos(1,2)}`) and turn its line breaks into spaces.
fn strip_ass(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '\\' if depth == 0 && matches!(chars.peek().copied(), Some('N' | 'n')) => {
                chars.next();
                out.push(' ');
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

// ------------------------------------------------------------------ helpers ---

fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

fn number(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn flag(v: &Value, k: &str) -> bool {
    v.get(k)
        .map(|x| x.as_bool().unwrap_or_else(|| x.as_str() == Some("true")))
        .unwrap_or(false)
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn extensions(paths: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = paths
        .iter()
        .map(|p| {
            Path::new(p)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        })
        .collect();
    seen.sort();
    seen.dedup();
    seen
}

fn file_size(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Every file under `root` as `relative path → size`, capped.
///
/// `folder_diff::walk` is the one that reads the disk; this drops the mtime,
/// which the size comparison here does not use.
fn walk_sizes(root: &str, cap: usize) -> BTreeMap<String, u64> {
    crate::folder_diff::walk(root, cap)
        .into_iter()
        .map(|(rel, (bytes, _))| (rel, bytes))
        .collect()
}

/// Substitute a `%03d`-style token in an ffmpeg output template.
fn expand_index(template: &str, i: usize) -> String {
    let Some(start) = template.find('%') else {
        return base_name(template);
    };
    let rest = &template[start + 1..];
    let Some(end) = rest.find('d') else {
        return base_name(template);
    };
    let width: usize = rest[..end].trim_start_matches('0').parse().unwrap_or(0);
    let filled = format!(
        "{}{:0width$}{}",
        &template[..start],
        i,
        &rest[end + 1..],
        width = width
    );
    base_name(&filled)
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn human(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_blank_form_never_blows_up() {
        // The preview runs on every keystroke, including the first one, when
        // every other field is still empty. It must answer, not panic.
        for op in crate::catalog::CATALOG {
            let PreviewPlan::Pure(data) =
                plan_preview(op.kind, &json!({}), &Budget::default(), None)
            else {
                continue;
            };
            assert!(
                matches!(data.kind, "waiting" | "none" | "dryrun" | "cues"),
                "{}: unexpected preview kind {}",
                op.kind,
                data.kind
            );
        }
    }

    #[test]
    fn what_the_catalogue_promises_the_planner_delivers() {
        // The pane draws what `OpDef::preview` names. An op that claims a dry
        // run and falls through to the `_` arm here would show the empty-pane
        // placeholder instead — the exact kind of drift between two tables that
        // the catalogue exists to prevent.
        use crate::catalog::Preview;
        for op in crate::catalog::CATALOG {
            // `Plain` is the honest "nothing to draw before it runs". Every
            // other declared preview must answer something.
            if matches!(op.preview, Preview::Plain) {
                continue;
            }
            let PreviewPlan::Pure(data) =
                plan_preview(op.kind, &json!({}), &Budget::default(), None)
            else {
                continue;
            };
            assert_ne!(
                data.kind,
                "none",
                "{} declares {} but plan_preview has no arm for it",
                op.kind,
                op.preview.id()
            );
        }
    }

    #[test]
    fn a_document_preview_names_the_file_it_will_write() {
        let PreviewPlan::Pure(d) = plan_preview(
            "doc_convert",
            &json!({"input":"/n/meeting notes.md","format":"docx"}),
            &Budget::default(),
            None,
        ) else {
            unreachable!()
        };
        assert_eq!(d.kind, "dryrun");
        let names = d
            .rows
            .iter()
            .find(|r| r.note == "name")
            .expect("a name row");
        assert_eq!(names.left, "meeting notes.md");
        assert_eq!(names.right, "meeting notes.docx");
        assert!(!d.rows.iter().any(|r| r.kind == RowKind::Warn));
    }

    #[test]
    fn converting_a_format_to_itself_is_flagged_rather_than_refused() {
        let PreviewPlan::Pure(d) = plan_preview(
            "ebook_convert",
            &json!({"input":"/b/x.epub","format":"epub"}),
            &Budget::default(),
            None,
        ) else {
            unreachable!()
        };
        assert!(d.rows.iter().any(|r| r.kind == RowKind::Warn));
    }

    #[test]
    fn the_stems_preview_is_the_tree_demucs_writes() {
        let PreviewPlan::Pure(d) = plan_preview(
            "stems",
            &json!({"input":"/m/song.flac","mode":"four"}),
            &Budget::default(),
            None,
        ) else {
            unreachable!()
        };
        let adds: Vec<&str> = d
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Add)
            .map(|r| r.left.as_str())
            .collect();
        assert_eq!(adds, ["vocals.wav", "drums.wav", "bass.wav", "other.wav"]);
        assert!(d.rows[0].note.ends_with("/htdemucs/song"));

        // Two stems, and mp3 drops the size warning that wav earns.
        let PreviewPlan::Pure(two) = plan_preview(
            "stems",
            &json!({"input":"/m/song.flac","mode":"vocals","mp3":true}),
            &Budget::default(),
            None,
        ) else {
            unreachable!()
        };
        let adds: Vec<&str> = two
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Add)
            .map(|r| r.left.as_str())
            .collect();
        assert_eq!(adds, ["vocals.mp3", "no_vocals.mp3"]);
        assert!(!two.rows.iter().any(|r| r.kind == RowKind::Warn));
    }

    #[test]
    fn srt_and_vtt_parse_the_same() {
        let srt = "1\n00:00:01,000 --> 00:00:03,500\nHello\nthere\n\n2\n00:00:04,000 --> 00:00:05,000\nBye\n";
        let (cues, total) = parse_cues(srt, 10);
        assert_eq!(total, 2);
        assert_eq!(cues[0].start_ms, 1000);
        assert_eq!(cues[0].end_ms, 3500);
        assert_eq!(cues[0].text, "Hello there");

        let vtt = "WEBVTT\n\n00:01.000 --> 00:03.500 line:90%\nHello there\n";
        let (cues, _) = parse_cues(vtt, 10);
        assert_eq!(cues[0].start_ms, 1000);
        assert_eq!(cues[0].text, "Hello there");
    }

    #[test]
    fn ass_dialogue_loses_its_override_blocks() {
        let ass =
            "[Events]\nDialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,{\\pos(1,2)}One\\NTwo\n";
        let (cues, total) = parse_cues(ass, 10);
        assert_eq!(total, 1);
        assert_eq!(cues[0].text, "One Two");
        assert_eq!(cues[0].start_ms, 1000);
        assert_eq!(cues[0].end_ms, 2500);
    }

    #[test]
    fn cues_are_capped_but_counted() {
        let mut srt = String::new();
        for i in 0..50 {
            srt.push_str(&format!(
                "{i}\n00:00:0{}.000 --> 00:00:0{}.000\nline\n\n",
                i % 9,
                (i % 9) + 1
            ));
        }
        let (cues, total) = parse_cues(&srt, 5);
        assert_eq!(cues.len(), 5);
        assert_eq!(total, 50);
    }

    #[test]
    fn split_names_its_segments() {
        let spec = json!({ "output": "/out/clip_%03d.mp4", "every_s": 90 });
        let PreviewPlan::Pure(d) = plan_preview("split", &spec, &Budget::default(), None) else {
            unreachable!()
        };
        assert_eq!(d.rows[0].left, "clip_000.mp4");
        assert_eq!(d.rows[1].left, "clip_001.mp4");
        assert_eq!(d.rows[1].right, "1:30 – 3:00");
    }

    #[test]
    fn a_template_without_a_counter_is_called_out() {
        let spec = json!({ "output": "/out/clip.mp4", "every_s": 60 });
        let PreviewPlan::Pure(d) = plan_preview("split", &spec, &Budget::default(), None) else {
            unreachable!()
        };
        assert!(d.note.contains("writes over"));
    }

    #[test]
    fn rename_previews_the_real_listing() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["b.jpg", "a.jpg", "c.png"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        let spec = json!({
            "dir": dir.path().to_string_lossy(),
            "pattern": "shot_{n:02}",
            "start": 1,
        });
        let PreviewPlan::Pure(d) = plan_preview("rename", &spec, &Budget::default(), None) else {
            unreachable!()
        };
        assert_eq!(d.kind, "dryrun");
        // Sorted by name, so `{n}` numbers the way the folder reads.
        assert_eq!(d.rows[0].left, "a.jpg");
        assert_eq!(d.rows[0].right, "shot_01.jpg");
        assert_eq!(d.rows[2].right, "shot_03.png");
    }

    #[test]
    fn rename_flags_collisions_before_anything_moves() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"x").unwrap();
        std::fs::write(dir.path().join("b.jpg"), b"x").unwrap();
        let spec = json!({ "dir": dir.path().to_string_lossy(), "pattern": "same" });
        let PreviewPlan::Pure(d) = plan_preview("rename", &spec, &Budget::default(), None) else {
            unreachable!()
        };
        assert!(d.rows.iter().all(|r| r.kind == RowKind::Warn));
        assert!(d.note.contains("collide"));
    }

    #[test]
    fn folder_diff_sorts_both_sides_together() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("same.txt"), b"xx").unwrap();
        std::fs::write(b.path().join("same.txt"), b"xx").unwrap();
        std::fs::write(a.path().join("only_a.txt"), b"x").unwrap();
        std::fs::write(b.path().join("only_b.txt"), b"x").unwrap();
        std::fs::write(a.path().join("size.txt"), b"x").unwrap();
        std::fs::write(b.path().join("size.txt"), b"xxxx").unwrap();
        let spec = json!({ "a": a.path().to_string_lossy(), "b": b.path().to_string_lossy() });
        let PreviewPlan::Pure(d) = plan_preview("folder_diff", &spec, &Budget::default(), None)
        else {
            unreachable!()
        };
        let kinds: Vec<_> = d.rows.iter().map(|r| (r.left.as_str(), r.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("only_a.txt", RowKind::Add),
                ("only_b.txt", RowKind::Remove),
                ("size.txt", RowKind::Change),
            ]
        );
    }

    #[test]
    fn merge_warns_about_mixed_containers() {
        let spec = json!({ "inputs": ["/a/one.mp4", "/a/two.mkv"] });
        let PreviewPlan::Pure(d) = plan_preview("merge", &spec, &Budget::default(), None) else {
            unreachable!()
        };
        assert!(d.rows.last().unwrap().kind == RowKind::Warn);
        assert!(d.note.contains("may not share a codec"));
    }

    #[test]
    fn estimates_move_the_right_way() {
        let src = 100_000_000;
        let low = estimate("compress_video", &json!({ "crf": 18 }), src, None).unwrap();
        let high = estimate("compress_video", &json!({ "crf": 35 }), src, None).unwrap();
        assert!(high.out < low.out, "a higher CRF must estimate smaller");
        let h265 = estimate(
            "compress_video",
            &json!({ "crf": 23, "codec": "h265" }),
            src,
            None,
        )
        .unwrap();
        let h264 = estimate("compress_video", &json!({ "crf": 23 }), src, None).unwrap();
        assert!(h265.out < h264.out);
        assert!(estimate("rename", &json!({}), src, None).is_none());
        assert!(estimate("compress_video", &json!({}), 0, None).is_none());
    }

    #[test]
    fn encode_time_needs_a_probe_and_scales_with_the_codec() {
        let probe = Probe {
            duration_s: 600.0,
            ..Probe::default()
        };
        let spec = json!({ "crf": 23, "codec": "av1" });
        assert_eq!(
            estimate("compress_video", &spec, 1000, None).unwrap().secs,
            0.0
        );
        let with = estimate("compress_video", &spec, 1000, Some(&probe)).unwrap();
        assert!(with.secs > 600.0, "AV1 is slower than realtime");
    }

    fn render_budget(dir: &Path) -> Budget {
        Budget {
            cache: dir.to_path_buf(),
            ..Budget::default()
        }
    }

    #[test]
    fn a_render_reuses_the_job_s_own_plan() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("holiday.jpg");
        std::fs::write(&src, b"not really a jpeg").unwrap();
        let spec = json!({
            "input": src.to_string_lossy(),
            "quality": 40,
            "format": "webp",
            "output": "/somewhere/else.webp",
        });
        let PreviewPlan::Render(r) =
            plan_preview("compress_photo", &spec, &render_budget(dir.path()), None)
        else {
            panic!("compress_photo should render");
        };
        assert_eq!(r.kind, "image");
        assert!(r.out.ends_with(".webp"), "the format field picks the muxer");
        // Frame first, then the operation itself — and the operation is the
        // same argv `plan()` builds, run against the frame.
        assert_eq!(r.steps.len(), 2);
        let Step::Ffmpeg { args, .. } = &r.steps[1] else {
            panic!("second step should be ffmpeg");
        };
        assert!(args.contains(&"libwebp".to_string()));
        assert!(args.contains(&r.before), "the operation runs on the frame");
        assert!(args.contains(&r.out));
        // The frame is the before, and both sides are the same picture size.
        assert!(r.before.contains("src-"));
    }

    #[test]
    fn moving_the_output_does_not_throw_away_a_render() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jpg");
        std::fs::write(&src, b"x").unwrap();
        let b = render_budget(dir.path());
        let key = |output: &str, quality: i32| {
            let spec = json!({
                "input": src.to_string_lossy(),
                "quality": quality,
                "output": output,
            });
            let PreviewPlan::Render(r) = plan_preview("compress_photo", &spec, &b, None) else {
                panic!()
            };
            r.out
        };
        assert_eq!(key("/one.jpg", 70), key("/two.jpg", 70));
        assert_ne!(key("/one.jpg", 70), key("/one.jpg", 71));
    }

    #[test]
    fn compress_video_encodes_a_frame_and_decodes_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        std::fs::write(&src, b"x").unwrap();
        let spec = json!({
            "input": src.to_string_lossy(),
            "crf": 35,
            "codec": "h265",
            "output": "/out.mp4",
        });
        let PreviewPlan::Render(r) =
            plan_preview("compress_video", &spec, &render_budget(dir.path()), None)
        else {
            panic!("compress_video should render");
        };
        assert_eq!(r.steps.len(), 3, "frame, encode, decode");
        let Step::Ffmpeg { args, .. } = &r.steps[1] else {
            panic!()
        };
        assert!(args.contains(&"libx265".to_string()));
        assert!(args.contains(&"35".to_string()));
        assert!(r.out.ends_with(".png"));
    }

    #[test]
    fn a_caller_with_nowhere_to_write_gets_no_render() {
        // The CLI and the tests plan previews too, and neither has a cache.
        let spec = json!({ "input": "/x.jpg", "quality": 80 });
        let PreviewPlan::Pure(d) = plan_preview("compress_photo", &spec, &Budget::default(), None)
        else {
            panic!("no cache means no render")
        };
        assert_eq!(d.kind, "none");
    }

    #[test]
    fn a_crop_preview_keeps_the_forms_numbers_and_caps_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("wide.mp4");
        std::fs::write(&src, b"x").unwrap();
        let spec = json!({
            "input": src.to_string_lossy(),
            "aspect": "manual", "w": 1920, "h": 800, "x": 0, "y": 140,
        });
        let PreviewPlan::Render(r) = plan_preview("crop", &spec, &render_budget(dir.path()), None)
        else {
            panic!("crop should render");
        };
        assert!(
            r.before.is_empty(),
            "a crop has no before to line up against"
        );
        assert_eq!(r.steps.len(), 1, "one command, one decode");
        let Step::Ffmpeg { args, .. } = &r.steps[0] else {
            panic!()
        };
        let vf = args.iter().position(|a| a == "-vf").unwrap();
        // The form's pixels, unscaled, and the cap on the end of the chain.
        assert!(
            args[vf + 1].starts_with("crop=1920:800:0:140,scale="),
            "{}",
            args[vf + 1]
        );
        assert!(args.windows(2).any(|w| w == ["-frames:v", "1"]));
        assert_eq!(args.last().unwrap(), &r.out);
    }

    #[test]
    fn an_audio_estimate_is_arithmetic_rather_than_a_ratio() {
        let probe = Probe {
            duration_s: 300.0,
            ..Probe::default()
        };
        let mp3 = estimate(
            "audio_convert",
            &json!({ "target_ext": "mp3", "kbps": 192 }),
            50_000_000,
            Some(&probe),
        )
        .unwrap();
        // 192 kb/s over five minutes is 7.2 MB, whatever the source was.
        assert!((mp3.out as i64 - 7_200_000).abs() < 50_000, "{}", mp3.out);
        let flac = estimate(
            "audio_convert",
            &json!({ "target_ext": "flac" }),
            50_000_000,
            Some(&probe),
        )
        .unwrap();
        assert!(
            flac.out < 50_000_000,
            "FLAC is smaller than the WAV it came from"
        );
    }

    #[test]
    fn a_page_strip_marks_what_survives() {
        // No PDF on disk, so this exercises the half that does not need one:
        // the range arithmetic that decides which page is page 3.
        let count = 10;
        assert_eq!(crate::pdf::selected("1-3,5", count), vec![1, 2, 3, 5]);
        // Deleting inverts the same set; keeping does not.
        for (kind, page, expected) in [
            ("pdf_pages", 5u32, true),
            ("pdf_pages", 4, false),
            ("pdf_delete", 5, false),
            ("pdf_delete", 4, true),
        ] {
            let named = crate::pdf::selected("1-3,5", count).contains(&page);
            let kept = match kind {
                "pdf_delete" => !named,
                "pdf_pages" => named,
                _ => true,
            };
            assert_eq!(kept, expected, "{kind} page {page}");
        }
    }

    #[test]
    fn a_mirror_lists_every_deletion_by_name() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("new.txt"), b"hello").unwrap();
        std::fs::write(b.path().join("stale.txt"), b"old").unwrap();
        let spec = json!({
            "a": a.path().to_string_lossy(),
            "b": b.path().to_string_lossy(),
            "delete_extra": true,
        });
        let PreviewPlan::Pure(d) = plan_preview("mirror", &spec, &Budget::default(), None) else {
            unreachable!()
        };
        assert_eq!(d.kind, "dryrun");
        let deletions: Vec<&Row> = d
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Remove)
            .collect();
        assert_eq!(deletions.len(), 1);
        assert_eq!(deletions[0].left, "stale.txt");
        assert!(d.note.contains("permanent"), "{}", d.note);

        // With the toggle off, nothing is listed for deletion and the note says so.
        let safe = json!({
            "a": a.path().to_string_lossy(),
            "b": b.path().to_string_lossy(),
        });
        let PreviewPlan::Pure(d) = plan_preview("mirror", &safe, &Budget::default(), None) else {
            unreachable!()
        };
        assert!(d.rows.iter().all(|r| r.kind != RowKind::Remove));
        assert!(d.note.contains("Nothing is deleted"), "{}", d.note);
    }

    #[test]
    fn an_archive_preview_reads_the_directory_and_unpacks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("holiday");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("one.jpg"), vec![0u8; 900]).unwrap();
        std::fs::write(src.join("two.jpg"), vec![0u8; 100]).unwrap();

        let zip_path = dir.path().join("holiday.zip");
        let spec = json!({
            "folder": src.to_string_lossy(),
            "output": zip_path.to_string_lossy(),
        });
        let PreviewPlan::Pure(d) = plan_preview("archive_create", &spec, &Budget::default(), None)
        else {
            unreachable!()
        };
        assert_eq!(d.rows.len(), 2);
        assert_eq!(d.rows[0].left, "holiday/one.jpg");
        assert!(d.note.contains("1.0 KB"), "{}", d.note);
        // The preview planned it; nothing was written.
        assert!(!zip_path.exists());

        // And once it exists, reading it back lists the same two.
        let members = crate::archive::members(&src.to_string_lossy(), &[], 100);
        crate::archive::create(&members, &zip_path.to_string_lossy(), false, |_, _| true).unwrap();
        let read = json!({ "input": zip_path.to_string_lossy(), "dir": "/tmp/out" });
        let PreviewPlan::Pure(d) = plan_preview("archive_extract", &read, &Budget::default(), None)
        else {
            unreachable!()
        };
        assert_eq!(d.rows.len(), 2);
        assert!(d.rows.iter().all(|r| r.kind == RowKind::Plain));
    }

    #[test]
    fn peaks_are_bucketed_maxima() {
        // Four samples: 0, half, quarter, full.
        let pcm: Vec<u8> = [0i16, 16384, 8192, 32767]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let peaks = peaks_from_pcm(&pcm, 2);
        assert_eq!(peaks.len(), 2);
        assert!((peaks[0] - 0.5).abs() < 0.01);
        assert!((peaks[1] - 1.0).abs() < 0.01);
        assert!(peaks_from_pcm(&[], 8).is_empty());
    }

    #[test]
    fn the_cache_sweep_takes_the_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        for (n, size) in [("old", 400), ("new", 400)] {
            std::fs::write(dir.path().join(n), vec![0u8; size]).unwrap();
            // Distinct mtimes without sleeping.
            let stamp = if n == "old" { 1_000 } else { 2_000_000 };
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(stamp);
            let f = std::fs::File::options()
                .write(true)
                .open(dir.path().join(n))
                .unwrap();
            f.set_modified(t).unwrap();
        }
        let freed = sweep_cache(dir.path(), 500);
        assert_eq!(freed, 400);
        assert!(!dir.path().join("old").exists());
        assert!(dir.path().join("new").exists());
    }

    #[test]
    fn split_counts_its_segments_once_it_knows_the_length() {
        let spec = json!({ "output": "/o/clip_%03d.mp4", "every_s": 60 });
        let probe = Probe {
            duration_s: 305.0,
            ..Probe::default()
        };
        let PreviewPlan::Pure(d) = plan_preview("split", &spec, &Budget::default(), Some(&probe))
        else {
            unreachable!()
        };
        assert!(d.note.contains("6 files"), "{}", d.note);
        // The last one is short, and says so.
        assert_eq!(d.rows[5].right, "5:00 – 5:05");
    }

    #[test]
    fn convert_names_both_codecs() {
        let spec = json!({ "input": "/a/clip.mov", "target_ext": "webm" });
        let probe = Probe {
            duration_s: 90.0,
            width: 1920,
            height: 1080,
            v_codec: "h264".into(),
            a_codec: "aac".into(),
            bytes: 100,
        };
        let PreviewPlan::Pure(d) = plan_preview("convert", &spec, &Budget::default(), Some(&probe))
        else {
            unreachable!()
        };
        let rendered: Vec<String> = d
            .rows
            .iter()
            .map(|r| format!("{} {} {}", r.left, r.right, r.note))
            .collect();
        assert!(rendered.iter().any(|r| r.contains("h264 libvpx-vp9")));
        assert!(rendered.iter().any(|r| r.contains("MOV WEBM")));
        assert!(rendered.iter().any(|r| r.contains("1920×1080 1:30")));
    }
}
