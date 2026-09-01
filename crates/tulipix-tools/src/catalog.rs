//! The op catalogue — one table, and the only place an operation is declared.
//!
//! There used to be three. `api/tools.rs` held `ALL_OPS` plus four `match kind`
//! functions in snake_case; `section.rs` held a second category map in
//! kebab-case; `cli.rs` held a third list. They had already drifted — `resize`
//! sat under Video in one and Photo in another, the CLI knew a `pdf` op the GUI
//! did not, and `mediainfo` fell through every category arm and so appeared
//! under no tab at all.
//!
//! Adding an operation now means adding one [`OpDef`] here. The bridge's
//! labels, blurbs, categories and form schemas all read from it, `cli.rs`
//! derives its subcommands from it, and a test in `exec.rs` fails if this table
//! and `plan()` ever disagree about which kinds exist.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- category ---

/// The tabs on the landing page.
///
/// There is deliberately no `Queue` variant. The queue is a panel, never a
/// category, and having it here as the `_ =>` fallthrough is what hid
/// `mediainfo` from the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    FileOps,
    /// Format changes, which used to be one operation filed under Video. It is
    /// its own errand: people arrive knowing the format they want, not the
    /// medium it happens to be.
    Convert,
    Video,
    Audio,
    Photo,
    /// Page surgery. `pdf.rs` had the range parser and the operation enum
    /// written and unit-tested for months with nothing wired to either.
    Pdf,
    Subtitles,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::FileOps,
        Category::Convert,
        Category::Video,
        Category::Audio,
        Category::Photo,
        Category::Pdf,
        Category::Subtitles,
    ];

    /// The wire identifier. This is what the UI sends back in `SetCategory`, so
    /// it must stay stable.
    pub fn id(self) -> &'static str {
        match self {
            Category::FileOps => "fileops",
            Category::Convert => "convert",
            Category::Video => "video",
            Category::Audio => "audio",
            Category::Photo => "photo",
            Category::Pdf => "pdf",
            Category::Subtitles => "subtitles",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::FileOps => "File ops",
            Category::Convert => "Convert",
            Category::Video => "Video",
            Category::Audio => "Audio",
            Category::Photo => "Photo",
            Category::Pdf => "PDF",
            Category::Subtitles => "Subtitles",
        }
    }

    pub fn from_id(id: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.id() == id)
    }
}

// ----------------------------------------------------------------- preview ---

/// Which preview the right-hand pane draws while the form is being filled in.
///
/// This used to be a lookup table in Dart, which meant the answer to "what does
/// this tool show you before you run it" lived a bridge away from the tool.
/// `preview::plan_preview` matches on `kind`; this is the same decision,
/// declared where the operation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preview {
    /// Before and after on one frame.
    Image,
    /// A short sample at these settings, and a timeline.
    Video,
    /// The audio as peaks, with what is being cut marked.
    Wave,
    /// Every page as a thumbnail, kept ones lit.
    Pages,
    /// Subtitle lines with their timings.
    Cues,
    /// Every file this will touch, and what it becomes.
    DryRun,
    /// What goes in and what comes out, side by side.
    Convert,
    /// What the run will report back.
    Plain,
}

impl Preview {
    /// The wire identifier, which is what `tools_preview.dart` switches on.
    pub fn id(self) -> &'static str {
        match self {
            Preview::Image => "image",
            Preview::Video => "video",
            Preview::Wave => "wave",
            Preview::Pages => "pages",
            Preview::Cues => "cues",
            Preview::DryRun => "dryrun",
            Preview::Convert => "convert",
            Preview::Plain => "plain",
        }
    }
}

// ------------------------------------------------------------------- field ---

/// What control the form draws for a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    File,
    Files,
    Folder,
    Text,
    Number,
    Dropdown,
    Slider,
    Toggle,
    /// Where to write something that does not exist yet. A file chooser cannot
    /// pick a file that is not there, so these get the save dialog instead.
    Save,
}

impl FieldKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldKind::File => "file",
            FieldKind::Files => "files",
            FieldKind::Folder => "folder",
            FieldKind::Text => "text",
            FieldKind::Number => "number",
            FieldKind::Dropdown => "dropdown",
            FieldKind::Slider => "slider",
            FieldKind::Toggle => "toggle",
            FieldKind::Save => "save",
        }
    }
}

/// One row of the form an operation asks for. The declaration; the value the
/// user has typed lives in the bridge's session, not here.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    /// The default, and what Reset puts back.
    pub value: &'static str,
    pub options: &'static [&'static str],
    pub min: f64,
    pub max: f64,
    pub required: bool,
    pub hint: &'static str,
    /// Extensions the dialog filters to, without the dot. Empty means every
    /// file, which is right for the fields that genuinely take anything.
    pub ext: &'static [&'static str],
}

