//! resaiz Font Helper
//!
//! A tiny local HTTP server that lists the fonts installed on this computer
//! (system, user and Adobe Fonts activations) and serves their files to the
//! resaiz editor on 127.0.0.1. No dependencies: the font name tables are
//! parsed here and the HTTP layer is plain std::net.
//!
//! Endpoints (GET, JSON unless noted):
//!   /v1/health        {"name","version","fonts","platform","port"}
//!   /v1/fonts         [{"id","family","style","weight","italic","postscript","source","format","size","axes"}]
//!   /v1/font/{id}     the font bytes (font/ttf or font/otf; a collection face is repacked on its own)
//!
//! Only whitelisted browser origins may read it (see ALLOWED_ORIGINS). File
//! paths never leave the process; fonts are addressed by id.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PORTS: [u16; 10] = [57731, 57732, 57733, 57734, 57735, 57736, 57737, 57738, 57739, 57740];
const RESCAN_SECS: u64 = 60;
const MAX_DEPTH: usize = 6;

#[derive(Clone, Debug)]
struct FontEntry {
    id: String,
    family: String,
    style: String,
    weight: u16,
    italic: bool,
    postscript: String,
    source: &'static str,
    format: &'static str,
    path: PathBuf,
    face_index: u32,
    /// Bytes the browser receives: the file, or the single face repacked out of a collection.
    size: u64,
    /// Variation axes of a variable font (fvar): tag, min, default, max. Empty for static faces.
    axes: Vec<Axis>,
}

#[derive(Clone, Debug)]
struct Axis {
    tag: String,
    min: f32,
    def: f32,
    max: f32,
}

#[derive(Default)]
struct Catalog {
    fonts: Vec<FontEntry>,
    by_id: HashMap<String, usize>,
    scanned_at: Option<Instant>,
    scan_ms: u128,
    signature: u64,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let once = args.iter().any(|a| a == "--once");
    let port_arg = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<u16>().ok());

    let roots = font_roots();
    let t0 = Instant::now();
    let fonts = scan(&roots);
    let scan_ms = t0.elapsed().as_millis();
    eprintln!(
        "[resaiz-font-helper] v{} scanned {} faces in {} ms from {} roots",
        VERSION,
        fonts.len(),
        scan_ms,
        roots.len()
    );

    if once {
        println!("{}", fonts_json(&fonts));
        return;
    }

    let catalog = Arc::new(RwLock::new(Catalog {
        by_id: index(&fonts),
        signature: roots_signature(&roots),
        fonts,
        scanned_at: Some(Instant::now()),
        scan_ms,
    }));

    // Background rescan: cheap directory mtime signature every RESCAN_SECS.
    {
        let catalog = Arc::clone(&catalog);
        let roots = roots.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(RESCAN_SECS));
            let sig = roots_signature(&roots);
            let stale = catalog.read().map(|c| c.signature != sig).unwrap_or(false);
            if stale {
                let t0 = Instant::now();
                let fonts = scan(&roots);
                if let Ok(mut c) = catalog.write() {
                    c.by_id = index(&fonts);
                    c.fonts = fonts;
                    c.signature = sig;
                    c.scanned_at = Some(Instant::now());
                    c.scan_ms = t0.elapsed().as_millis();
                }
                eprintln!("[resaiz-font-helper] rescanned after a font directory changed");
            }
        });
    }

    let (listener, port) = bind(port_arg);
    eprintln!("[resaiz-font-helper] listening on http://127.0.0.1:{port}");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let catalog = Arc::clone(&catalog);
                thread::spawn(move || handle(s, catalog, port));
            }
            Err(e) => eprintln!("[resaiz-font-helper] accept error: {e}"),
        }
    }
}

fn bind(preferred: Option<u16>) -> (TcpListener, u16) {
    let candidates: Vec<u16> = match preferred {
        Some(p) => vec![p],
        None => PORTS.to_vec(),
    };
    for p in candidates {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)) {
            return (l, p);
        }
    }
    eprintln!("[resaiz-font-helper] no free port in {:?}", PORTS);
    std::process::exit(2);
}

/* ------------------------------------------------------------------------ */
/* Font directories                                                          */
/* ------------------------------------------------------------------------ */

fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// (directory, source label)
fn font_roots() -> Vec<(PathBuf, &'static str)> {
    let mut out: Vec<(PathBuf, &'static str)> = Vec::new();
    let h = home();
    if cfg!(target_os = "macos") {
        out.push((PathBuf::from("/System/Library/Fonts"), "system"));
        out.push((PathBuf::from("/Library/Fonts"), "system"));
        if let Some(h) = &h {
            out.push((h.join("Library/Fonts"), "user"));
            out.push((
                h.join("Library/Application Support/Adobe/CoreSync/plugins/livetype/.r"),
                "adobe",
            ));
        }
    } else if cfg!(target_os = "windows") {
        let windir = env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        out.push((windir.join("Fonts"), "system"));
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            out.push((PathBuf::from(local).join("Microsoft\\Windows\\Fonts"), "user"));
        }
        if let Some(roaming) = env::var_os("APPDATA") {
            out.push((PathBuf::from(roaming).join("Adobe\\CoreSync\\plugins\\livetype\\r"), "adobe"));
        }
    } else {
        out.push((PathBuf::from("/usr/share/fonts"), "system"));
        out.push((PathBuf::from("/usr/local/share/fonts"), "system"));
        if let Some(h) = &h {
            out.push((h.join(".fonts"), "user"));
            out.push((h.join(".local/share/fonts"), "user"));
            out.push((h.join(".config/Adobe/CoreSync/plugins/livetype/.r"), "adobe"));
        }
    }
    out.into_iter().filter(|(p, _)| p.is_dir()).collect()
}

/// Sum of directory mtimes (recursive) so a changed font folder is noticed.
fn roots_signature(roots: &[(PathBuf, &'static str)]) -> u64 {
    let mut sig: u64 = 1469598103934665603;
    for (root, _) in roots {
        walk_dirs(root, 0, &mut |dir| {
            if let Ok(m) = fs::metadata(dir) {
                if let Ok(t) = m.modified() {
                    if let Ok(d) = t.duration_since(SystemTime::UNIX_EPOCH) {
                        sig ^= d.as_secs();
                        sig = sig.wrapping_mul(1099511628211);
                    }
                }
            }
        });
    }
    sig
}

fn walk_dirs(dir: &Path, depth: usize, f: &mut dyn FnMut(&Path)) {
    f(dir);
    if depth >= MAX_DEPTH {
        return;
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_dirs(&p, depth + 1, f);
            }
        }
    }
}

fn walk_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_files(&p, depth + 1, out);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
}

/* ------------------------------------------------------------------------ */
/* Scanning and sfnt parsing                                                 */
/* ------------------------------------------------------------------------ */

