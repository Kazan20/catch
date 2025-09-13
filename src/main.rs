use std::env;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::io;
use std::fs::File;
use std::io::Write;
use calcbits::{download_with_progress, save_to_db, load_from_db, create_progress_bar};

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

// ---------- Pinger ----------
fn ping(host: &str, count: u16) -> io::Result<()> {
    let addr: Ipv4Addr = host.parse().expect("Invalid IP address");
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;

    let sockaddr = SocketAddr::new(addr.into(), 0);
    let mut received = 0;
    let mut times: Vec<Duration> = Vec::new();

    // Use progress bar from calcbits
    let pb = create_progress_bar(count as u64, "Pinging");

    for seq in 0..count {
        let packet = build_icmp_packet(1, seq);
        let start = Instant::now();
        socket.send_to(&packet, &sockaddr.into())?;

        use std::mem::MaybeUninit;
        let mut buf = [MaybeUninit::<u8>::uninit(); 1024];
        match socket.recv(&mut buf) {
            Ok(n) => {
                let _bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, n) };
                let elapsed = start.elapsed();
                received += 1;
                times.push(elapsed);
                println!("Reply from {}: seq={} time={:?}", addr, seq, elapsed);
            }
            Err(_) => println!("Request timeout for seq={}", seq),
        }

        pb.inc(1);
    }

    pb.finish_with_message("Ping complete");

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
        println!(
            "Approximate round trip times in milli-seconds:\n    Minimum = {:?}, Maximum = {:?}, Average = {:?}",
            min, max, avg
        );
    }

    Ok(())
}

// ---------- Main ----------
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
            a if a.starts_with("/u") => { url = Some(args[i + 1].clone()); i += 1; }
            a if a.starts_with("/o") => { out = Some(args[i + 1].clone()); i += 1; }
            a if a.starts_with("/s") => { save_db = Some(args[i + 1].clone()); i += 1; }
            a if a.starts_with("/l") => { load_db = Some(args[i + 1].clone()); i += 1; }
            a if a.starts_with("/t") => { take_file = Some(args[i + 1].clone()); i += 1; }
            a if a.starts_with("/p:") => {
                let parts: Vec<&str> = a.split(':').collect();
                ping_count = Some(parts[1].parse().unwrap_or(4));
                if i + 1 < args.len() { ping_host = Some(args[i + 1].clone()); i += 1; }
            }
            _ => {}
        }
        i += 1;
    }

    // --- Downloader + save to DB using calcbits progress bar ---
    if let Some(u) = url {
        let outfile = out.clone().unwrap_or("output.html".into());
        println!("Downloading {} -> {}", u, outfile);

        let data = download_with_progress(&u).await?;

        // Write to file with calcbits progress bar
        let pb = create_progress_bar(data.len() as u64, "Writing file");
        let mut f = File::create(&outfile)?;
        for chunk in data.chunks(4096) {
            f.write_all(chunk)?;
            pb.inc(chunk.len() as u64);
        }
        pb.finish_with_message("File saved");

        if let Some(db) = save_db {
            let quantum = db.ends_with(".dqb");
            save_to_db(&db, &outfile, &data, quantum)?;
            println!("Stored {} into {}", outfile, db);
        }
    }

    // --- Load from DB using calcbits progress bar ---
    if let (Some(db), Some(t), Some(o)) = (load_db, take_file, out) {
        load_from_db(&db, &t, &o)?;
    }

    // --- Ping with calcbits progress bar ---
    if let (Some(c), Some(h)) = (ping_count, ping_host) {
        ping(&h, c)?;
    }

    Ok(())
}