const fn field(
    key: &'static str,
    label: &'static str,
    kind: FieldKind,
    value: &'static str,
    options: &'static [&'static str],
    min: f64,
    max: f64,
    required: bool,
    hint: &'static str,
    ext: &'static [&'static str],
) -> FieldDef {
    FieldDef {
        key,
        label,
        kind,
        value,
        options,
        min,
        max,
        required,
        hint,
        ext,
    }
}

// The extension lists the dialogs filter to. Written out rather than composed
// because a slice cannot be concatenated in a const.
const ANY: &[&str] = &[];
const VIDEO: &[&str] = &[
    "mp4", "mkv", "mov", "webm", "avi", "m4v", "mpg", "mpeg", "ts", "wmv", "flv",
];
const AUDIO: &[&str] = &[
    "mp3", "m4a", "aac", "opus", "ogg", "flac", "wav", "wma", "aiff",
];
const IMAGE: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "avif", "gif", "bmp", "tif", "tiff", "heic",
];
const SUBS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub"];
const PDF: &[&str] = &["pdf"];
const ARCHIVE: &[&str] = &["zip", "cbz", "epub", "jar"];
/// What pandoc reads. Deliberately not everything it can read — this is the
/// list worth putting in front of someone, not the manual.
const DOC: &[&str] = &[
    "docx", "odt", "md", "markdown", "html", "htm", "rtf", "tex", "epub", "txt", "rst", "org",
];
/// What ebook-convert reads.
const EBOOK: &[&str] = &[
    "epub", "mobi", "azw3", "azw", "fb2", "lit", "pdb", "rtf", "txt", "html", "pdf", "cbz", "cbr",
];
/// Anything with a timeline — what ffmpeg will read for loudness or speech.
const AV: &[&str] = &[
    "mp4", "mkv", "mov", "webm", "avi", "m4v", "mpg", "mpeg", "ts", "wmv", "flv", "mp3", "m4a",
    "aac", "opus", "ogg", "flac", "wav", "wma", "aiff",
];
/// Anything ffmpeg will open at all.
const MEDIA: &[&str] = &[
    "mp4", "mkv", "mov", "webm", "avi", "m4v", "mpg", "mpeg", "ts", "wmv", "flv", "mp3", "m4a",
    "aac", "opus", "ogg", "flac", "wav", "wma", "aiff", "jpg", "jpeg", "png", "webp", "avif",
    "gif", "bmp", "tif", "tiff", "heic",
];
const IMAGE_VIDEO: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "avif", "gif", "bmp", "tif", "tiff", "heic", "mp4", "mkv", "mov",
    "webm", "avi", "m4v", "mpg", "mpeg", "ts", "wmv", "flv",
];

const fn file(key: &'static str, label: &'static str, ext: &'static [&'static str]) -> FieldDef {
    field(
        key,
        label,
        FieldKind::File,
        "",
        &[],
        0.0,
        0.0,
        true,
        "",
        ext,
    )
}
const fn files(
    key: &'static str,
    label: &'static str,
    ext: &'static [&'static str],
    hint: &'static str,
) -> FieldDef {
    field(
        key,
        label,
        FieldKind::Files,
        "",
        &[],
        0.0,
        0.0,
        true,
        hint,
        ext,
    )
}
/// A path that does not exist yet — the save dialog, with a suggested name.
const fn save(
    key: &'static str,
    label: &'static str,
    required: bool,
    ext: &'static [&'static str],
    hint: &'static str,
) -> FieldDef {
    field(
        key,
        label,
        FieldKind::Save,
        "",
        &[],
        0.0,
        0.0,
        required,
        hint,
        ext,
    )
}
const fn folder(
    key: &'static str,
    label: &'static str,
    required: bool,
    hint: &'static str,
) -> FieldDef {
    field(
        key,
        label,
        FieldKind::Folder,
        "",
        &[],
        0.0,
        0.0,
        required,
        hint,
        ANY,
    )
}
const fn text(
    key: &'static str,
    label: &'static str,
    value: &'static str,
    required: bool,
    hint: &'static str,
) -> FieldDef {
    field(
        key,
        label,
        FieldKind::Text,
        value,
        &[],
        0.0,
        0.0,
        required,
        hint,
        ANY,
    )
}
const fn number(
    key: &'static str,
    label: &'static str,
    value: &'static str,
    required: bool,
    hint: &'static str,
) -> FieldDef {
    field(
        key,
        label,
        FieldKind::Number,
        value,
        &[],
        0.0,
        0.0,
        required,
        hint,
        ANY,
    )
}
const fn pick(
    key: &'static str,
    label: &'static str,
    value: &'static str,
    options: &'static [&'static str],
    required: bool,
    hint: &'static str,
) -> FieldDef {
    field(
        key,
        label,
        FieldKind::Dropdown,
        value,
        options,
        0.0,
        0.0,
        required,
        hint,
        ANY,
    )
}
const fn slider(
    key: &'static str,
    label: &'static str,
    value: &'static str,
    min: f64,
    max: f64,
    hint: &'static str,
) -> FieldDef {
    field(
        key,
        label,
        FieldKind::Slider,
        value,
        &[],
        min,
        max,
        false,
        hint,
        ANY,
    )
}
const fn toggle(
    key: &'static str,
    label: &'static str,
    value: &'static str,
    hint: &'static str,
) -> FieldDef {
    field(
        key,
        label,
        FieldKind::Toggle,
        value,
        &[],
        0.0,
        0.0,
        false,
        hint,
        ANY,
    )
}

