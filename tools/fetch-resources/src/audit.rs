use anyhow::Result;
use std::path::Path;

#[derive(Debug)]
pub struct AuditResult {
    pub os: &'static str,
    pub total_bytes: u64,
    pub per_binary: Vec<(String, u64)>,
    pub budget_bytes: u64,
    pub over_budget: bool,
}

pub fn budget_for(os: &str) -> u64 {
    match os {
        "macos" | "windows" => 350 * 1024 * 1024, // 350 MB
        "linux" => 400 * 1024 * 1024,             // 400 MB AppImage
        _ => 400 * 1024 * 1024,
    }
}

pub fn audit_dir(dir: &Path, os: &'static str) -> Result<AuditResult> {
    let budget = budget_for(os);
    let mut per_binary = Vec::new();
    let mut total = 0u64;
    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_file() {
                let sz = meta.len();
                total += sz;
                per_binary.push((entry.file_name().to_string_lossy().into_owned(), sz));
            }
        }
    }
    per_binary.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(AuditResult {
        os,
        total_bytes: total,
        per_binary,
        budget_bytes: budget,
        over_budget: total > budget,
    })
}

pub fn print_report(r: &AuditResult) {
    println!("== installer size audit ({}) ==", r.os);
    println!("budget: {} MB", r.budget_bytes / 1024 / 1024);
    println!("total:  {} MB", r.total_bytes / 1024 / 1024);
    for (name, sz) in &r.per_binary {
        println!("  {:>8} KB  {}", sz / 1024, name);
    }
    println!("{}", if r.over_budget { "STATUS: OVER BUDGET" } else { "STATUS: ok" });
}
