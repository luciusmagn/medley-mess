use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_ROOT: &str = "/home/mag/docs";
const DEFAULT_REMOTE: &str = "git@github.com:luciusmagn/tedit-docs";
const DEFAULT_INTERVAL_SECS: u64 = 3600;
const TEDIT_TRAILER_MAGIC_BASE: u16 = 31415;

const PIECE_LOOKS: u16 = 0;
const PIECE_OBJECT: u16 = 1;
const PIECE_PARA: u16 = 2;
const PIECE_PAGEFRAME: u16 = 3;
const PIECE_CHARLOOKS_LIST: u16 = 4;
const PIECE_PARALOOKS_LIST: u16 = 5;
const PIECE_SAFEOBJECT: u16 = 6;
const PIECE_METAINFO: u16 = 7;
const PIECE_PROPERTIES: u16 = 8;

#[derive(Clone, Debug)]
struct Config {
    root: PathBuf,
    remote: String,
    interval_secs: u64,
    once: bool,
    push: bool,
}

#[derive(Clone, Debug)]
struct TeditDoc {
    text: String,
    text_bytes: Vec<u8>,
    trailer: String,
    header: Option<TeditHeader>,
    has_trailer: bool,
    nul_count: usize,
    styles: Vec<String>,
    formatting: Option<TeditFormatting>,
}

#[derive(Clone, Debug)]
struct TeditHeader {
    piece_start: usize,
    piece_count: u16,
    version: u16,
}

#[derive(Clone, Debug, Default)]
struct TeditFormatting {
    charlooks: Vec<CharLook>,
    paralooks: Vec<ParaLook>,
    spans: Vec<Span>,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct CharLook {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
}

#[derive(Clone, Debug, Default)]
struct ParaLook {
    heading: Option<u8>,
    code: bool,
}

#[derive(Clone, Debug)]
struct Span {
    start: usize,
    end: usize,
    charlook: usize,
    paralook: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct InlineStyle {
    code: bool,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

#[derive(Clone, Debug, Default)]
struct RenderSegment {
    text: String,
    heading: Option<u8>,
    style: InlineStyle,
}

#[derive(Clone, Debug, Default)]
struct RenderPart {
    text: String,
    style: InlineStyle,
}

#[derive(Clone, Debug, Default)]
struct RenderLine {
    parts: Vec<RenderPart>,
    heading: Option<u8>,
}

fn main() {
    let config = match parse_args() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            print_usage();
            std::process::exit(2);
        }
    };

    if config.once {
        if let Err(err) = run_once(&config) {
            eprintln!("tedit-doc-sync: {err}");
            std::process::exit(1);
        }
        return;
    }