/// The output field most operations share: blank means beside the source.
const fn out() -> FieldDef {
    // No extension filter: `out()` is shared by ops that write video, audio and
    // images, and a filter that is wrong three times out of four is worse than
    // none.
    save(
        "output",
        "Output (blank = beside source)",
        false,
        ANY,
        "auto-named next to the source",
    )
}

// ---------------------------------------------------------------------- op ---

/// One operation.
#[derive(Debug, Clone)]
pub struct OpDef {
    /// Canonical, snake_case. The key `exec::plan` matches on and `tools.db`
    /// stores. The CLI accepts the kebab spelling and resolves it here.
    pub kind: &'static str,
    pub label: &'static str,
    pub cat: Category,
    /// One or two sentences on the card, and above the form.
    pub info: &'static str,
    /// What the right-hand pane draws for this operation.
    pub preview: Preview,
    /// The binaries this operation cannot run without. The tile greys out and
    /// says which one is missing, rather than queueing a job that dies at 4%.
    /// Empty means pure Rust: it works on a machine with nothing installed.
    pub needs: &'static [&'static str],
    pub fields: &'static [FieldDef],
}

/// The three download ops differ only in what yt-dlp is pointed at.
const DOWNLOAD_FIELDS: &[FieldDef] = &[
    text("url", "URL", "", true, "YouTube / Vimeo / 1800+ sites"),
    toggle("audio_only", "Audio only", "false", "extract audio"),
    pick(
        "max_height",
        "Resolution",
        "1080",
        &["Best", "2160", "1440", "1080", "720", "480", "360", "240"],
        false,
        "“Best” grabs the highest available",
    ),
    toggle("embed_subs", "Embed subtitles", "false", ""),
    toggle(
        "embed_thumbnail",
        "Embed thumbnail",
        "false",
        "cover art from the video thumbnail",
    ),
    folder("output", "Save folder (blank = Downloads)", false, ""),
];

