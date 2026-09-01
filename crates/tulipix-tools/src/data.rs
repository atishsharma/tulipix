//! Spreadsheets and the flat formats around them — CSV, TSV, JSON and XLSX,
//! in any direction.
//!
//! Everything passes through one shape: a sheet is a name and a grid of
//! strings. That is lossy on purpose. A converter that tried to keep formulas,
//! number formats and merged cells would be a spreadsheet engine, and the job
//! here is to get a table out of one file and into another without a detour
//! through Excel.
//!
//! CSV and TSV are parsed to RFC 4180 rather than by splitting on the
//! separator: a quoted field holding a comma is the first row of half the
//! exports in the world, and splitting mangles it silently.

use anyhow::{Context, Result, anyhow};

/// A table. `rows` is ragged — a short row is a short row, not an error.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

impl Sheet {
    /// The widest row, which is how many columns a writer has to allow for.
    pub fn width(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }
}

/// The formats this understands, in and out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Json,
    Xlsx,
}

impl Format {
    pub fn of(path: &str) -> Option<Format> {
        let ext = std::path::Path::new(path)
            .extension()?
            .to_string_lossy()
            .to_lowercase();
        Format::parse(&ext)
    }

    pub fn parse(ext: &str) -> Option<Format> {
        match ext.trim_start_matches('.') {
            "csv" => Some(Format::Csv),
            "tsv" | "tab" => Some(Format::Tsv),
            "json" => Some(Format::Json),
            // `xls` is a different format entirely and calamine reads it, but
            // nothing here writes it, so it is not offered in either direction.
            "xlsx" | "xlsm" => Some(Format::Xlsx),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Json => "json",
            Format::Xlsx => "xlsx",
        }
    }
}

pub fn read(path: &str) -> Result<Sheet> {
    let format = Format::of(path).ok_or_else(|| anyhow!("{path}: not a table this can read"))?;
    let name = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Sheet1".into());
    match format {
        Format::Xlsx => read_xlsx(path, name),
        Format::Json => {
            let body =
                std::fs::read_to_string(path).with_context(|| format!("cannot read {path}"))?;
            read_json(&body, name)
        }
        Format::Csv | Format::Tsv => {
            let body =
                std::fs::read_to_string(path).with_context(|| format!("cannot read {path}"))?;
            let sep = if format == Format::Tsv { '\t' } else { ',' };
            Ok(Sheet {
                name,
                rows: parse_delimited(&body, sep),
            })
        }
    }
}

pub fn write(sheet: &Sheet, path: &str) -> Result<usize> {
    let format = Format::of(path).ok_or_else(|| anyhow!("{path}: not a table this can write"))?;
    match format {
        Format::Xlsx => write_xlsx(sheet, path)?,
        Format::Json => std::fs::write(path, write_json(sheet))
            .with_context(|| format!("cannot write {path}"))?,
        Format::Csv | Format::Tsv => {
            let sep = if format == Format::Tsv { '\t' } else { ',' };
            std::fs::write(path, write_delimited(sheet, sep))
                .with_context(|| format!("cannot write {path}"))?
        }
    }
    Ok(sheet.rows.len())
}

// ------------------------------------------------------------------- xlsx ---

fn read_xlsx(path: &str, name: String) -> Result<Sheet> {
    use calamine::Reader;

    let mut book = calamine::open_workbook_auto(path)
        .with_context(|| format!("cannot open {path} as a spreadsheet"))?;
    // The first sheet, because the form asks for a file and not a tab. A
    // workbook of twelve sheets converts its first one, and the preview says
    // which one that is rather than leaving it a surprise.
    let first = book
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("{path} has no sheets"))?;
    let range = book
        .worksheet_range(&first)
        .with_context(|| format!("cannot read sheet {first}"))?;

    let rows = range
        .rows()
        .map(|row| row.iter().map(|cell| cell.to_string()).collect())
        .collect();
    Ok(Sheet {
        name: if first.is_empty() { name } else { first },
        rows,
    })
}

fn write_xlsx(sheet: &Sheet, path: &str) -> Result<()> {
    let mut book = rust_xlsxwriter::Workbook::new();
    let page = book.add_worksheet();
    // Excel refuses a sheet name over 31 characters or holding []:*?/\ — a
    // file stem hits both often enough to be worth trimming rather than
    // failing on.
    page.set_name(&safe_sheet_name(&sheet.name)).ok();
    for (r, row) in sheet.rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            page.write_string(r as u32, c as u16, cell)?;
        }
    }
    book.save(path)
        .with_context(|| format!("cannot write {path}"))?;
    Ok(())
}

fn safe_sheet_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if "[]:*?/\\".contains(c) { '-' } else { c })
        .take(31)
        .collect();
    if cleaned.trim().is_empty() {
        "Sheet1".into()
    } else {
        cleaned
    }
}

// ------------------------------------------------------------------- json ---

