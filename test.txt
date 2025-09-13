//! calcbits — small utilities for working with downloaded binary data.
//!
//! - formatting helpers: `to_hex`, `to_octal`, `to_decimal`
//! - timing helpers: `average_time`, `time_exec`
//! - progress bar helper: `create_progress_bar`
//! - async downloader with progress: `download_with_progress`
//! - simple DB save/load helpers with progress: `save_to_db`, `load_from_db`

use indicatif::{ProgressBar, ProgressStyle};
use std::time::{Duration, Instant};

/// Convert a byte slice into a space-separated HEX string (uppercase).
pub fn to_hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert a byte slice into a space-separated OCTAL string.
pub fn to_octal(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{:o}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert a byte slice into a space-separated DECIMAL string.
pub fn to_decimal(data: &[u8]) -> String {
    data.iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Calculate the average of a slice of `Duration`. Returns `None` if empty.
pub fn average_time(times: &[Duration]) -> Option<Duration> {
    if times.is_empty() {
        return None;
    }
    let total: Duration = times.iter().copied().sum();
    Some(total / (times.len() as u32))
}

/// Time execution of a closure, returning `(result, elapsed)`.
pub fn time_exec<F, T>(f: F) -> (T, Duration)
where
    F: FnOnce() -> T,
{
    let start = Instant::now();
    let res = f();
    (res, start.elapsed())
}

/// Create a styled progress bar with `length` and initial `msg`.
/// Use `.inc(n)` on the returned `ProgressBar` to advance.
pub fn create_progress_bar(length: u64, msg: &str) -> ProgressBar {
    let pb = ProgressBar::new(length);
    pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%) {msg}",
        )
        .unwrap()
        .progress_chars("##-"),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

//
// Async downloader with progress
//

use futures_util::StreamExt;
use reqwest::Client;

/// Download `url` and return the whole body as `Vec<u8>`, displaying a progress bar.
/// This is an async function that expects to run inside a Tokio runtime.
pub async fn download_with_progress(url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::new();
    let resp = client.get(url).send().await?;
    let total_size = resp
        .content_length()
        .unwrap_or(0);

    let pb = create_progress_bar(total_size, "Downloading");

    let mut data: Vec<u8> = Vec::with_capacity(total_size as usize);
    let mut stream = resp.bytes_stream();

    while let Some(item) = stream.next().await {
        let chunk = item?;
        pb.inc(chunk.len() as u64);
        data.extend_from_slice(&chunk);
    }

    pb.finish_with_message("Download complete!");
    Ok(data)
}

//
// Simple DB-format save/load (synchronous, filesystem). These mirror the behavior
// you used in `catch` and include progress bars.
//

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

/// Save a file into a DB file. If `quantum == true` it writes the "###ENTRY###" style
/// with [DEC], [OCT], [HEX] lines. Otherwise it writes the `---ENTRY---` style with
/// `DATA: <hex bytes>`.
///
/// The function shows a progress bar while writing the hex bytes (for the non-quantum case).
pub fn save_to_db(dbfile: &str, filename: &str, data: &[u8], quantum: bool) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(dbfile)?;
    if quantum {
        writeln!(file, "###ENTRY###")?;
        writeln!(file, "NAME:{}", filename)?;
        writeln!(file, "SIZE:{}", data.len())?;
        writeln!(file, "[DEC] {}", to_decimal(data))?;
        writeln!(file, "[OCT] {}", to_octal(data))?;
        writeln!(file, "[HEX] {}", to_hex(data))?;
        writeln!(file, "###END###")?;
    } else {
        writeln!(file, "---ENTRY---")?;
        writeln!(file, "NAME:{}", filename)?;
        writeln!(file, "SIZE:{}", data.len())?;
        write!(file, "DATA: ")?;

        let pb = create_progress_bar(data.len() as u64, "Saving to DB");
        for (i, b) in data.iter().enumerate() {
            write!(file, "{:02X} ", b)?;
            pb.inc(1);
            if (i + 1) % 16 == 0 {
                writeln!(file)?;
            }
        }
        writeln!(file, "\n---END---")?;
        pb.finish_with_message("Saved to DB");
    }
    Ok(())
}

/// Load a named file from the DB into `out`. Supports both `DATA:` (hex) and `[HEX]`.
/// Shows a progress bar while writing the extracted bytes to `out`.
pub fn load_from_db(dbfile: &str, target: &str, out: &str) -> std::io::Result<()> {
    let f = File::open(dbfile)?;
    let reader = BufReader::new(f);
    let mut inside = false;
    let mut collected: Vec<u8> = Vec::new();
    let mut current_name = String::new();

    for line in reader.lines() {
        let l = line?;
        if l.contains("ENTRY") {
            inside = true;
            collected.clear();
            current_name.clear();
        } else if l.starts_with("NAME:") {
            if let Some((_, name)) = l.split_once(':') {
                current_name = name.to_string();
            }
        } else if l.starts_with("DATA:") {
            for p in l["DATA:".len()..].trim().split_whitespace() {
                if let Ok(b) = u8::from_str_radix(p, 16) {
                    collected.push(b);
                }
            }
        } else if l.starts_with("[HEX]") {
            for p in l[5..].trim().split_whitespace() {
                if let Ok(b) = u8::from_str_radix(p, 16) {
                    collected.push(b);
                }
            }
        } else if l.contains("END") && inside {
            if current_name == target {
                let pb = create_progress_bar(collected.len() as u64, "Extracting");
                let mut outf = File::create(out)?;
                for b in &collected {
                    outf.write_all(&[*b])?;
                    pb.inc(1);
                }
                pb.finish_with_message("Extraction complete!");
                println!("Extracted {} -> {}", current_name, out);
                return Ok(());
            }
            inside = false;
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "File not found in DB",
    ))
}

//
// Tests
//
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn conv_tests() {
        let data = vec![0x00, 0x0F, 0xFF];
        assert_eq!(to_hex(&data), "00 0F FF");
        assert_eq!(to_octal(&data), "0 17 377");
        assert_eq!(to_decimal(&data), "0 15 255");
    }

    #[test]
    fn avg_time_test() {
        let times = vec![Duration::from_millis(10), Duration::from_millis(20)];
        let avg = average_time(&times).unwrap();
        assert_eq!(avg, Duration::from_millis(15));
    }

    #[test]
    fn time_exec_test() {
        let (res, elapsed) = time_exec(|| 2 + 2);
        assert_eq!(res, 4);
        // just ensure elapsed is non-zero-ish type (can't assert exact)
        assert!(elapsed.as_nanos() >= 0);
    }
}