/// Every operation. Order is the order the landing grid shows them in.
pub const CATALOG: &[OpDef] = &[
    // ------------------------------------------------------------ file ops --
    OpDef {
        kind: "rename",
        label: "Rename",
        cat: Category::FileOps,
        info: "Rename in bulk from a pattern. Collisions are refused rather than half-applied.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            folder("dir", "Folder", true, ""),
            text(
                "pattern",
                "Pattern",
                "{n:03}_{name}",
                true,
                "{n}, {n:03} = sequence · {name} = original name",
            ),
            number("start", "Start number", "1", false, ""),
        ],
    },
    OpDef {
        kind: "merge",
        label: "Merge",
        cat: Category::FileOps,
        info: "Join files end to end. They must share a codec — this copies streams rather than re-encoding.",
        preview: Preview::DryRun,
        needs: &["ffmpeg"],
        fields: &[
            files("inputs", "Source files", AV, "they must share a codec"),
            save("output", "Output file", true, ANY, ""),
        ],
    },
    OpDef {
        kind: "split",
        label: "Split",
        cat: Category::FileOps,
        info: "Cut into fixed-length segments.",
        preview: Preview::DryRun,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            number("every_s", "Segment length (seconds)", "60", true, ""),
            // `plan()` calls `req(spec, "output")` for this one, so "blank =
            // beside source" was never true: leaving it empty failed the plan.
            text(
                "output",
                "Output template",
                "",
                true,
                "e.g. /out/clip_%03d.mp4 — %03d numbers the segments",
            ),
        ],
    },
    OpDef {
        kind: "hash",
        label: "Hash",
        cat: Category::FileOps,
        info: "SHA-256 every file, to a manifest or to the report.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            files("files", "Files to hash", ANY, "SHA-256"),
            save(
                "manifest",
                "Save manifest to (blank = show inline)",
                false,
                &["txt", "sha256"],
                "",
            ),
        ],
    },
    OpDef {
        kind: "folder_diff",
        label: "Folder diff",
        cat: Category::FileOps,
        info: "Compare two folders by content, not by timestamp.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            folder("a", "Folder A", true, ""),
            folder("b", "Folder B", true, ""),
        ],
    },
    OpDef {
        kind: "archive_create",
        label: "Create archive",
        cat: Category::FileOps,
        info: "Zip a folder, or a handful of files. A folder goes in under its own name so unpacking it does not scatter.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            folder(
                "folder",
                "Folder to archive",
                false,
                "leave blank to use the file list",
            ),
            files(
                "files",
                "Or these files",
                ANY,
                "ignored when a folder is chosen",
            ),
            save("output", "Archive", true, &["zip"], ""),
            toggle(
                "store",
                "Store without compressing",
                "false",
                "faster, and honest about photos and video, which do not shrink",
            ),
        ],
    },
    OpDef {
        kind: "archive_extract",
        label: "Extract archive",
        cat: Category::FileOps,
        info: "Unpack into a folder. Members whose names try to climb out of it are listed and skipped.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            file("input", "Archive", ARCHIVE),
            folder("dir", "Unpack into", true, ""),
        ],
    },
    OpDef {
        kind: "mirror",
        label: "Mirror folders",
        cat: Category::FileOps,
        info: "Make the second folder match the first. Every copy and every deletion is listed before anything runs.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            folder("a", "Source folder", true, "the one that is right"),
            folder("b", "Folder to update", true, "the one that changes"),
            toggle(
                "delete_extra",
                "Delete what the source does not have",
                "false",
                "off by default — this is the setting that loses files",
            ),
        ],
    },
    OpDef {
        kind: "cache_clean",
        label: "Clean cache",
        cat: Category::FileOps,
        info: "Delete the app's thumbnail and temporary files.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[],
    },
    // `mediainfo` used to land here by accident — it matched no category arm
    // and the `_ =>` fallthrough was "queue", which is not a tab. It belongs in
    // File ops on purpose now.
    OpDef {
        kind: "mediainfo",
        label: "Media info",
        cat: Category::FileOps,
        info: "Report every stream, codec and bitrate.",
        preview: Preview::Plain,
        needs: &["ffprobe"],
        fields: &[
            file("input", "File to inspect", ANY),
            save(
                "output",
                "Report .txt (blank = show inline)",
                false,
                &["txt"],
                "codec/stream/bitrate report",
            ),
        ],
    },
    // --------------------------------------------------------------- video --
    OpDef {
        kind: "compress_video",
        label: "Compress video",
        cat: Category::Video,
        info: "Re-encode to a smaller file. CRF is the quality dial: lower is better and bigger, and every +6 roughly halves the size.",
        preview: Preview::Video,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            pick("codec", "Codec", "h264", &["h264", "h265", "av1"], true, ""),
            slider(
                "crf",
                "Quality — CRF (lower = better, bigger)",
                "23",
                18.0,
                35.0,
                "~18 near-lossless · 23 default · 28 small · each +6 ≈ half the size",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "trim",
        label: "Trim",
        cat: Category::Video,
        info: "Cut a section out. Lossless snaps to keyframes and does not re-encode, so it is fast and the cut may land a moment early.",
        preview: Preview::Video,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            number("start_s", "Start (seconds)", "0", true, "e.g. 12.5"),
            number("end_s", "End (seconds)", "", true, "e.g. 48"),
            toggle(
                "lossless",
                "Lossless cut (keyframe-aligned)",
                "true",
                "fast, no re-encode",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "convert",
        label: "Convert",
        cat: Category::Convert,
        info: "Change container or codec. Picking an audio extension on a video file extracts the audio.",
        preview: Preview::Convert,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source file", MEDIA),
            pick(
                "target_ext",
                "Convert to",
                "mp4",
                &[
                    "mp4", "mkv", "webm", "mp3", "m4a", "opus", "flac", "png", "jpg", "webp",
                ],
                true,
                "",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "audio_convert",
        label: "Audio converter",
        cat: Category::Convert,
        info: "Any source to any audio format, at a bitrate you choose. Video sources give up their soundtrack.",
        preview: Preview::Convert,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source file", AV),
            pick(
                "target_ext",
                "Convert to",
                "mp3",
                &["mp3", "m4a", "opus", "ogg", "flac", "wav"],
                true,
                "FLAC and WAV are lossless — the bitrate does not apply",
            ),
            slider("kbps", "Bitrate", "192", 64.0, 320.0, "kb/s"),
            save(
                "output",
                "Output file",
                false,
                AUDIO,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "archive_repack",
        label: "Archive repack",
        cat: Category::Convert,
        info: "Read an archive and write it out again — squeezed, or opened up for something that cannot inflate.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            file("input", "Archive", ARCHIVE),
            save(
                "output",
                "New archive",
                true,
                &["zip"],
                "must not be the same file",
            ),
            toggle("store", "Store without compressing", "false", ""),
        ],
    },
    OpDef {
        kind: "image_convert",
        label: "Image converter",
        cat: Category::Convert,
        info: "Any picture to JPEG, PNG, WebP or AVIF. PNG is lossless, so quality does nothing to it.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE),
            pick(
                "target_ext",
                "Convert to",
                "webp",
                &["jpg", "png", "webp", "avif"],
                true,
                "",
            ),
            slider("quality", "Quality", "85", 1.0, 100.0, "higher is bigger"),
            save(
                "output",
                "Output file",
                false,
                IMAGE,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "doc_convert",
        label: "Document converter",
        cat: Category::Convert,
        info: "Word, Markdown, HTML, LaTeX, ODT — any of them into any other. Needs pandoc installed; the tile stays grey until it is.",
        preview: Preview::Convert,
        needs: &["pandoc"],
        fields: &[
            file("input", "Source document", DOC),
            pick(
                "format",
                "Convert to",
                "docx",
                &["docx", "odt", "md", "html", "rtf", "tex", "epub", "txt"],
                true,
                "PDF is missing on purpose: pandoc needs a LaTeX engine for it",
            ),
            save(
                "output",
                "Output file",
                false,
                DOC,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "ebook_convert",
        label: "Ebook converter",
        cat: Category::Convert,
        info: "EPUB, MOBI, AZW3, FB2 and the rest, in any direction. Needs Calibre’s ebook-convert installed.",
        preview: Preview::Convert,
        needs: &["ebook-convert"],
        fields: &[
            file("input", "Source ebook", EBOOK),
            pick(
                "format",
                "Convert to",
                "epub",
                &["epub", "mobi", "azw3", "fb2", "pdf", "txt", "docx"],
                true,
                "",
            ),
            text("title", "Title (blank = keep)", "", false, ""),
            text("author", "Author (blank = keep)", "", false, ""),
            save(
                "output",
                "Output file",
                false,
                EBOOK,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "thumbnail",
        label: "Thumbnail",
        cat: Category::Video,
        info: "Grab one frame as an image.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            number("at_s", "At timestamp (seconds)", "1", false, ""),
            number("width", "Thumbnail width (px)", "320", false, ""),
            out(),
        ],
    },
    OpDef {
        kind: "resize",
        label: "Resize",
        cat: Category::Video,
        info: "Scale an image or video. Fit keeps the aspect ratio inside the box; exact will distort.",
        preview: Preview::Video,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image/video", IMAGE_VIDEO),
            pick(
                "mode",
                "Aspect",
                "fit",
                &["fit", "exact", "width", "height"],
                false,
                "fit keeps the aspect inside W×H · exact may distort · width/height scale one edge",
            ),
            number("w", "Width (px)", "1920", true, ""),
            number("h", "Height (px)", "1080", true, ""),
            out(),
        ],
    },
    OpDef {
        kind: "crop",
        label: "Crop & debar",
        cat: Category::Video,
        info: "Cut to a box or to an aspect ratio. The ratio presets take the black bars off without measuring them.",
        preview: Preview::Video,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source", IMAGE_VIDEO),
            pick(
                "aspect",
                "Crop to",
                "16:9",
                &["16:9", "4:3", "1:1", "9:16", "21:9", "manual"],
                true,
                "a ratio crops from the centre; “manual” uses the box below",
            ),
            number("w", "Width", "1920", false, "manual only"),
            number("h", "Height", "800", false, "manual only"),
            number("x", "Left", "0", false, "manual only"),
            number("y", "Top", "140", false, "manual only"),
            out(),
        ],
    },
    OpDef {
        kind: "rotate",
        label: "Rotate & flip",
        cat: Category::Video,
        info: "Turn in quarters and mirror. Re-encodes, because a rotation flag is what a player ignored to get you here.",
        preview: Preview::Video,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source", IMAGE_VIDEO),
            pick(
                "turn",
                "Rotate",
                "0",
                &["0", "90", "180", "270"],
                false,
                "degrees clockwise",
            ),
            toggle("hflip", "Mirror left to right", "false", ""),
            toggle("vflip", "Mirror top to bottom", "false", ""),
            out(),
        ],
    },
    OpDef {
        kind: "denoise",
        label: "Denoise & deblock",
        cat: Category::Video,
        info: "Take the grain out, and the blocking a low bitrate put in before you ever saw the file.",
        preview: Preview::Video,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source", IMAGE_VIDEO),
            slider(
                "strength",
                "Strength",
                "4",
                0.0,
                10.0,
                "4 is ffmpeg's own default",
            ),
            toggle(
                "deblock",
                "Deblock as well",
                "false",
                "for sources that were compressed hard once already",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "add_subs",
        label: "Add subtitle track",
        cat: Category::Video,
        info: "Mux a subtitle file in as a track that can be switched off. Nothing is re-encoded.",
        preview: Preview::Cues,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            file("sub", "Subtitle file", SUBS),
            text(
                "language",
                "Language",
                "eng",
                false,
                "three-letter code — eng, fra, deu",
            ),
            save(
                "output",
                "Output file",
                false,
                VIDEO,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "contact_sheet",
        label: "Contact sheet",
        cat: Category::Video,
        info: "One image of evenly spaced frames.",
        preview: Preview::Image,
        needs: &["ffmpeg", "ffprobe"],
        fields: &[
            file("input", "Source video", VIDEO),
            number("cols", "Columns", "4", false, ""),
            number("rows", "Rows", "4", false, ""),
            save(
                "output",
                "Output .jpg (blank = beside source)",
                false,
                &["jpg"],
                "",
            ),
        ],
    },
    OpDef {
        kind: "download",
        label: "Download",
        cat: Category::Video,
        info: "Fetch a video from any of the sites yt-dlp supports.",
        preview: Preview::Plain,
        needs: &["yt-dlp"],
        fields: DOWNLOAD_FIELDS,
    },
    OpDef {
        kind: "download_live",
        label: "Live record",
        cat: Category::Video,
        info: "Record a live stream until it ends or you cancel.",
        preview: Preview::Plain,
        needs: &["yt-dlp"],
        fields: DOWNLOAD_FIELDS,
    },
    OpDef {
        kind: "download_playlist",
        label: "Playlist download",
        cat: Category::Video,
        info: "Fetch every video in a playlist.",
        preview: Preview::Plain,
        needs: &["yt-dlp"],
        fields: DOWNLOAD_FIELDS,
    },
    // --------------------------------------------------------------- audio --
    OpDef {
        kind: "compress_audio",
        label: "Compress audio",
        cat: Category::Audio,
        info: "Re-encode audio at a chosen bitrate. Opus is the best of these per kilobit; mp3 is the one everything plays.",
        preview: Preview::Wave,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source audio", AUDIO),
            pick(
                "codec",
                "Codec",
                "mp3",
                &["mp3", "aac", "opus", "vorbis"],
                true,
                "",
            ),
            slider("kbps", "Bitrate (kbps)", "192", 64.0, 320.0, ""),
            out(),
        ],
    },
    OpDef {
        kind: "normalize",
        label: "Normalise",
        cat: Category::Audio,
        info: "Level the loudness to a target. -23 LUFS is the broadcast standard.",
        preview: Preview::Wave,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source audio/video", AV),
            number(
                "lufs",
                "Target loudness (LUFS)",
                "-23",
                false,
                "EBU R128 = -23",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "denoise_audio",
        label: "Clean up noise",
        cat: Category::Audio,
        info: "Broadband hiss out, and the rumble a hand on the microphone put in. Video sources keep their picture untouched.",
        preview: Preview::Wave,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source", AV),
            slider(
                "reduction",
                "Noise reduction",
                "12",
                1.0,
                40.0,
                "dB — past about 20 it starts to sound underwater",
            ),
            toggle(
                "rumble",
                "Cut rumble below 80 Hz",
                "true",
                "handling noise, air conditioning",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "extract",
        label: "Extract audio",
        cat: Category::Audio,
        info: "Pull an audio or subtitle track out into its own file.",
        preview: Preview::Wave,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            pick(
                "stream",
                "Stream",
                "audio",
                &["audio", "subtitle"],
                true,
                "",
            ),
            number("index", "Track index", "0", false, "0 = first"),
            out(),
        ],
    },
    // --------------------------------------------------------------- photo --
    OpDef {
        kind: "stems",
        label: "Separate stems",
        cat: Category::Audio,
        info: "Split a track into vocals, drums, bass and the rest. Needs demucs installed, and it is slow — minutes per song, on the processor.",
        preview: Preview::DryRun,
        needs: &["demucs"],
        fields: &[
            file("input", "Source track", AV),
            pick(
                "mode",
                "Split into",
                "four",
                &["four", "vocals"],
                true,
                "“vocals” is vocals and everything else — twice as fast",
            ),
            toggle(
                "mp3",
                "Write MP3",
                "false",
                "off = wav, which is exact and large",
            ),
            folder(
                "output",
                "Save folder (blank = beside source)",
                false,
                "demucs writes a folder per track, not a file",
            ),
        ],
    },
    OpDef {
        kind: "compress_photo",
        label: "Compress photo",
        cat: Category::Photo,
        info: "Re-encode an image. AVIF is smallest, WebP is widely supported, JPEG is universal.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE),
            pick(
                "format",
                "Format",
                "jpeg",
                &["jpeg", "webp", "avif"],
                true,
                "",
            ),
            slider("quality", "Quality", "82", 1.0, 100.0, "1–100"),
            out(),
        ],
    },
    OpDef {
        kind: "watermark",
        label: "Watermark",
        cat: Category::Photo,
        info: "Burn a line of text into the bottom-right corner.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image/video", IMAGE_VIDEO),
            text("text", "Watermark text", "", true, "shown bottom-right"),
            out(),
        ],
    },
    // ----------------------------------------------------------------- pdf --
    OpDef {
        kind: "pdf_pages",
        label: "Extract pages",
        cat: Category::Pdf,
        info: "Keep the pages you name and nothing else. The rest of the document is left alone.",
        preview: Preview::Pages,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            text(
                "ranges",
                "Pages to keep",
                "1-3",
                true,
                "1-3,5,8-10 — commas and dashes, one-based",
            ),
            save(
                "output",
                "Output PDF",
                false,
                PDF,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "pdf_delete",
        label: "Delete pages",
        cat: Category::Pdf,
        info: "Drop the pages you name. Every page is listed first, struck through where it is going.",
        preview: Preview::Pages,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            text(
                "ranges",
                "Pages to delete",
                "",
                true,
                "1-3,5,8-10 — commas and dashes, one-based",
            ),
            save(
                "output",
                "Output PDF",
                false,
                PDF,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "pdf_stamp",
        label: "Number & stamp",
        cat: Category::Pdf,
        info: "Put page numbers, or a word, on every page. Helvetica, so nothing is embedded and the file barely grows.",
        preview: Preview::Pages,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            text(
                "pattern",
                "What to write",
                "{n}",
                true,
                "{n} = page number · {total} = how many there are",
            ),
            pick(
                "corner",
                "Where",
                "bottom_centre",
                &[
                    "bottom_centre",
                    "bottom_right",
                    "bottom_left",
                    "top_centre",
                    "top_right",
                ],
                true,
                "",
            ),
            slider("size", "Size", "10", 6.0, 36.0, "points"),
            text(
                "ranges",
                "Pages (blank = all)",
                "",
                false,
                "1-3,5,8-10 — commas and dashes, one-based",
            ),
            save(
                "output",
                "Output PDF",
                false,
                PDF,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "pdf_rotate",
        label: "Rotate pages",
        cat: Category::Pdf,
        info: "Turn pages in quarters. A scanner that fed one page sideways only did it to some of them.",
        preview: Preview::Pages,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            text(
                "ranges",
                "Pages to turn (blank = all)",
                "",
                false,
                "1-3,5,8-10 — commas and dashes, one-based",
            ),
            pick(
                "turn",
                "Turn by",
                "90",
                &["90", "180", "270"],
                true,
                "degrees clockwise, on top of whatever the page already has",
            ),
            save(
                "output",
                "Output PDF",
                false,
                PDF,
                "blank = beside the source",
            ),
        ],
    },
    // ----------------------------------------------------------- subtitles --
    OpDef {
        kind: "transcribe",
        label: "Transcribe",
        cat: Category::Subtitles,
        info: "Speech to an SRT subtitle file, locally, with Whisper.",
        preview: Preview::Plain,
        needs: &["ffmpeg", "whisper-cli"],
        fields: &[
            file("input", "Audio/video file", AV),
            pick(
                "language",
                "Source language",
                "auto",
                &[
                    "auto", "en", "es", "fr", "de", "it", "pt", "nl", "ru", "uk", "pl", "tr", "ar",
                    "fa", "hi", "ur", "bn", "ta", "th", "vi", "id", "ja", "ko", "zh",
                ],
                false,
                "auto-detect, or pick to sharpen accuracy",
            ),
            toggle(
                "translate",
                "Translate to English",
                "false",
                "transcribe any language straight to English",
            ),
            save(
                "output",
                "Output .srt (blank = beside source)",
                false,
                &["srt"],
                "",
            ),
        ],
    },
    OpDef {
        kind: "burn_subs",
        label: "Burn subtitles",
        cat: Category::Subtitles,
        info: "Render a subtitle file into the picture, permanently.",
        preview: Preview::Cues,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            file("sub", "Subtitle file (.srt/.ass)", SUBS),
            out(),
        ],
    },
    OpDef {
        kind: "subs_sync",
        label: "Auto-sync to audio",
        cat: Category::Subtitles,
        info: "Shift and stretch a subtitle file until it lines up with what is being said. Needs ffsubsync installed.",
        preview: Preview::Cues,
        needs: &["ffsubsync"],
        fields: &[
            file("input", "Source video", AV),
            file("sub", "Subtitle file (.srt/.ass)", SUBS),
            save(
                "output",
                "Output subtitle",
                false,
                SUBS,
                "blank = beside the subtitle, not the video",
            ),
        ],
    },
];

// ------------------------------------------------------------------ lookup ---

pub fn get(kind: &str) -> Option<&'static OpDef> {
    CATALOG.iter().find(|o| o.kind == kind)
}

pub fn kinds() -> impl Iterator<Item = &'static str> {
    CATALOG.iter().map(|o| o.kind)
}

pub fn in_category(cat: Category) -> impl Iterator<Item = &'static OpDef> {
    CATALOG.iter().filter(move |o| o.cat == cat)
}