fn scan(roots: &[(PathBuf, &'static str)]) -> Vec<FontEntry> {
    let mut fonts: Vec<FontEntry> = Vec::new();
    for (root, source) in roots {
        let mut files = Vec::new();
        walk_files(root, 0, &mut files);
        for path in files {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            // Adobe activations may carry odd names; sniff the magic instead.
            let candidate = matches!(ext.as_str(), "ttf" | "otf" | "ttc") || *source == "adobe" || ext.is_empty();
            if !candidate {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            if bytes.len() < 12 {
                continue;
            }
            for entry in parse_file(&bytes, &path, source) {
                fonts.push(entry);
            }
        }
    }
    // The same face often exists twice (Adobe ships .otf and .ttf, a user copy
    // beside a system collection). One entry per family, style, weight and
    // italic; the first found wins, so the scan order of the roots decides.
    let mut seen: HashSet<String> = HashSet::new();
    fonts.retain(|f| seen.insert(format!("{}|{}|{}|{}", f.family.to_lowercase(), f.style.to_lowercase(), f.weight, f.italic)));
    fonts.sort_by(|a, b| {
        a.family
            .to_lowercase()
            .cmp(&b.family.to_lowercase())
            .then(a.weight.cmp(&b.weight))
            .then(a.italic.cmp(&b.italic))
    });
    fonts
}

fn index(fonts: &[FontEntry]) -> HashMap<String, usize> {
    fonts.iter().enumerate().map(|(i, f)| (f.id.clone(), i)).collect()
}

fn be16(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn parse_file(bytes: &[u8], path: &Path, source: &'static str) -> Vec<FontEntry> {
    let magic = &bytes[0..4];
    let mut out = Vec::new();
    if magic == b"ttcf" {
        let n = be32(bytes, 8).unwrap_or(0).min(64);
        for i in 0..n {
            if let Some(off) = be32(bytes, 12 + 4 * i as usize) {
                if let Some(e) = parse_face(bytes, off as usize, path, source, i, "ttc") {
                    out.push(e);
                }
            }
        }
        return out;
    }
    let format = match magic {
        [0x00, 0x01, 0x00, 0x00] | b"true" => "ttf",
        b"OTTO" => "otf",
        _ => return out,
    };
    if let Some(e) = parse_face(bytes, 0, path, source, 0, format) {
        out.push(e);
    }
    out
}

fn parse_face(
    bytes: &[u8],
    off: usize,
    path: &Path,
    source: &'static str,
    face_index: u32,
    format: &'static str,
) -> Option<FontEntry> {
    let num_tables = be16(bytes, off + 4)? as usize;
    let mut tables: HashMap<[u8; 4], (usize, usize)> = HashMap::new();
    for i in 0..num_tables.min(64) {
        let rec = off + 12 + i * 16;
        let tag = bytes.get(rec..rec + 4)?;
        let toff = be32(bytes, rec + 8)? as usize;
        let tlen = be32(bytes, rec + 12)? as usize;
        if toff < bytes.len() {
            tables.insert([tag[0], tag[1], tag[2], tag[3]], (toff, tlen.min(bytes.len() - toff)));
        }
    }
    let (noff, nlen) = *tables.get(b"name")?;
    let names = parse_names(&bytes[noff..noff + nlen]);
    let family = names
        .get(&16)
        .or_else(|| names.get(&1))
        .cloned()
        .filter(|s| !s.trim().is_empty())?;
    // macOS ships UI only faces named ".Al Bayan PUA", ".SF NS" and so on. They are not meant for documents.
    if family.trim().starts_with('.') {
        return None;
    }
    let size: u64 = if format == "ttc" {
        12 + 16 * tables.len() as u64 + tables.values().map(|&(_, l)| ((l + 3) & !3) as u64).sum::<u64>()
    } else {
        bytes.len() as u64
    };
    let style = names
        .get(&17)
        .or_else(|| names.get(&2))
        .cloned()
        .unwrap_or_else(|| "Regular".to_string());
    let postscript = names.get(&6).cloned().unwrap_or_default();

    let mut weight: u16 = 400;
    let mut italic = false;
    if let Some(&(o, l)) = tables.get(b"OS/2") {
        if l >= 64 {
            if let Some(w) = be16(bytes, o + 4) {
                if (1..=1000).contains(&w) {
                    weight = w;
                }
            }
            if let Some(fs_sel) = be16(bytes, o + 62) {
                italic = fs_sel & 0x0001 != 0;
                if fs_sel & 0x0020 != 0 && weight == 400 {
                    weight = 700;
                }
            }
        }
    }
    if let Some(&(o, _)) = tables.get(b"head") {
        if let Some(mac_style) = be16(bytes, o + 44) {
            if mac_style & 0x0002 != 0 {
                italic = true;
            }
            if mac_style & 0x0001 != 0 && weight == 400 {
                weight = 700;
            }
        }
    }
    let lower = style.to_lowercase();
    if lower.contains("italic") || lower.contains("oblique") {
        italic = true;
    }

    let axes = tables
        .get(b"fvar")
        .map(|&(o, l)| parse_fvar(&bytes[o..o + l]))
        .unwrap_or_default();
    // A variable font's OS/2 weight is only its default instance; the browser
    // reaches every weight of the wght axis, so the entry reports the default.
    if let Some(w) = axes.iter().find(|a| a.tag == "wght") {
        weight = w.def.round().clamp(1.0, 1000.0) as u16;
    }

    let id = fnv_hex(&format!("{}#{}", path.display(), face_index));
    Some(FontEntry {
        id,
        family: family.trim().to_string(),
        style: style.trim().to_string(),
        weight,
        italic,
        postscript: postscript.trim().to_string(),
        source,
        format,
        path: path.to_path_buf(),
        face_index,
        size,
        axes,
    })
}

/// fvar table: the variation axes of a variable font (16.16 fixed values).
fn parse_fvar(t: &[u8]) -> Vec<Axis> {
    let mut out = Vec::new();
    let (Some(axes_off), Some(count), Some(size)) = (be16(t, 4), be16(t, 8), be16(t, 10)) else {
        return out;
    };
    let fixed = |o: usize| be32(t, o).map(|v| (v as i32) as f32 / 65536.0);
    for i in 0..count.min(16) as usize {
        let r = axes_off as usize + i * size.max(20) as usize;
        let Some(tag) = t.get(r..r + 4) else { break };
        let (Some(min), Some(def), Some(max)) = (fixed(r + 4), fixed(r + 8), fixed(r + 12)) else { break };
        let tag: String = tag.iter().map(|&b| if b.is_ascii_graphic() { b as char } else { '?' }).collect();
        out.push(Axis { tag, min, def, max });
    }
    out
}

/// Repack one face of a TrueType collection as a standalone sfnt. Browsers only
/// read the first face of a collection handed to FontFace, so every face is
/// served on its own. Tables are copied as they are (offsets rewritten).
fn extract_face(bytes: &[u8], off: usize) -> Option<Vec<u8>> {
    let num_tables = be16(bytes, off + 4)? as usize;
    let mut recs: Vec<([u8; 4], u32, usize, usize)> = Vec::new();
    for i in 0..num_tables.min(64) {
        let r = off + 12 + i * 16;
        let tag = bytes.get(r..r + 4)?;
        let csum = be32(bytes, r + 4)?;
        let toff = be32(bytes, r + 8)? as usize;
        let tlen = be32(bytes, r + 12)? as usize;
        if toff >= bytes.len() {
            continue;
        }
        recs.push(([tag[0], tag[1], tag[2], tag[3]], csum, toff, tlen.min(bytes.len() - toff)));
    }
    let n = recs.len();
    if n == 0 {
        return None;
    }
    let mut es: u16 = 0;
    while (1usize << (es + 1)) <= n {
        es += 1;
    }
    let sr: u16 = (1u16 << es) * 16;
    let mut out = Vec::with_capacity(12 + 16 * n + recs.iter().map(|r| r.3 + 3).sum::<usize>());
    out.extend_from_slice(bytes.get(off..off + 4)?);
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&sr.to_be_bytes());
    out.extend_from_slice(&es.to_be_bytes());
    out.extend_from_slice(&((n as u16) * 16 - sr).to_be_bytes());
    let mut dir = Vec::with_capacity(16 * n);
    let mut data = Vec::new();
    let base = 12 + 16 * n;
    for (tag, csum, toff, tlen) in &recs {
        dir.extend_from_slice(tag);
        dir.extend_from_slice(&csum.to_be_bytes());
        dir.extend_from_slice(&((base + data.len()) as u32).to_be_bytes());
        dir.extend_from_slice(&(*tlen as u32).to_be_bytes());
        data.extend_from_slice(&bytes[*toff..*toff + *tlen]);
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    out.extend_from_slice(&dir);
    out.extend_from_slice(&data);
    Some(out)
}

/// name table: prefer Windows English (3, x, 0x409), then any Windows, then Mac Roman.
fn parse_names(t: &[u8]) -> HashMap<u16, String> {
    let mut out: HashMap<u16, (u8, String)> = HashMap::new();
    let count = be16(t, 2).unwrap_or(0) as usize;
    let str_off = be16(t, 4).unwrap_or(0) as usize;
    for i in 0..count.min(512) {
        let r = 6 + i * 12;
        let (Some(plat), Some(enc), Some(lang), Some(nid), Some(len), Some(off)) = (
            be16(t, r),
            be16(t, r + 2),
            be16(t, r + 4),
            be16(t, r + 6),
            be16(t, r + 8),
            be16(t, r + 10),
        ) else {
            break;
        };
        if !matches!(nid, 1 | 2 | 4 | 6 | 16 | 17) {
            continue;
        }
        let start = str_off + off as usize;
        let Some(raw) = t.get(start..start + len as usize) else { continue };
        let (rank, text) = match (plat, enc) {
            (3, 1) | (3, 10) | (0, _) => {
                let units: Vec<u16> = raw.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                let s = String::from_utf16_lossy(&units);
                (if plat == 3 && lang == 0x0409 { 3u8 } else if plat == 3 { 2 } else { 1 }, s)
            }
            (1, 0) => (0u8, raw.iter().map(|&b| b as char).collect::<String>()),
            _ => continue,
        };
        let text = text.trim_matches(char::from(0)).to_string();
        if text.is_empty() {
            continue;
        }
        match out.get(&nid) {
            Some((r, _)) if *r >= rank => {}
            _ => {
                out.insert(nid, (rank, text));
            }
        }
    }
    out.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

fn fnv_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/* ------------------------------------------------------------------------ */
/* HTTP                                                                      */
/* ------------------------------------------------------------------------ */

fn origin_allowed(origin: &str) -> bool {
    origin == "https://resaiz.vercel.app"
        || origin == "https://www.resaiz.com"
        || origin == "https://resaiz.com"
        || origin == "http://localhost:5173"
        || origin == "http://127.0.0.1:5173"
        || (origin.starts_with("https://resaiz-") && origin.ends_with(".vercel.app"))
}

fn handle(mut stream: TcpStream, catalog: Arc<RwLock<Catalog>>, port: u16) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut origin: Option<String> = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let l = line.trim_end();
                if l.is_empty() {
                    break;
                }
                if let Some(v) = l.strip_prefix("Origin:").or_else(|| l.strip_prefix("origin:")) {
                    origin = Some(v.trim().to_string());
                }
            }
            Err(_) => return,
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");

    let mut cors: Vec<String> = Vec::new();
    if let Some(o) = &origin {
        if !origin_allowed(o) {
            respond(&mut stream, 403, "text/plain", b"origin not allowed", &[]);
            return;
        }
        cors.push(format!("Access-Control-Allow-Origin: {o}"));
        cors.push("Vary: Origin".into());
        cors.push("Access-Control-Allow-Methods: GET, OPTIONS".into());
        cors.push("Access-Control-Allow-Headers: *".into());
        cors.push("Access-Control-Allow-Private-Network: true".into());
        cors.push("Access-Control-Max-Age: 86400".into());
    }

    if method == "OPTIONS" {
        respond(&mut stream, 204, "text/plain", b"", &cors);
        return;
    }
    if method != "GET" && method != "HEAD" {
        respond(&mut stream, 405, "text/plain", b"method not allowed", &cors);
        return;
    }

    match path {
        "/v1/health" | "/v1/health/" => {
            let c = catalog.read().unwrap();
            let body = format!(
                "{{\"name\":\"resaiz-font-helper\",\"version\":\"{}\",\"fonts\":{},\"platform\":\"{}\",\"port\":{},\"scanMs\":{}}}",
                VERSION,
                c.fonts.len(),
                env::consts::OS,
                port,
                c.scan_ms
            );
            respond(&mut stream, 200, "application/json", body.as_bytes(), &cors);
        }
        "/v1/fonts" | "/v1/fonts/" => {
            let c = catalog.read().unwrap();
            let body = fonts_json(&c.fonts);
            respond(&mut stream, 200, "application/json", body.as_bytes(), &cors);
        }
        p if p.starts_with("/v1/font/") => {
            let id = p.trim_start_matches("/v1/font/").trim_end_matches('/');
            let entry = {
                let c = catalog.read().unwrap();
                c.by_id.get(id).map(|&i| c.fonts[i].clone())
            };
            match entry {
                Some(e) => match fs::read(&e.path) {
                    Ok(bytes) => {
                        let (bytes, ct) = if e.format == "ttc" {
                            let off = be32(&bytes, 12 + 4 * e.face_index as usize).unwrap_or(0) as usize;
                            let is_cff = bytes.get(off..off + 4) == Some(b"OTTO");
                            match extract_face(&bytes, off) {
                                Some(face) => (face, if is_cff { "font/otf" } else { "font/ttf" }),
                                None => (bytes, "font/collection"),
                            }
                        } else if e.format == "otf" {
                            (bytes, "font/otf")
                        } else {
                            (bytes, "font/ttf")
                        };
                        let mut extra = cors.clone();
                        extra.push("Cache-Control: private, max-age=3600".into());
                        extra.push(format!("X-Face-Index: {}", e.face_index));
                        respond(&mut stream, 200, ct, &bytes, &extra);
                    }
                    Err(_) => respond(&mut stream, 404, "text/plain", b"font file missing", &cors),
                },
                None => respond(&mut stream, 404, "text/plain", b"unknown font id", &cors),
            }
        }
        _ => respond(&mut stream, 404, "text/plain", b"not found", &cors),
    }
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8], extra: &[String]) {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n",
        body.len()
    );
    for h in extra {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn fonts_json(fonts: &[FontEntry]) -> String {
    let mut s = String::from("[");
    for (i, f) in fonts.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"id\":{},\"family\":{},\"style\":{},\"weight\":{},\"italic\":{},\"postscript\":{},\"source\":{},\"format\":{},\"size\":{},\"axes\":{}}}",
            json_str(&f.id),
            json_str(&f.family),
            json_str(&f.style),
            f.weight,
            f.italic,
            json_str(&f.postscript),
            json_str(f.source),
            json_str(f.format),
            f.size,
            axes_json(&f.axes)
        ));
    }
    s.push(']');
    s
}

fn axes_json(axes: &[Axis]) -> String {
    let mut s = String::from("[");
    for (i, a) in axes.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"tag\":{},\"min\":{},\"def\":{},\"max\":{}}}",
            json_str(&a.tag),
            a.min,
            a.def,
            a.max
        ));
    }
    s.push(']');
    s
}

#[allow(dead_code)]
fn _read_exact_unused<R: Read>(_r: R) {}