    loop {
        if let Err(err) = run_once(&config) {
            eprintln!("tedit-doc-sync: {err}");
        }
        thread::sleep(Duration::from_secs(config.interval_secs));
    }
}

fn parse_args() -> Result<Config, String> {
    let mut config = Config {
        root: PathBuf::from(DEFAULT_ROOT),
        remote: DEFAULT_REMOTE.to_string(),
        interval_secs: DEFAULT_INTERVAL_SECS,
        once: false,
        push: true,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--once" => config.once = true,
            "--no-push" => config.push = false,
            "--root" => {
                let value = args.next().ok_or("--root requires a directory")?;
                config.root = PathBuf::from(value);
            }
            "--remote" => {
                config.remote = args.next().ok_or("--remote requires a git URL")?;
            }
            "--interval-seconds" => {
                let value = args.next().ok_or("--interval-seconds requires a number")?;
                config.interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| "--interval-seconds must be an integer")?;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }

    Ok(config)
}

fn print_usage() {
    eprintln!(
        "usage: tedit-doc-sync [--once] [--no-push] [--root DIR] [--remote URL] [--interval-seconds N]"
    );
}

fn run_once(config: &Config) -> io::Result<()> {
    fs::create_dir_all(&config.root)?;
    fs::create_dir_all(config.root.join("md"))?;

    let sources = collect_sources(&config.root)?;
    for source in &sources {
        render_source(&config.root, source)?;
    }

    ensure_git_repo(&config.root, &config.remote)?;
    commit_and_push(&config.root, config.push)?;
    Ok(())
}

fn collect_sources(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_sources_inner(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_sources_inner(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();

        if path.is_dir() {
            if should_skip_dir(&name) {
                continue;
            }
            collect_sources_inner(root, &path, out)?;
        } else if path.is_file() && is_tedit_source(root, &path) {
            out.push(path);
        }
    }
    Ok(())
}

fn should_skip_dir(name: &OsStr) -> bool {
    matches!(name.to_str(), Some(".git" | "md" | "org" | ".sync"))
}

fn is_tedit_source(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };

    if rel.components().any(|component| {
        let text = component.as_os_str().to_string_lossy();
        text == ".git" || text == "md" || text == "org" || text == ".sync"
    }) {
        return false;
    }

    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if name.starts_with('.') || name.ends_with(".md") || name.ends_with(".org") {
        return false;
    }
    true
}

fn render_source(root: &Path, source: &Path) -> io::Result<()> {
    let bytes = fs::read(source)?;
    let doc = parse_tedit(&bytes);
    let rel = source.strip_prefix(root).unwrap_or(source);
    let out_path = markdown_path(root, rel);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let source_mtime = fs::metadata(source)
        .and_then(|metadata| metadata.modified())
        .map(unix_seconds_from)
        .unwrap_or_else(|_| unix_seconds());
    let markdown = render_markdown(rel, &doc, source_mtime);
    write_if_changed(&out_path, markdown.as_bytes())
}

fn markdown_path(root: &Path, rel: &Path) -> PathBuf {
    let mut out_rel = rel.to_path_buf();
    let name = rel
        .file_name()
        .map(|name| format!("{}.md", name.to_string_lossy()))
        .unwrap_or_else(|| "document.md".to_string());
    out_rel.set_file_name(name);
    root.join("md").join(out_rel)
}

fn parse_tedit(bytes: &[u8]) -> TeditDoc {
    let header = read_tedit_header(bytes);
    let split = header
        .as_ref()
        .map(|header| header.piece_start)
        .or_else(|| bytes.iter().position(|byte| *byte == 0))
        .unwrap_or(bytes.len())
        .min(bytes.len());

    let text_bytes = &bytes[..split];
    let trailer_bytes = if split < bytes.len() { &bytes[split..] } else { &[] };
    let raw_text = String::from_utf8_lossy(text_bytes);
    let raw_trailer = String::from_utf8_lossy(trailer_bytes);
    let text = clean_text(&raw_text);
    let trailer = clean_trailer(&raw_trailer);
    let formatting = parse_formatting(bytes, text_bytes.len(), header.as_ref());
    let styles = detect_styles(&trailer, formatting.as_ref());

    TeditDoc {
        text,
        text_bytes: text_bytes.to_vec(),
        trailer,
        header: header.clone(),
        has_trailer: header.is_some() || !trailer_bytes.is_empty(),
        nul_count: bytes.iter().filter(|byte| **byte == 0).count(),
        styles,
        formatting,
    }
}

fn read_tedit_header(bytes: &[u8]) -> Option<TeditHeader> {
    if bytes.len() < 8 {
        return None;
    }
    let trailer_start = bytes.len() - 8;
    let piece_start = read_u32_at(bytes, trailer_start)? as usize;
    let piece_count = read_u16_at(bytes, trailer_start + 4)?;
    let raw_version = read_u16_at(bytes, trailer_start + 6)?;
    if raw_version < TEDIT_TRAILER_MAGIC_BASE {
        return None;
    }
    let version = raw_version - TEDIT_TRAILER_MAGIC_BASE;
    if !(1..=8).contains(&version) || piece_start > bytes.len() - 8 {
        return None;
    }
    Some(TeditHeader {
        piece_start,
        piece_count,
        version,
    })
}

fn clean_text(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        match ch {
            '\r' => out.push('\n'),
            '\t' | '\n' => out.push(ch),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out.trim_matches('\n').to_string()
}

fn clean_trailer(input: &str) -> String {
    input
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect::<String>()
}

fn detect_styles(trailer: &str, formatting: Option<&TeditFormatting>) -> Vec<String> {
    let mut styles = BTreeSet::new();
    for style in ["HEADING1", "HEADING2", "HEADING3", "NORMAL", "CODE"] {
        if trailer.contains(&format!("MAG-TEDIT {style}")) {
            styles.insert(style.to_ascii_lowercase());
        }
    }
    if trailer.contains("WEIGHT BOLD") {
        styles.insert("bold".to_string());
    }
    if trailer.contains("SLOPE ITALIC") {
        styles.insert("italic".to_string());
    }

    if let Some(formatting) = formatting {
        for look in &formatting.charlooks {
            if look.bold {
                styles.insert("bold".to_string());
            }
            if look.italic {
                styles.insert("italic".to_string());
            }
            if look.underline {
                styles.insert("underline".to_string());
            }
            if look.strike {
                styles.insert("strike".to_string());
            }
            if look.code {
                styles.insert("code".to_string());
            }
        }
        for look in &formatting.paralooks {
            if let Some(level) = look.heading {
                styles.insert(format!("heading{level}"));
            }
            if look.code {
                styles.insert("code".to_string());
            }
        }
    }

    styles.into_iter().collect()
}

fn parse_formatting(
    bytes: &[u8],
    text_len: usize,
    header: Option<&TeditHeader>,
) -> Option<TeditFormatting> {
    let header = header?;
    let mut formatting = TeditFormatting::default();
    formatting.charlooks.push(CharLook::default());
    formatting.paralooks.push(ParaLook::default());

    let trailer_meta_start = bytes.len().saturating_sub(8);
    let mut reader = Reader::new(bytes, header.piece_start);
    let mut text_cursor = 0usize;
    let mut current_para = None;
    let mut pieces_seen = 0usize;
    let mut guard = 0usize;

    while reader.pos + 6 <= trailer_meta_start && guard < 100_000 {
        guard += 1;
        let descriptor_pos = reader.pos;
        let Some(byte_len) = reader.read_u32() else {
            break;
        };
        let Some(kind) = reader.read_u16() else {
            break;
        };

        if kind > PIECE_PROPERTIES {
            break;
        }

        match kind {
            PIECE_LOOKS => {
                let Some(_flags) = reader.read_u8() else {
                    formatting
                        .notes
                        .push(format!("truncated looks descriptor at {descriptor_pos}"));
                    break;
                };
                let Some(charlook) = reader.read_u16() else {
                    formatting
                        .notes
                        .push(format!("truncated looks index at {descriptor_pos}"));
                    break;
                };
                let start = text_cursor.min(text_len);
                let next = text_cursor.saturating_add(byte_len as usize);
                let end = next.min(text_len);
                if start < end {
                    formatting.spans.push(Span {
                        start,
                        end,
                        charlook: charlook as usize,
                        paralook: current_para,
                    });
                }
                text_cursor = next;
                pieces_seen += 1;
            }
            PIECE_OBJECT | PIECE_SAFEOBJECT => {
                let _getfn = reader.read_len_string().unwrap_or_default();
                let start = text_cursor.min(text_len);
                let next = text_cursor.saturating_add(byte_len as usize);
                let end = next.min(text_len);
                if start < end {
                    formatting.spans.push(Span {
                        start,
                        end,
                        charlook: 0,
                        paralook: current_para,
                    });
                }
                text_cursor = next;
                pieces_seen += 1;
            }
            PIECE_PARA => {
                let Some(index) = reader.read_u16() else {
                    formatting
                        .notes
                        .push(format!("truncated paragraph descriptor at {descriptor_pos}"));
                    break;
                };
                current_para = Some(index as usize);
                pieces_seen += 1;
            }
            PIECE_PAGEFRAME => {
                let sexp_start = reader.pos;
                match skip_printed_lisp_object(bytes, sexp_start, trailer_meta_start) {
                    Some(next) => {
                        reader.pos = next;
                        pieces_seen += 1;
                    }
                    None => {
                        formatting
                            .notes
                            .push(format!("could not skip pageframe at {sexp_start}"));
                        break;
                    }
                }
            }
            PIECE_CHARLOOKS_LIST => {
                let Some(count) = reader.read_u16() else {
                    formatting
                        .notes
                        .push(format!("truncated charlooks list at {descriptor_pos}"));
                    break;
                };
                formatting.charlooks.clear();
                formatting.charlooks.push(CharLook::default());
                for index in 1..=count as usize {
                    match parse_charlook_record(&mut reader, index) {
                        Some(look) => formatting.charlooks.push(look),
                        None => {
                            formatting
                                .notes
                                .push(format!("truncated charlook {index} at byte {}", reader.pos));
                            break;
                        }
                    }
                }
            }
            PIECE_PARALOOKS_LIST => {
                let Some(count) = reader.read_u16() else {
                    formatting
                        .notes
                        .push(format!("truncated paralooks list at {descriptor_pos}"));
                    break;
                };
                formatting.paralooks.clear();
                formatting.paralooks.push(ParaLook::default());
                for index in 1..=count as usize {
                    match parse_paralook_record(&mut reader, index) {
                        Some(look) => formatting.paralooks.push(look),
                        None => {
                            formatting
                                .notes
                                .push(format!("truncated paralook {index} at byte {}", reader.pos));
                            break;
                        }
                    }
                }
            }
            PIECE_METAINFO | PIECE_PROPERTIES => {
                let skip = byte_len as usize;
                reader.pos = reader.pos.saturating_add(skip).min(trailer_meta_start);
            }
            _ => break,
        }

        if pieces_seen >= header.piece_count as usize && text_cursor >= text_len {
            break;
        }
    }

    if guard >= 100_000 {
        formatting
            .notes
            .push("format parser stopped by descriptor guard".to_string());
    }
    if formatting.spans.is_empty() && formatting.charlooks.len() <= 1 && formatting.paralooks.len() <= 1 {
        return None;
    }
    Some(formatting)
}

fn parse_charlook_record(reader: &mut Reader<'_>, _index: usize) -> Option<CharLook> {
    let start = reader.pos;
    let len = reader.read_u16()? as usize;
    if len < 2 {
        return None;
    }
    let end = start.checked_add(len)?.min(reader.bytes.len());

    let family = reader.read_len_string().unwrap_or_default();
    let _size = reader.read_u16().unwrap_or_default();
    let _offset = reader.read_i16().unwrap_or_default();
    let style = reader.read_len_string().unwrap_or_default();
    let props = reader.read_len_string().unwrap_or_default();
    let bits = reader.read_u16().unwrap_or_default();
    reader.pos = end;

    let upper_family = family.to_ascii_uppercase();
    let upper_style = style.to_ascii_uppercase();
    let upper_props = props.to_ascii_uppercase();
    Some(CharLook {
        bold: bits & 0x0200 != 0 || upper_style.contains("BOLD") || upper_props.contains("WEIGHT BOLD"),
        italic: bits & 0x0100 != 0
            || upper_style.contains("ITALIC")
            || upper_style.contains("OBLIQUE")
            || upper_props.contains("SLOPE ITALIC"),
        underline: bits & 0x0080 != 0 || upper_props.contains("UNDERLINE ON"),
        strike: bits & 0x0020 != 0 || upper_props.contains("STRIKEOUT ON"),
        code: upper_family.contains("GACHA")
            || upper_family.contains("MONO")
            || upper_props.contains("MAG-TEDIT CODE"),
    })
}

fn parse_paralook_record(reader: &mut Reader<'_>, _index: usize) -> Option<ParaLook> {
    let start = reader.pos;
    let len = reader.read_i16()?;
    if len < 2 {
        return None;
    }
    let end = start.checked_add(len as usize)?.min(reader.bytes.len());

    for _ in 0..6 {
        let _ = reader.read_i16()?;
    }
    let tab_flags = reader.read_u8()?;
    let _quad = reader.read_u8()?;

    if tab_flags & 1 != 0 {
        let _default_tab = reader.read_i16()?;
        let count = reader.read_u8()? as usize;
        for _ in 0..count {
            let _tab_x = reader.read_i16()?;
            let _tab_kind = reader.read_u8()?;
        }
    }

    let mut userinfo = String::new();
    let mut style = String::new();
    if tab_flags & 2 != 0 {
        let _special_x = reader.read_i16()?;
        let _special_y = reader.read_i16()?;
        userinfo = reader.read_len_string().unwrap_or_default();
        let _para_type = reader.read_len_string().unwrap_or_default();
        let _para_subtype = reader.read_len_string().unwrap_or_default();
        style = reader.read_len_string().unwrap_or_default();
        let _charstyles = reader.read_len_string().unwrap_or_default();
        let _new_page_before = reader.read_len_string().unwrap_or_default();
        let _new_page_after = reader.read_len_string().unwrap_or_default();
        let _heading_keep = reader.read_len_string().unwrap_or_default();
        let _keep = reader.read_len_string().unwrap_or_default();
        while reader.pos < end {
            let Some(len) = read_u16_at(reader.bytes, reader.pos) else {
                break;
            };
            let next = reader.pos.saturating_add(2).saturating_add(len as usize);
            if next > end {
                break;
            }
            reader.pos = next;
        }
    }

    reader.pos = end;
    let marker = format!("{} {}", userinfo.to_ascii_uppercase(), style.to_ascii_uppercase());
    let heading = if marker.contains("HEADING1") {
        Some(1)
    } else if marker.contains("HEADING2") {
        Some(2)
    } else if marker.contains("HEADING3") {
        Some(3)
    } else {
        None
    };

    Some(ParaLook {
        heading,
        code: marker.contains("MAG-TEDIT CODE") || marker.contains(" CODE"),
    })
}

fn skip_printed_lisp_object(bytes: &[u8], start: usize, limit: usize) -> Option<usize> {
    let mut pos = start;
    while pos < limit && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    if pos >= limit {
        return None;
    }

    if bytes[pos] != b'(' {
        return skip_atom(bytes, pos, limit);
    }

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[pos..limit].iter().enumerate() {
        let absolute = pos + offset;
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match *byte {
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(absolute + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_atom(bytes: &[u8], mut pos: usize, limit: usize) -> Option<usize> {
    while pos < limit && !bytes[pos].is_ascii_whitespace() {
        if bytes[pos] == 0 {
            break;
        }
        pos += 1;
    }
    Some(pos)
}

fn render_markdown(rel: &Path, doc: &TeditDoc, source_mtime: u64) -> String {
    let source = rel.to_string_lossy();
    let styles = if doc.styles.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", doc.styles.join(", "))
    };
    let span_count = doc
        .formatting
        .as_ref()
        .map(|formatting| formatting.spans.len())
        .unwrap_or(0);
    let charlook_count = doc
        .formatting
        .as_ref()
        .map(|formatting| formatting.charlooks.len().saturating_sub(1))
        .unwrap_or(0);
    let paralook_count = doc
        .formatting
        .as_ref()
        .map(|formatting| formatting.paralooks.len().saturating_sub(1))
        .unwrap_or(0);
    let notes = doc
        .formatting
        .as_ref()
        .map(|formatting| formatting.notes.join("; "))
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("source: {:?}\n", source));
    out.push_str("format: tedit\n");
    out.push_str(&format!("source_modified_unix_seconds: {}\n", source_mtime));
    out.push_str(&format!("has_tedit_trailer: {}\n", doc.has_trailer));
    if let Some(header) = &doc.header {
        out.push_str(&format!("tedit_version: {}\n", header.version));
        out.push_str(&format!("tedit_piece_start: {}\n", header.piece_start));
        out.push_str(&format!("tedit_piece_count: {}\n", header.piece_count));
    }
    out.push_str(&format!("tedit_trailer_chars: {}\n", doc.trailer.chars().count()));
    out.push_str(&format!("tedit_spans: {}\n", span_count));
    out.push_str(&format!("tedit_charlooks: {}\n", charlook_count));
    out.push_str(&format!("tedit_paralooks: {}\n", paralook_count));
    out.push_str(&format!("nul_bytes: {}\n", doc.nul_count));
    out.push_str(&format!("detected_styles: {}\n", styles));
    if !notes.is_empty() {
        out.push_str(&format!("parse_notes: {:?}\n", notes));
    }
    out.push_str("converter: tedit-doc-sync\n");
    out.push_str("---\n\n");

    let body = if let Some(formatting) = &doc.formatting {
        let rendered = render_formatted_markdown(&doc.text_bytes, formatting);
        if rendered.trim().is_empty() {
            normalize_markdown_body(&doc.text)
        } else {
            rendered
        }
    } else {
        normalize_markdown_body(&doc.text)
    };

    if body.trim().is_empty() {
        out.push_str("_No readable TEdit text payload extracted._\n");
    } else {
        out.push_str(&body);
        out.push('\n');
    }

    if doc.has_trailer {
        out.push_str("\n<!-- tedit-trailer-detected: yes; TEdit piece formatting was decoded where possible. -->\n");
    }
    out
}

fn render_formatted_markdown(text_bytes: &[u8], formatting: &TeditFormatting) -> String {
    let mut spans = formatting.spans.clone();
    spans.sort_by_key(|span| (span.start, span.end));

    let mut segments = Vec::new();
    let mut cursor = 0usize;
    for span in spans {
        if span.start > text_bytes.len() || span.end <= span.start {
            continue;
        }
        if cursor < span.start {
            segments.push(RenderSegment {
                text: clean_fragment(&text_bytes[cursor..span.start.min(text_bytes.len())]),
                heading: None,
                style: InlineStyle::default(),
            });
        }
        let start = span.start.max(cursor).min(text_bytes.len());
        let end = span.end.min(text_bytes.len());
        if start < end {
            let charlook = formatting.charlooks.get(span.charlook);
            let paralook = span
                .paralook
                .and_then(|index| formatting.paralooks.get(index));
            segments.push(RenderSegment {
                text: clean_fragment(&text_bytes[start..end]),
                heading: paralook.and_then(|look| look.heading),
                style: inline_style(charlook, paralook),
            });
            cursor = end;
        }
    }
    if cursor < text_bytes.len() {
        segments.push(RenderSegment {
            text: clean_fragment(&text_bytes[cursor..]),
            heading: None,
            style: InlineStyle::default(),
        });
    }

    render_segments_as_lines(&segments)
}

fn inline_style(charlook: Option<&CharLook>, paralook: Option<&ParaLook>) -> InlineStyle {
    InlineStyle {
        code: charlook.map(|look| look.code).unwrap_or(false)
            || paralook.map(|look| look.code).unwrap_or(false),
        bold: charlook.map(|look| look.bold).unwrap_or(false),
        italic: charlook.map(|look| look.italic).unwrap_or(false),
        underline: charlook.map(|look| look.underline).unwrap_or(false),
        strike: charlook.map(|look| look.strike).unwrap_or(false),
    }
}

fn clean_fragment(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    for ch in raw.chars() {
        match ch {
            '\r' => out.push('\n'),
            '\t' | '\n' => out.push(ch),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}

fn style_inline_text(text: &str, style: InlineStyle) -> String {
    if text.trim().is_empty() {
        return text.to_string();
    }

    if !(style.code || style.bold || style.italic || style.underline || style.strike) {
        return text.to_string();
    }

    let Some(first) = text.find(|ch: char| !ch.is_whitespace()) else {
        return text.to_string();
    };
    let last = text
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(first);
    let leading = &text[..first];
    let mut core = text[first..last].to_string();
    let trailing = &text[last..];

    if style.code {
        core = wrap_code(&core);
    } else {
        if style.bold && style.italic {
            core = format!("***{core}***");
        } else if style.bold {
            core = format!("**{core}**");
        } else if style.italic {
            core = format!("*{core}*");
        }
        if style.underline {
            core = format!("<u>{core}</u>");
        }
        if style.strike {
            core = format!("~~{core}~~");
        }
    }

    format!("{leading}{core}{trailing}")
}

fn wrap_code(text: &str) -> String {
    if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

fn render_segments_as_lines(segments: &[RenderSegment]) -> String {
    let mut lines: Vec<RenderLine> = vec![RenderLine::default()];

    for segment in segments {
        for ch in segment.text.chars() {
            if ch == '\n' {
                lines.push(RenderLine::default());
                continue;
            }
            if let Some(line) = lines.last_mut() {
                if !ch.is_whitespace() && line.heading.is_none() {
                    line.heading = segment.heading;
                }
                if let Some(part) = line.parts.last_mut() {
                    if part.style == segment.style {
                        part.text.push(ch);
                        continue;
                    }
                }
                line.parts.push(RenderPart {
                    text: ch.to_string(),
                    style: segment.style,
                });
            }
        }
    }

    let mut out = String::new();
    let mut in_code_block = false;
    for line in lines {
        if line_is_whole_code(&line) {
            if !in_code_block {
                out.push_str("```\n");
                in_code_block = true;
            }
            let plain = line_plain_text(&line);
            out.push_str(plain.trim_end());
            out.push('\n');
            continue;
        }

        if in_code_block {
            out.push_str("```\n");
            in_code_block = false;
        }

        let mut line_text = render_line_inline(&line).trim_end().to_string();
        if let Some(level) = line.heading {
            let trimmed = line_text.trim_start();
            if !trimmed.is_empty() {
                line_text = format!("{} {}", "#".repeat(level as usize), trimmed);
            }
        }
        if line_text.starts_with("```") {
            out.push('\\');
        }
        out.push_str(&line_text);
        out.push('\n');
    }

    if in_code_block {
        out.push_str("```\n");
    }

    out.trim_end_matches('\n').to_string()
}

fn line_is_whole_code(line: &RenderLine) -> bool {
    if line.heading.is_some() {
        return false;
    }

    let mut saw_non_ws = false;
    for part in &line.parts {
        for ch in part.text.chars() {
            if ch.is_whitespace() {
                continue;
            }
            saw_non_ws = true;
            if !part.style.code {
                return false;
            }
        }
    }
    saw_non_ws
}

fn line_plain_text(line: &RenderLine) -> String {
    let mut out = String::new();
    for part in &line.parts {
        out.push_str(&part.text);
    }
    out
}

fn render_line_inline(line: &RenderLine) -> String {
    let mut out = String::new();
    for part in &line.parts {
        out.push_str(&style_inline_text(&part.text, part.style));
    }
    out
}

fn normalize_markdown_body(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let trimmed_end = line.trim_end();
        if trimmed_end.starts_with("```") {
            out.push('\\');
        }
        out.push_str(trimmed_end);
        out.push('\n');
    }
    out.trim_end_matches('\n').to_string()
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return Ok(());
        }
    }
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)
}

fn ensure_git_repo(root: &Path, remote: &str) -> io::Result<()> {
    if !root.join(".git").exists() {
        run_git(root, &["init"])?;
        let _ = run_git(root, &["checkout", "-B", "main"]);
    }

    let _ = run_git(root, &["config", "user.name", "Lukáš Hozda"]);
    let _ = run_git(root, &["config", "user.email", "luk.hozda@gmail.com"]);

    let remotes = git_output(root, &["remote"])?;
    if !remotes.lines().any(|line| line == "origin") {
        let _ = run_git(root, &["remote", "add", "origin", remote]);
    } else {
        let _ = run_git(root, &["remote", "set-url", "origin", remote]);
    }
    Ok(())
}

fn commit_and_push(root: &Path, push: bool) -> io::Result<()> {
    run_git(root, &["add", "-A"])?;
    let status = git_output(root, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        if push {
            push_current_branch(root)?;
        }
        return Ok(());
    }

    let message = format!("Sync TEdit docs {}", unix_seconds());
    run_git(root, &["commit", "-m", &message])?;

    if push {
        push_current_branch(root)?;
    }
    Ok(())
}

fn push_current_branch(root: &Path) -> io::Result<()> {
    let branch = git_output(root, &["branch", "--show-current"])
        .unwrap_or_else(|_| "main".to_string())
        .trim()
        .to_string();
    let branch = if branch.is_empty() { "main" } else { &branch };
    let result = Command::new("git")
        .arg("push")
        .arg("-u")
        .arg("origin")
        .arg(branch)
        .current_dir(root)
        .env_remove("LD_LIBRARY_PATH")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=10")
        .output();
    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("git push failed: {stderr}"),
            ))
        }
        Err(err) => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("git push failed: {err}"),
        )),
    }
}

