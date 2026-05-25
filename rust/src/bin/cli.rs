// Console driver — invokes the scanner modules from the lib crate. Useful for
// validating the scanners independently of the GUI.

use cluttercutter::{analysis, mft, scanner, temp, types};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut use_mft = false;
    let mut top_n: usize = 0;
    let mut oldest_n: usize = 0;
    let mut temp_mode = false;
    // Strip --mft / --top-n N / --oldest-n N / --temp; remainder is the path
    // (path is unused in --temp mode).
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mft" => {
                use_mft = true;
                args.remove(i);
            }
            "--top-n" => {
                args.remove(i);
                if i < args.len() {
                    top_n = args.remove(i).parse().unwrap_or(0);
                }
            }
            "--oldest-n" => {
                args.remove(i);
                if i < args.len() {
                    oldest_n = args.remove(i).parse().unwrap_or(0);
                }
            }
            "--temp" => {
                temp_mode = true;
                args.remove(i);
            }
            _ => i += 1,
        }
    }

    if temp_mode {
        run_temp_scan();
        return;
    }

    let track_files = top_n > 0 || oldest_n > 0;
    let root = match args.into_iter().next() {
        Some(p) => p,
        None => {
            eprintln!(
                "usage: cluttercutter-cli.exe [--mft] [--top-n N] [--oldest-n N] <path>\n       cluttercutter-cli.exe --temp"
            );
            std::process::exit(2);
        }
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let progress: scanner::ProgressFn = Box::new(|p| {
        eprint!(
            "\r  {:>8} files  {:>10}  {:>5}  {}",
            p.files_scanned,
            fmt_bytes(p.total_size),
            if p.percent < 0.0 {
                "  ?  ".to_string()
            } else {
                format!("{:>4.1}%", p.percent)
            },
            truncate(&p.current_path, 70),
        );
    });

    let start = Instant::now();
    let result = if use_mft {
        mft::MftScanner::new()
            .with_cancel(cancel.clone())
            .with_progress(progress)
            .with_track_files(track_files)
            .scan(&root)
    } else {
        scanner::Scanner::new()
            .with_cancel(cancel.clone())
            .with_progress(progress)
            .with_track_files(track_files)
            .scan(&root)
            .map_err(|s| s.to_string())
    };
    let elapsed = start.elapsed();
    eprintln!();

    let root = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Scan failed: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "\n{} — {} ({} files, {} folders) in {:.2}s [{}]",
        root.name,
        fmt_bytes(root.size),
        root.file_count,
        root.folder_count,
        elapsed.as_secs_f64(),
        if use_mft { "MFT" } else { "walker" },
    );

    let printed_special = top_n > 0 || oldest_n > 0;
    if top_n > 0 {
        println!("\nTop {top_n} largest files:");
        let hits = analysis::top_n_files(&root, top_n);
        for h in &hits {
            let full = full_path(h);
            println!("  {:>10}  {}", fmt_bytes(h.file.size), full);
        }
    }
    if oldest_n > 0 {
        println!("\nOldest {oldest_n} files (by last-modified):");
        let hits = analysis::oldest_n_files(&root, oldest_n);
        for h in &hits {
            let full = full_path(h);
            let date = format_short_date(h.file.last_modified_ft);
            println!("  {date}  {:>10}  {full}", fmt_bytes(h.file.size));
        }
    }
    if !printed_special {
        let mut kids: Vec<&types::FolderNode> = root.children.iter().collect();
        kids.sort_by(|a, b| b.size.cmp(&a.size));
        for k in kids.iter().take(20) {
            println!("  {:>10}  {}", fmt_bytes(k.size), k.name);
        }
    }
}

fn run_temp_scan() {
    let locations = temp::discover_locations();
    if locations.is_empty() {
        eprintln!("No temp locations discovered.");
        return;
    }
    eprintln!("Discovered {} locations:", locations.len());
    for (src, p) in &locations {
        eprintln!("  [{}] {}", src.label(), p);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let entries = temp::scan_locations(&locations, cancel);
    let elapsed = start.elapsed();

    let total: i64 = entries.iter().map(|e| e.size).sum();
    println!(
        "\n{} files — {} reclaimable across {} locations in {:.2}s",
        entries.len(),
        fmt_bytes(total),
        locations.len(),
        elapsed.as_secs_f64(),
    );
    println!("\nTop 30 by size:");
    for e in entries.iter().take(30) {
        let date = format_short_date(e.last_modified_ft);
        println!(
            "  {date}  {:>10}  [{}]  {}",
            fmt_bytes(e.size),
            e.source.label(),
            e.full_path,
        );
    }
}

fn full_path(h: &analysis::FileHit<'_>) -> String {
    if h.folder.full_path.ends_with('\\') {
        format!("{}{}", h.folder.full_path, h.file.name)
    } else {
        format!("{}\\{}", h.folder.full_path, h.file.name)
    }
}

// Quick yyyy-mm-dd formatter via Win32 FILETIME→SYSTEMTIME (UTC, no TZ shift —
// good enough for a CLI debug view).
fn format_short_date(raw: i64) -> String {
    if raw == 0 {
        return "          ".into();
    }
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::FileTimeToSystemTime;
    let ft = FILETIME {
        dwLowDateTime: raw as u32,
        dwHighDateTime: (raw >> 32) as u32,
    };
    let mut st = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut st).is_err() {
            return "          ".into();
        }
    }
    format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
}

fn fmt_bytes(n: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let trimmed: String = s.chars().take(max - 1).collect();
    format!("{trimmed}…")
}