/// An array of objects, keyed by the header row — which is what every JSON API
/// and every `jq` pipeline produces, and what a spreadsheet round-trips into.
fn read_json(body: &str, name: String) -> Result<Sheet> {
    let value: serde_json::Value = serde_json::from_str(body).context("that is not valid JSON")?;
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("expected an array of rows at the top level"))?;

    // Union of the keys, in first-seen order: a row missing a field writes an
    // empty cell rather than shifting every column after it.
    let mut headers: Vec<String> = Vec::new();
    for row in array {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if !headers.iter().any(|h| h == key) {
                    headers.push(key.clone());
                }
            }
        }
    }

    let mut rows = vec![headers.clone()];
    for row in array {
        let cells = match row.as_object() {
            Some(obj) => headers
                .iter()
                .map(|h| obj.get(h).map(json_cell).unwrap_or_default())
                .collect(),
            // A bare value in the array is a one-column row.
            None => vec![json_cell(row)],
        };
        rows.push(cells);
    }
    Ok(Sheet { name, rows })
}

fn json_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn write_json(sheet: &Sheet) -> String {
    let mut iter = sheet.rows.iter();
    let Some(headers) = iter.next() else {
        return "[]\n".into();
    };
    let out: Vec<serde_json::Value> = iter
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, header) in headers.iter().enumerate() {
                let cell = row.get(i).cloned().unwrap_or_default();
                obj.insert(header.clone(), serde_json::Value::String(cell));
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into())
}

// -------------------------------------------------------------- delimited ---

/// RFC 4180. Quotes toggle, a doubled quote inside a quoted field is a literal
/// quote, and a newline inside quotes belongs to the field.
fn parse_delimited(body: &str, sep: char) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = body.chars().peekable();

    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            c if c == sep => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    // A file that does not end in a newline still has a last row.
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn write_delimited(sheet: &Sheet, sep: char) -> String {
    let mut out = String::new();
    for row in &sheet.rows {
        let line: Vec<String> = row.iter().map(|c| quote(c, sep)).collect();
        out.push_str(&line.join(&sep.to_string()));
        out.push('\n');
    }
    out
}

fn quote(v: &str, sep: char) -> String {
    if v.contains(sep) || v.contains('"') || v.contains('\n') || v.contains('\r') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_comma_stays_in_its_field() {
        let rows = parse_delimited("name,note\nAda,\"born 1815, London\"\n", ',');
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["Ada", "born 1815, London"]);
    }

    #[test]
    fn doubled_quotes_and_embedded_newlines_survive() {
        let rows = parse_delimited("a\n\"say \"\"hi\"\"\nagain\"\n", ',');
        assert_eq!(rows[1][0], "say \"hi\"\nagain");
    }

    #[test]
    fn a_file_without_a_trailing_newline_keeps_its_last_row() {
        let rows = parse_delimited("a,b\n1,2", ',');
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["1", "2"]);
    }

    #[test]
    fn delimited_round_trips_through_the_writer() {
        let sheet = Sheet {
            name: "t".into(),
            rows: vec![
                vec!["name".into(), "note".into()],
                vec!["Ada".into(), "born 1815, London".into()],
            ],
        };
        let text = write_delimited(&sheet, ',');
        assert!(text.contains("\"born 1815, London\""));
        assert_eq!(parse_delimited(&text, ','), sheet.rows);
    }

    #[test]
    fn json_rows_become_a_header_and_a_grid() {
        let sheet = read_json(
            r#"[{"name":"Ada","year":1815},{"name":"Grace","role":"admiral"}]"#,
            "t".into(),
        )
        .unwrap();
        // The union of the keys, first-seen order, and a gap where a row was
        // missing one.
        assert_eq!(sheet.rows[0], vec!["name", "year", "role"]);
        assert_eq!(sheet.rows[1], vec!["Ada", "1815", ""]);
        assert_eq!(sheet.rows[2], vec!["Grace", "", "admiral"]);
    }

    #[test]
    fn a_grid_becomes_json_objects_keyed_by_the_header() {
        let sheet = Sheet {
            name: "t".into(),
            rows: vec![
                vec!["name".into(), "role".into()],
                vec!["Grace".into(), "admiral".into()],
            ],
        };
        let body = write_json(&sheet);
        assert!(body.contains("\"name\": \"Grace\""));
        assert!(body.contains("\"role\": \"admiral\""));
        // And back again, unchanged.
        assert_eq!(read_json(&body, "t".into()).unwrap().rows, sheet.rows);
    }

    #[test]
    fn a_sheet_name_excel_would_refuse_is_trimmed_rather_than_fatal() {
        assert_eq!(safe_sheet_name("sales/2026"), "sales-2026");
        assert_eq!(safe_sheet_name("   "), "Sheet1");
        assert_eq!(safe_sheet_name(&"x".repeat(40)).len(), 31);
    }

    #[test]
    fn formats_come_from_the_name() {
        assert_eq!(Format::of("/a/b.CSV"), Some(Format::Csv));
        assert_eq!(Format::of("/a/b.xlsx"), Some(Format::Xlsx));
        assert_eq!(Format::of("/a/b.mp4"), None);
    }
}