fn run_git(root: &Path, args: &[&str]) -> io::Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env_remove("LD_LIBRARY_PATH")
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ))
    }
}

fn git_output(root: &Path, args: &[&str]) -> io::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env_remove("LD_LIBRARY_PATH")
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ))
    }
}

fn read_u32_at(bytes: &[u8], pos: usize) -> Option<u32> {
    let slice = bytes.get(pos..pos + 4)?;
    Some(u32::from_be_bytes(slice.try_into().ok()?))
}

fn read_u16_at(bytes: &[u8], pos: usize) -> Option<u16> {
    let slice = bytes.get(pos..pos + 2)?;
    Some(u16::from_be_bytes(slice.try_into().ok()?))
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], pos: usize) -> Self {
        Self { bytes, pos }
    }

    fn read_u8(&mut self) -> Option<u8> {
        let value = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(value)
    }

    fn read_u16(&mut self) -> Option<u16> {
        let value = read_u16_at(self.bytes, self.pos)?;
        self.pos += 2;
        Some(value)
    }

    fn read_i16(&mut self) -> Option<i16> {
        let slice = self.bytes.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(i16::from_be_bytes(slice.try_into().ok()?))
    }

    fn read_u32(&mut self) -> Option<u32> {
        let value = read_u32_at(self.bytes, self.pos)?;
        self.pos += 4;
        Some(value)
    }

    fn read_len_string(&mut self) -> Option<String> {
        let len = self.read_u16()? as usize;
        let slice = self.bytes.get(self.pos..self.pos + len)?;
        self.pos += len;
        Some(String::from_utf8_lossy(slice).to_string())
    }
}

