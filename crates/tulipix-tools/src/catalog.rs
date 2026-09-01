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
/// Tables, in and out.
const DATA: &[&str] = &["csv", "tsv", "tab", "json", "xlsx", "xlsm"];
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
    required: bool,
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
        required,
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
            files("inputs", "Source files", AV, true, "they must share a codec"),
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
            files("files", "Files to hash", ANY, true, "SHA-256"),
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
                "or leave blank and add files below",
            ),
            files(
                "files",
                "Or these files",
                ANY,
                // Either half will do. Marking both required meant Run
                // refused a folder on its own, which is the ordinary way to
                // use this.
                false,
                "or leave blank and choose a folder above",
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
        kind: "encrypt",
        label: "Encrypt or decrypt",
        cat: Category::FileOps,
        info: "Lock a file behind a passphrase, or open one that is. Uses age, so anything sealed here opens anywhere the age command exists.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            file("input", "File", ANY),
            pick(
                "mode",
                "Do what",
                "encrypt",
                &["encrypt", "decrypt"],
                true,
                "",
            ),
            text(
                "passphrase",
                "Passphrase",
                "",
                true,
                "there is no way to recover this — write it down somewhere",
            ),
            save(
                "output",
                "Output file",
                false,
                ANY,
                "blank = the same name with .age added, or taken off",
            ),
        ],
    },
    OpDef {
        kind: "dedupe",
        label: "Find duplicates",
        cat: Category::FileOps,
        info: "Files with identical contents, wherever they are and whatever they are called. Sizes are compared first, so only real candidates are read.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            folder("dir", "Folder", true, "searched all the way down"),
            toggle(
                "delete",
                "Delete the extra copies",
                "false",
                "keeps the first of each set — read the list before turning this on",
            ),
            save(
                "manifest",
                "Save the list (optional)",
                false,
                &["txt", "csv"],
                "",
            ),
        ],
    },
    OpDef {
        kind: "sort_files",
        label: "Sort into folders",
        cat: Category::FileOps,
        info: "File everything loose in a folder into subfolders, by type, by month, or by first letter.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            folder(
                "dir",
                "Folder",
                true,
                "only the files sitting directly in it",
            ),
            pick(
                "by",
                "Sort by",
                "extension",
                &["extension", "date", "letter"],
                true,
                "date reads the file’s own modified time",
            ),
        ],
    },
    OpDef {
        kind: "empty_dirs",
        label: "Remove empty folders",
        cat: Category::FileOps,
        info: "Delete folders that hold no files at any depth. A folder that could not be read is left alone.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[folder(
            "dir",
            "Folder",
            true,
            "the folder itself is never deleted",
        )],
    },
    OpDef {
        kind: "file_list",
        label: "Export a listing",
        cat: Category::FileOps,
        info: "Every file under a folder as a spreadsheet — path, size, month.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            folder("dir", "Folder", true, ""),
            save(
                "output",
                "Save as",
                false,
                &["csv"],
                "blank = beside the folder",
            ),
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
        kind: "data_convert",
        label: "Data converter",
        cat: Category::Convert,
        info: "CSV, TSV, JSON and Excel, in any direction. Formulas and formatting do not survive — the values do.",
        preview: Preview::Convert,
        needs: &[],
        fields: &[
            file("input", "Source table", DATA),
            pick(
                "format",
                "Convert to",
                "csv",
                &["csv", "tsv", "json", "xlsx"],
                true,
                "JSON is an array of objects keyed by the header row",
            ),
            save(
                "output",
                "Output file",
                false,
                DATA,
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
        kind: "speed",
        label: "Change speed",
        cat: Category::Video,
        info: "Speed a video up or slow it down. The sound follows, and can keep its pitch.",
        preview: Preview::Convert,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            slider(
                "rate",
                "Speed",
                "2",
                0.25,
                4.0,
                "2 is twice as fast, 0.5 is half",
            ),
            toggle(
                "keep_pitch",
                "Keep the pitch",
                "true",
                "off makes voices squeak or growl, which is sometimes the point",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "gif",
        label: "Make a GIF",
        cat: Category::Video,
        info: "A few seconds of video as an animated GIF or WebP, with a palette built from the clip itself.",
        preview: Preview::Convert,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            number("start_s", "Start (seconds)", "0", true, ""),
            number(
                "seconds",
                "Length (seconds)",
                "5",
                true,
                "keep it short — GIF is enormous",
            ),
            slider("width", "Width", "480", 120.0, 1280.0, "height follows"),
            slider("fps", "Frames a second", "15", 5.0, 30.0, ""),
            save(
                "output",
                "Output file",
                false,
                &["gif", "webp"],
                "blank = beside the source · .webp is far smaller",
            ),
        ],
    },
    OpDef {
        kind: "fade",
        label: "Fade in & out",
        cat: Category::Video,
        info: "Fade up from black at the start and down at the end, picture and sound together.",
        preview: Preview::Convert,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            slider("in_s", "Fade in", "1", 0.0, 10.0, "seconds · 0 for none"),
            slider("out_s", "Fade out", "1", 0.0, 10.0, "seconds · 0 for none"),
            out(),
        ],
    },
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
        kind: "tags",
        label: "Edit tags & artwork",
        cat: Category::Audio,
        info: "Title, artist, album and cover art. Blank fields are left as they are rather than cleared.",
        preview: Preview::Plain,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source track", AUDIO),
            text("title", "Title", "", false, ""),
            text("artist", "Artist", "", false, ""),
            text("album", "Album", "", false, ""),
            text("year", "Year", "", false, ""),
            file("cover", "Cover image", IMAGE),
            out(),
        ],
    },
    OpDef {
        kind: "silence_trim",
        label: "Trim silence",
        cat: Category::Audio,
        info: "Cut the dead air off the ends, or out of the middle as well.",
        preview: Preview::Wave,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source", AV),
            slider(
                "threshold",
                "Counts as silence below",
                "-50",
                -80.0,
                -20.0,
                "dB",
            ),
            slider(
                "min_ms",
                "Shorter gaps are kept",
                "500",
                100.0,
                5000.0,
                "milliseconds",
            ),
            toggle(
                "middle",
                "Also cut the middle",
                "false",
                "off trims only the start and end, which is what a recording usually needs",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "audio_speed",
        label: "Speed & pitch",
        cat: Category::Audio,
        info: "Play faster or slower, with or without moving the pitch. Separating the two is the difference between a podcast at 1.5× and a chipmunk.",
        preview: Preview::Wave,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source track", AUDIO),
            slider("rate", "Speed", "1.5", 0.25, 4.0, ""),
            toggle("keep_pitch", "Keep the pitch", "true", ""),
            out(),
        ],
    },
    OpDef {
        kind: "replace_audio",
        label: "Replace the soundtrack",
        cat: Category::Audio,
        info: "Swap a video’s audio for another file. The picture is copied through untouched, so this is quick whatever the film’s length.",
        preview: Preview::Wave,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source video", VIDEO),
            file("audio", "New soundtrack", AUDIO),
            number(
                "offset_s",
                "Nudge the audio (seconds)",
                "0",
                false,
                "negative starts it earlier",
            ),
            toggle(
                "shortest",
                "Stop at whichever ends first",
                "true",
                "off keeps the picture running in silence",
            ),
            out(),
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
        kind: "strip_meta",
        label: "Remove metadata",
        cat: Category::Photo,
        info: "Strip EXIF, GPS and camera details out of a picture. The pixels are copied through, so nothing is re-encoded.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[file("input", "Source image", IMAGE), out()],
    },
    OpDef {
        kind: "border",
        label: "Add a border",
        cat: Category::Photo,
        info: "A margin of flat colour around a picture — even on all sides, or wider at the bottom the way a print is mounted.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE),
            slider("width", "Border", "40", 0.0, 300.0, "pixels"),
            slider("bottom", "Extra at the bottom", "0", 0.0, 300.0, "pixels"),
            text("colour", "Colour", "white", true, "a name, or #rrggbb"),
            out(),
        ],
    },
    OpDef {
        kind: "collage",
        label: "Photo collage",
        cat: Category::Photo,
        info: "Several pictures on one canvas, in a grid. They are scaled to a common size first, so mixed shapes still line up.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            files("inputs", "Pictures", IMAGE, true, "in the order you pick them"),
            slider("cols", "Columns", "3", 1.0, 8.0, ""),
            slider("cell", "Each cell", "480", 120.0, 1600.0, "pixels square"),
            slider("gap", "Gap", "8", 0.0, 60.0, "pixels"),
            text("colour", "Background", "white", true, "a name, or #rrggbb"),
            save(
                "output",
                "Output image",
                false,
                IMAGE,
                "blank = beside the first picture",
            ),
        ],
    },
    OpDef {
        kind: "adjust",
        label: "Brightness & colour",
        cat: Category::Photo,
        info: "Brightness, contrast, saturation and gamma, on one screen with the before beside it.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE),
            slider(
                "brightness",
                "Brightness",
                "0",
                -1.0,
                1.0,
                "0 leaves it alone",
            ),
            slider("contrast", "Contrast", "1", 0.0, 3.0, "1 leaves it alone"),
            slider("saturation", "Saturation", "1", 0.0, 3.0, "0 is greyscale"),
            slider(
                "gamma",
                "Gamma",
                "1",
                0.1,
                3.0,
                "lifts the midtones without touching black",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "recolour",
        label: "Black & white and looks",
        cat: Category::Photo,
        info: "Greyscale, sepia, a colour-negative, or a hue rotation. One dropdown, no sliders to get wrong.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE),
            pick(
                "look",
                "Look",
                "mono",
                &["mono", "sepia", "negative", "warm", "cool"],
                true,
                "",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "sharpen",
        label: "Sharpen",
        cat: Category::Photo,
        info: "Unsharp masking — the same thing every photo editor calls sharpening. Past about 1.5 it starts to look like it.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE),
            slider("amount", "Amount", "1", 0.0, 3.0, ""),
            slider(
                "radius",
                "Radius",
                "5",
                3.0,
                13.0,
                "odd numbers only — it is rounded",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "censor",
        label: "Hide a region",
        cat: Category::Photo,
        info: "Blur or pixelate a rectangle — a face, a number plate, an address on an envelope. The region is given in pixels from the top left.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE_VIDEO),
            number("x", "Left", "0", true, "pixels"),
            number("y", "Top", "0", true, "pixels"),
            number("w", "Width", "200", true, "pixels"),
            number("h", "Height", "200", true, "pixels"),
            pick(
                "style",
                "Style",
                "pixelate",
                &["pixelate", "blur", "solid"],
                true,
                "",
            ),
            out(),
        ],
    },
    OpDef {
        kind: "favicon",
        label: "Icon & favicon set",
        cat: Category::Photo,
        info: "One square picture into every size a site or an app asks for, plus the .ico and the HTML to paste.",
        preview: Preview::DryRun,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE),
            text(
                "name",
                "Base name",
                "favicon",
                true,
                "favicon-32.png, favicon.ico, and so on",
            ),
            folder("output", "Save folder", false, "blank = beside the source"),
        ],
    },
    OpDef {
        kind: "remove_bg",
        label: "Remove background",
        cat: Category::Photo,
        info: "Cut the subject out and leave the rest transparent. Needs rembg installed; the first run downloads its model.",
        preview: Preview::Convert,
        needs: &["rembg"],
        fields: &[
            file("input", "Source image", IMAGE),
            save(
                "output",
                "Output image",
                false,
                &["png", "webp"],
                "PNG or WebP — the others cannot hold transparency",
            ),
        ],
    },
    OpDef {
        kind: "upscale",
        label: "Enlarge",
        cat: Category::Photo,
        info: "Scale a picture up with Lanczos, which is as good as resampling gets without inventing detail.",
        preview: Preview::Image,
        needs: &["ffmpeg"],
        fields: &[
            file("input", "Source image", IMAGE),
            pick("factor", "Enlarge by", "2", &["2", "3", "4"], true, "×"),
            out(),
        ],
    },
    OpDef {
        kind: "photo_batch",
        label: "Resize a folder",
        cat: Category::Photo,
        info: "Every picture in a folder scaled to fit a box, written to a folder beside it. Nothing is overwritten.",
        preview: Preview::DryRun,
        needs: &["ffmpeg"],
        fields: &[
            folder("dir", "Folder of pictures", true, "not searched downwards"),
            slider("max_px", "Longest side", "1920", 240.0, 6000.0, "pixels"),
            slider("quality", "Quality", "85", 1.0, 100.0, ""),
            pick(
                "target_ext",
                "Save as",
                "jpg",
                &["jpg", "png", "webp", "avif"],
                true,
                "",
            ),
            folder(
                "output",
                "Save folder",
                false,
                "blank = a “resized” folder beside it",
            ),
        ],
    },
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
        kind: "pdf_merge",
        label: "Merge PDFs",
        cat: Category::Pdf,
        info: "Several PDFs into one, in the order you pick them. Bookmarks are dropped; the pages are not touched.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            files("inputs", "PDFs", PDF, true, "at least two, in order"),
            save("output", "Output PDF", true, PDF, ""),
        ],
    },
    OpDef {
        kind: "pdf_split",
        label: "Split into files",
        cat: Category::Pdf,
        info: "Break one PDF into several — every so many pages, or at the pages you name.",
        preview: Preview::Pages,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            pick("mode", "Split", "every", &["every", "at"], true, ""),
            number(
                "every",
                "Every N pages",
                "10",
                false,
                "when splitting every",
            ),
            text(
                "at",
                "Start a new file at",
                "",
                false,
                "when splitting at · 4,8,12",
            ),
            text(
                "template",
                "Name each part",
                "{name}-{n}",
                true,
                "{name} = the source · {n} = 01, 02…",
            ),
            folder("output", "Save folder", false, "blank = beside the source"),
        ],
    },
    OpDef {
        kind: "pdf_impose",
        label: "Impose & booklet",
        cat: Category::Pdf,
        info: "Lay the pages onto bigger sheets. Booklet order means printing double-sided, folding the stack once and stapling the spine.",
        preview: Preview::Pages,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            pick(
                "layout",
                "Layout",
                "booklet",
                &["booklet", "2up", "4up"],
                true,
                "booklet reorders the pages; 2up and 4up do not",
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
        kind: "pdf_redact",
        label: "Redact",
        cat: Category::Pdf,
        info: "Take words out of the file itself. Nothing is covered over — a black box leaves the text underneath, where anyone can copy it back out.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            text(
                "words",
                "Words to remove",
                "",
                true,
                "one per line, or separated by commas · not case sensitive",
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
        kind: "pdf_forms",
        label: "Fill & flatten forms",
        cat: Category::Pdf,
        info: "Fill in a PDF form’s fields. Flattening prints the answers onto the page and removes the form, so nothing can be edited afterwards.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            text(
                "values",
                "Values",
                "",
                true,
                "one per line: field = value · the preview lists the field names",
            ),
            toggle(
                "flatten",
                "Flatten",
                "true",
                "off leaves it a fillable form",
            ),
            slider(
                "size",
                "Text size",
                "10",
                6.0,
                24.0,
                "points, when flattening",
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
        kind: "pdf_images",
        label: "Extract images",
        cat: Category::Pdf,
        info: "Pull the pictures out of a PDF. JPEGs come out exactly as they went in; images stored another way are counted and left.",
        preview: Preview::DryRun,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            folder(
                "output",
                "Save folder",
                false,
                "blank = a folder beside the source",
            ),
        ],
    },
    OpDef {
        kind: "pdf_text",
        label: "Extract text",
        cat: Category::Pdf,
        info: "The text of a PDF as a plain file, one page after another. A scanned document has no text in it — that is a job for Transcribe’s OCR sibling, not this.",
        preview: Preview::Plain,
        needs: &[],
        fields: &[
            file("input", "Source PDF", PDF),
            text("ranges", "Pages (blank = all)", "", false, "1-3,5,8-10"),
            save(
                "output",
                "Save as",
                false,
                &["txt"],
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "pdf_from_images",
        label: "Pictures to PDF",
        cat: Category::Pdf,
        info: "One picture per page, each page the size of its picture. Anything that is not already a JPEG is converted first.",
        preview: Preview::DryRun,
        needs: &["ffmpeg"],
        fields: &[
            files("inputs", "Pictures", IMAGE, true, "in the order you pick them"),
            save("output", "Output PDF", true, PDF, ""),
        ],
    },
    OpDef {
        kind: "pdf_compress",
        label: "Shrink a PDF",
        cat: Category::Pdf,
        info: "Downsample the images inside a PDF. Needs Ghostscript installed — nothing bundled can re-encode a PDF’s pictures.",
        preview: Preview::Convert,
        needs: &["gs"],
        fields: &[
            file("input", "Source PDF", PDF),
            pick(
                "quality",
                "For",
                "ebook",
                &["screen", "ebook", "printer", "prepress"],
                true,
                "screen is smallest, prepress barely shrinks",
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
        kind: "pdf_protect",
        label: "Password protect",
        cat: Category::Pdf,
        info: "Put a password on a PDF, or take a known one off. Needs qpdf installed.",
        preview: Preview::Plain,
        needs: &["qpdf"],
        fields: &[
            file("input", "Source PDF", PDF),
            pick(
                "mode",
                "Do what",
                "protect",
                &["protect", "unlock"],
                true,
                "",
            ),
            text(
                "password",
                "Password",
                "",
                true,
                "the one to set, or the one it already has",
            ),
            toggle("allow_print", "Allow printing", "true", "when protecting"),
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
    OpDef {
        kind: "subs_convert",
        label: "Change format",
        cat: Category::Subtitles,
        info: "SRT, WebVTT and ASS, in any direction. Styling does not survive the trip out of ASS.",
        preview: Preview::Cues,
        needs: &["ffmpeg"],
        fields: &[
            file("sub", "Subtitle file", SUBS),
            pick(
                "format",
                "Convert to",
                "srt",
                &["srt", "vtt", "ass"],
                true,
                "",
            ),
            save(
                "output",
                "Output subtitle",
                false,
                SUBS,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "subs_shift",
        label: "Shift & retime",
        cat: Category::Subtitles,
        info: "Move every line earlier or later, and stretch the timing if the subtitle was written for a different frame rate.",
        preview: Preview::Cues,
        needs: &[],
        fields: &[
            file("sub", "Subtitle file", SUBS),
            number(
                "by_s",
                "Shift by (seconds)",
                "0",
                true,
                "negative is earlier",
            ),
            pick(
                "rate",
                "Frame rate fix",
                "none",
                &[
                    "none",
                    "25 to 23.976",
                    "23.976 to 25",
                    "30 to 29.97",
                    "29.97 to 30",
                ],
                true,
                "for a subtitle that drifts further out the longer it runs",
            ),
            save(
                "output",
                "Output subtitle",
                false,
                SUBS,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "subs_clean",
        label: "Clean up",
        cat: Category::Subtitles,
        info: "Strip formatting tags, pull overlapping lines apart, drop empty ones, and hold every line on screen long enough to read.",
        preview: Preview::Cues,
        needs: &[],
        fields: &[
            file("sub", "Subtitle file", SUBS),
            toggle(
                "strip_tags",
                "Remove formatting",
                "true",
                "<i>, <font>, {\\pos(…)}",
            ),
            toggle("fix_overlaps", "Pull overlaps apart", "true", ""),
            toggle("drop_empty", "Drop empty lines", "true", ""),
            slider(
                "min_ms",
                "Shortest line",
                "800",
                0.0,
                3000.0,
                "milliseconds · 0 leaves them",
            ),
            save(
                "output",
                "Output subtitle",
                false,
                SUBS,
                "blank = beside the source",
            ),
        ],
    },
    OpDef {
        kind: "subs_translate",
        label: "Translate",
        cat: Category::Subtitles,
        info: "Translate a subtitle file, keeping every timing exactly where it was. Needs argos-translate installed, which runs entirely on this machine.",
        preview: Preview::Cues,
        needs: &["argos-translate"],
        fields: &[
            file("sub", "Subtitle file", SUBS),
            text("from", "From", "en", true, "two-letter code"),
            text("to", "To", "es", true, "two-letter code"),
            save(
                "output",
                "Output subtitle",
                false,
                SUBS,
                "blank = beside the source",
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