/// Resolve a name the user typed to a canonical kind.
///
/// The CLI has always spelled ops in kebab-case and the GUI in snake_case, and
/// nothing translated between them — `tulipix compress-video` parsed fine and
/// then reached `plan()` as a kind it did not know. Both spellings resolve here.
pub fn resolve(name: &str) -> Option<&'static str> {
    kinds()
        .find(|k| *k == name)
        .or_else(|| kinds().find(|k| kebab_eq(k, name)))
}

fn kebab_eq(kind: &str, name: &str) -> bool {
    kind.len() == name.len()
        && kind
            .bytes()
            .zip(name.bytes())
            .all(|(a, b)| a == b || (a == b'_' && b == b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn kinds_are_unique_and_snake_case() {
        let mut seen = HashSet::new();
        for op in CATALOG {
            assert!(seen.insert(op.kind), "duplicate kind: {}", op.kind);
            assert!(
                op.kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "kind is not snake_case: {}",
                op.kind
            );
        }
    }

    #[test]
    fn every_op_has_a_label_and_a_blurb() {
        for op in CATALOG {
            assert!(!op.label.is_empty(), "{} has no label", op.kind);
            assert!(!op.info.is_empty(), "{} has no blurb", op.kind);
        }
    }

    #[test]
    fn field_keys_are_unique_within_an_op() {
        for op in CATALOG {
            let mut seen = HashSet::new();
            for f in op.fields {
                assert!(seen.insert(f.key), "{}: duplicate field {}", op.kind, f.key);
            }
        }
    }

    #[test]
    fn sliders_have_a_usable_range_and_a_default_inside_it() {
        for op in CATALOG {
            for f in op.fields.iter().filter(|f| f.kind == FieldKind::Slider) {
                assert!(f.min < f.max, "{}.{}: empty range", op.kind, f.key);
                let v: f64 = f.value.parse().unwrap_or_else(|_| {
                    panic!(
                        "{}.{}: default {:?} is not a number",
                        op.kind, f.key, f.value
                    )
                });
                assert!(
                    v >= f.min && v <= f.max,
                    "{}.{}: default {v} outside {}..{}",
                    op.kind,
                    f.key,
                    f.min,
                    f.max
                );
            }
        }
    }

    #[test]
    fn dropdown_defaults_are_one_of_the_options() {
        for op in CATALOG {
            for f in op.fields.iter().filter(|f| f.kind == FieldKind::Dropdown) {
                assert!(!f.options.is_empty(), "{}.{}: no options", op.kind, f.key);
                assert!(
                    f.options.contains(&f.value),
                    "{}.{}: default {:?} is not an option",
                    op.kind,
                    f.key,
                    f.value
                );
            }
        }
    }

    #[test]
    fn every_category_has_at_least_one_op() {
        for c in Category::ALL {
            assert!(in_category(c).next().is_some(), "{} is empty", c.id());
        }
    }

    #[test]
    fn mediainfo_is_in_a_real_category() {
        // It used to fall through every arm to "queue", which is not a tab, so
        // it rendered nowhere. Regression guard.
        assert_eq!(get("mediainfo").unwrap().cat, Category::FileOps);
    }

    #[test]
    fn extensions_are_bare_and_lowercase() {
        // `XTypeGroup` wants "mp4", not ".mp4" or "MP4"; a dot here filters
        // every dialog down to nothing and no error says why.
        for op in CATALOG {
            for f in op.fields {
                for e in f.ext {
                    assert!(
                        !e.starts_with('.'),
                        "{}.{}: {e:?} has a dot",
                        op.kind,
                        f.key
                    );
                    assert!(!e.is_empty(), "{}.{}: empty extension", op.kind, f.key);
                    assert_eq!(*e, e.to_lowercase(), "{}.{}: {e:?}", op.kind, f.key);
                }
            }
        }
    }

    #[test]
    fn only_path_fields_carry_a_filter() {
        for op in CATALOG {
            for f in op.fields {
                let path = matches!(
                    f.kind,
                    FieldKind::File | FieldKind::Files | FieldKind::Save | FieldKind::Folder
                );
                assert!(
                    path || f.ext.is_empty(),
                    "{}.{}: a {} field has extensions",
                    op.kind,
                    f.key,
                    f.kind.as_str()
                );
            }
        }
    }

    #[test]
    fn nothing_that_writes_is_still_a_text_box() {
        // Phase 2's rule: a path is chosen, never typed. `split.output` is the
        // one exception and is deliberate — it is an ffmpeg template with a
        // `%03d` in it, which no save dialog can express.
        for op in CATALOG {
            for f in op.fields.iter().filter(|f| f.kind == FieldKind::Text) {
                let writes = f.key == "output" || f.key == "manifest";
                assert!(
                    !writes || (op.kind == "split" && f.key == "output"),
                    "{}.{} writes a file but is a text box",
                    op.kind,
                    f.key
                );
            }
        }
    }

    #[test]
    fn resolve_accepts_both_spellings() {
        assert_eq!(resolve("compress_video"), Some("compress_video"));
        assert_eq!(resolve("compress-video"), Some("compress_video"));
        assert_eq!(resolve("folder-diff"), Some("folder_diff"));
        assert_eq!(resolve("frobnicate"), None);
        // A hyphen where the kind has a letter must not match.
        assert_eq!(resolve("compress-vide-"), None);
    }
}