fn unix_seconds() -> u64 {
    unix_seconds_from(SystemTime::now())
}

fn unix_seconds_from(time: SystemTime) -> u64 {
    time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_inline_bold_inside_paragraph() {
        let mut formatting = TeditFormatting::default();
        formatting.charlooks.push(CharLook::default());
        formatting.charlooks.push(CharLook::default());
        formatting.charlooks.push(CharLook {
            bold: true,
            ..CharLook::default()
        });
        formatting.spans.push(Span {
            start: 0,
            end: 6,
            charlook: 1,
            paralook: None,
        });
        formatting.spans.push(Span {
            start: 6,
            end: 10,
            charlook: 2,
            paralook: None,
        });
        formatting.spans.push(Span {
            start: 10,
            end: 16,
            charlook: 1,
            paralook: None,
        });

        assert_eq!(
            render_formatted_markdown(b"hello bold world", &formatting),
            "hello **bold** world"
        );
    }

    #[test]
    fn renders_paragraph_heading() {
        let mut formatting = TeditFormatting::default();
        formatting.charlooks.push(CharLook::default());
        formatting.charlooks.push(CharLook::default());
        formatting.paralooks.push(ParaLook::default());
        formatting.paralooks.push(ParaLook {
            heading: Some(1),
            ..ParaLook::default()
        });
        formatting.spans.push(Span {
            start: 0,
            end: 5,
            charlook: 1,
            paralook: Some(1),
        });

        assert_eq!(render_formatted_markdown(b"Title", &formatting), "# Title");
    }

    #[test]
    fn renders_inline_code_inside_paragraph() {
        let mut formatting = TeditFormatting::default();
        formatting.charlooks.push(CharLook::default());
        formatting.charlooks.push(CharLook::default());
        formatting.charlooks.push(CharLook {
            code: true,
            ..CharLook::default()
        });
        formatting.spans.push(Span {
            start: 0,
            end: 4,
            charlook: 1,
            paralook: None,
        });
        formatting.spans.push(Span {
            start: 4,
            end: 7,
            charlook: 2,
            paralook: None,
        });
        formatting.spans.push(Span {
            start: 7,
            end: 12,
            charlook: 1,
            paralook: None,
        });

        assert_eq!(
            render_formatted_markdown(b"use foo now", &formatting),
            "use `foo` now"
        );
    }

    #[test]
    fn renders_whole_code_lines_as_one_fenced_block() {
        let mut formatting = TeditFormatting::default();
        formatting.charlooks.push(CharLook::default());
        formatting.charlooks.push(CharLook {
            code: true,
            ..CharLook::default()
        });
        formatting.spans.push(Span {
            start: 0,
            end: 23,
            charlook: 1,
            paralook: None,
        });

        assert_eq!(
            render_formatted_markdown(b"let x = 1;\nprintln!(x);", &formatting),
            "```\nlet x = 1;\nprintln!(x);\n```"
        );
    }

    #[test]
    fn reads_trailer_header() {
        let mut bytes = b"abc".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 3]);
        bytes.extend_from_slice(&[0, 1]);
        bytes.extend_from_slice(&(TEDIT_TRAILER_MAGIC_BASE + 3).to_be_bytes());
        let header = read_tedit_header(&bytes).expect("header");
        assert_eq!(header.piece_start, 3);
        assert_eq!(header.piece_count, 1);
        assert_eq!(header.version, 3);
    }
}
