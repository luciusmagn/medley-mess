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
    trailer: String,
    has_trailer: bool,
    nul_count: usize,
    styles: Vec<String>,
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

    let markdown = render_markdown(rel, &doc);
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
    let first_nul = bytes.iter().position(|byte| *byte == 0);
    let (text_bytes, trailer_bytes) = match first_nul {
        Some(index) => {
            let trailer_start = bytes[index..]
                .iter()
                .position(|byte| *byte != 0)
                .map(|offset| index + offset)
                .unwrap_or(bytes.len());
            (&bytes[..index], &bytes[trailer_start..])
        }
        None => (bytes, &[][..]),
    };

    let raw_text = String::from_utf8_lossy(text_bytes);
    let raw_trailer = String::from_utf8_lossy(trailer_bytes);
    let text = clean_text(&raw_text);
    let trailer = clean_trailer(&raw_trailer);
    let styles = detect_styles(&trailer);

    TeditDoc {
        text,
        trailer,
        has_trailer: !trailer_bytes.is_empty(),
        nul_count: bytes.iter().filter(|byte| **byte == 0).count(),
        styles,
    }
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

fn detect_styles(trailer: &str) -> Vec<String> {
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
    styles.into_iter().collect()
}

fn render_markdown(rel: &Path, doc: &TeditDoc) -> String {
    let source = rel.to_string_lossy();
    let styles = if doc.styles.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", doc.styles.join(", "))
    };

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("source: {:?}\n", source));
    out.push_str("format: tedit\n");
    out.push_str(&format!("converted_unix_seconds: {}\n", unix_seconds()));
    out.push_str(&format!("has_tedit_trailer: {}\n", doc.has_trailer));
    out.push_str(&format!("tedit_trailer_chars: {}\n", doc.trailer.chars().count()));
    out.push_str(&format!("nul_bytes: {}\n", doc.nul_count));
    out.push_str(&format!("detected_styles: {}\n", styles));
    out.push_str("converter: tedit-doc-sync\n");
    out.push_str("---\n\n");

    if doc.text.trim().is_empty() {
        out.push_str("_No readable TEdit text payload extracted._\n");
    } else {
        out.push_str(&normalize_markdown_body(&doc.text));
        out.push('\n');
    }

    if doc.has_trailer {
        out.push_str("\n<!-- tedit-trailer-detected: yes; full piece-range style mapping is preserved in the source TEdit file. -->\n");
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
        return Ok(());
    }

    let message = format!("Sync TEdit docs {}", unix_seconds());
    run_git(root, &["commit", "-m", &message])?;

    if push {
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
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                eprintln!(
                    "tedit-doc-sync: git push failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(err) => eprintln!("tedit-doc-sync: git push failed: {err}"),
        }
    }
    Ok(())
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

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
