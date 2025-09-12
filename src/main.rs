use reqwest;
use socket2::{Domain, Protocol, Socket, Type};
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::io;

// ---------- Argument Parsing ----------
fn parse_args() -> Vec<String> {
    env::args().skip(1).collect()
}

// ---------- ICMP Checksum ----------
fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut chunks = data.chunks_exact(2);

    for chunk in &mut chunks {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        sum = sum.wrapping_add(word);
    }

    if let Some(&byte) = chunks.remainder().first() {
        sum = sum.wrapping_add((byte as u32) << 8);
    }

    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    !(sum as u16)
}

// ---------- ICMP Packet Builder ----------
fn build_icmp_packet(id: u16, seq: u16) -> Vec<u8> {
    let mut packet = vec![0u8; 8];
    packet[0] = 8; // Echo Request
    packet[1] = 0;
    packet[4..6].copy_from_slice(&id.to_be_bytes());
    packet[6..8].copy_from_slice(&seq.to_be_bytes());

    let csum = checksum(&packet);
    packet[2..4].copy_from_slice(&csum.to_be_bytes());
    packet
}

// ---------- Pinger with Stats ----------
fn ping(host: &str, count: u16) -> io::Result<()> {
    let addr: Ipv4Addr = host.parse().expect("Invalid IP address");
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;

    let sockaddr = SocketAddr::new(addr.into(), 0);
    let mut received = 0;
    let mut times: Vec<Duration> = Vec::new();

    for seq in 0..count {
        let packet = build_icmp_packet(1, seq);
        let start = Instant::now();
        socket.send_to(&packet, &sockaddr.into())?;

        use std::mem::MaybeUninit;
        let mut buf = [MaybeUninit::<u8>::uninit(); 1024];
        match socket.recv(&mut buf) {
            Ok(n) => {
                // SAFETY: We trust recv to have initialized the first n bytes.
                let _bytes: &[u8] =
                    unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, n) };
                let elapsed = start.elapsed();
                received += 1;
                times.push(elapsed);
                println!("Reply from {}: seq={} time={:?}", addr, seq, elapsed);
            }
            Err(_) => {
                println!("Request timeout for seq={}", seq);
            }
        }
    }

    println!("\nPing statistics for {}:", addr);
    println!(
        "    Packets: Sent = {}, Received = {}, Lost = {} ({}% loss)",
        count,
        received,
        count - received,
        ((count - received) as f64 / count as f64 * 100.0) as u32
    );

    if !times.is_empty() {
        let min = times.iter().min().unwrap();
        let max = times.iter().max().unwrap();
        let avg = times.iter().sum::<Duration>() / times.len() as u32;
        println!("Approximate round trip times in milli-seconds:");
        println!(
            "    Minimum = {:?}, Maximum = {:?}, Average = {:?}",
            min, max, avg
        );
    }

    Ok(())
}

// ---------- Database Save ----------
fn save_to_db(dbfile: &str, filename: &str, data: &[u8], quantum: bool) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(dbfile)?;
    if quantum {
        writeln!(file, "###ENTRY###")?;
        writeln!(file, "NAME:{}", filename)?;
        writeln!(file, "SIZE:{}", data.len())?;
        writeln!(
            file,
            "[DEC] {}",
            data.iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        )?;
        writeln!(
            file,
            "[OCT] {}",
            data.iter()
                .map(|b| format!("{:o}", b))
                .collect::<Vec<_>>()
                .join(" ")
        )?;
        writeln!(
            file,
            "[HEX] {}",
            data.iter()
                .map(|b| format!("{:02X}", b))
                .collect::<Vec<_>>()
                .join(" ")
        )?;
        writeln!(file, "###END###")?;
    } else {
        writeln!(file, "---ENTRY---")?;
        writeln!(file, "NAME:{}", filename)?;
        writeln!(file, "SIZE:{}", data.len())?;
        write!(file, "DATA: ")?;
        for (i, b) in data.iter().enumerate() {
            write!(file, "{:02X} ", b)?;
            if (i + 1) % 16 == 0 {
                writeln!(file)?;
            }
        }
        writeln!(file, "\n---END---")?;
    }
    Ok(())
}

// ---------- Database Load ----------
fn load_from_db(dbfile: &str, target: &str, out: &str) -> std::io::Result<()> {
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
            current_name = l.split_once(':').unwrap().1.to_string();
        } else if l.starts_with("DATA:") {
            let parts = l["DATA:".len()..].trim().split_whitespace();
            for p in parts {
                if let Ok(b) = u8::from_str_radix(p, 16) {
                    collected.push(b);
                }
            }
        } else if l.starts_with("[HEX]") {
            let parts = l[5..].trim().split_whitespace();
            for p in parts {
                if let Ok(b) = u8::from_str_radix(p, 16) {
                    collected.push(b);
                }
            }
        } else if l.contains("END") && inside {
            if current_name == target {
                let mut outf = File::create(out)?;
                outf.write_all(&collected)?;
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

// ---------- Main ----------
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();

    if args.is_empty() {
        println!("Catch | made by Ariel Zvinowanda in 5B");
        println!("Usage:");
        println!("  catch /u <url> /o <file> [/s <dbfile.dlb|.dqb>]");
        println!("  catch /p:<count> <host>");
        println!("  catch /l <dbfile> /t <filename> /o <outfile>");
        return Ok(());
    }

    let mut url: Option<String> = None;
    let mut out: Option<String> = None;
    let mut ping_count: Option<u16> = None;
    let mut ping_host: Option<String> = None;
    let mut save_db: Option<String> = None;
    let mut load_db: Option<String> = None;
    let mut take_file: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            a if a.starts_with("/u") => {
                url = Some(args[i + 1].clone());
                i += 1;
            }
            a if a.starts_with("/o") => {
                out = Some(args[i + 1].clone());
                i += 1;
            }
            a if a.starts_with("/s") => {
                save_db = Some(args[i + 1].clone());
                i += 1;
            }
            a if a.starts_with("/l") => {
                load_db = Some(args[i + 1].clone());
                i += 1;
            }
            a if a.starts_with("/t") => {
                take_file = Some(args[i + 1].clone());
                i += 1;
            }
            a if a.starts_with("/p:") => {
                let parts: Vec<&str> = a.split(':').collect();
                ping_count = Some(parts[1].parse().unwrap_or(4));
                if i + 1 < args.len() {
                    ping_host = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // --- Downloader + save to DB ---
    if let Some(u) = url {
        let outfile = out.clone().unwrap_or("output.html".into());
        println!("Downloading {} -> {}", u, outfile);
        let resp = reqwest::get(&u).await?.bytes().await?;
        let data = resp.to_vec();
        let mut file = File::create(&outfile)?;
        file.write_all(&data)?;
        println!("Download complete.");

        if let Some(db) = save_db {
            let quantum = db.ends_with(".dqb");
            save_to_db(&db, &outfile, &data, quantum)?;
            println!("Stored {} into {}", outfile, db);
        }
    }

    // --- Load from DB ---
    if let (Some(db), Some(t), Some(o)) = (load_db, take_file, out) {
        load_from_db(&db, &t, &o)?;
    }

    // --- Ping ---
    if let (Some(c), Some(h)) = (ping_count, ping_host) {
        ping(&h, c).unwrap();
    }

    Ok(())
}

