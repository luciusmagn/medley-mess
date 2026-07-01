use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_MARKDOWN: &str = "/home/mag/docs/md/bitcoin.md";
const DEFAULT_TITLE: &str = "Bitcoin";
const DEFAULT_CONFIG: &str = "/home/mag/.config/tedit-google-sync/bitcoin.json";
const DEFAULT_CLIENT_SECRET: &str = "/home/mag/.config/tedit-google-sync/client_secret.json";
const DEFAULT_TOKEN: &str = "/home/mag/.local/state/tedit-google-sync/token.json";
const DEFAULT_STATE: &str = "/home/mag/.local/state/tedit-google-sync/bitcoin.state.json";
const DEFAULT_REDIRECT_PORT: u16 = 8766;
const SYNC_PREFIX: &str = "mag_sync_bitcoin_";
const DOCS_SCOPE: &str = "https://www.googleapis.com/auth/documents";
const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";

#[derive(Debug, Clone)]
struct Args {
    config_path: PathBuf,
    markdown: Option<PathBuf>,
    title: Option<String>,
    document_id: Option<String>,
    client_secret: Option<PathBuf>,
    token_path: Option<PathBuf>,
    state_path: Option<PathBuf>,
    once: bool,
    dry_run: bool,
    auth: bool,
    open_browser: bool,
    force: bool,
    allow_suggestions: bool,
    interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileConfig {
    markdown: Option<PathBuf>,
    title: Option<String>,
    document_id: Option<String>,
    client_secret: Option<PathBuf>,
    token_path: Option<PathBuf>,
    state_path: Option<PathBuf>,
    open_browser: Option<bool>,
    interval_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
struct Config {
    markdown: PathBuf,
    title: String,
    document_id: Option<String>,
    client_secret: PathBuf,
    token_path: PathBuf,
    state_path: PathBuf,
    dry_run: bool,
    auth: bool,
    open_browser: bool,
    force: bool,
    allow_suggestions: bool,
    interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SyncState {
    document_id: Option<String>,
    revision_id: Option<String>,
    title: Option<String>,
    markdown: Option<PathBuf>,
    blocks: Vec<StateBlock>,
    last_sync_unix_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateBlock {
    id: String,
    kind: String,
    text_hash: String,
    text: String,
    named_range_name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ClientSecretFile {
    installed: Option<OAuthClient>,
    web: Option<OAuthClient>,
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthClient {
    client_id: String,
    client_secret: Option<String>,
    auth_uri: Option<String>,
    token_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TokenFile {
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Paragraph,
    Heading(u8),
    Code,
}

#[derive(Debug, Clone)]
struct Block {
    id: String,
    kind: BlockKind,
    text: String,
    hash: String,
    inline: Vec<InlineSpan>,
    start_index: i64,
    end_index: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    code: bool,
}

#[derive(Debug, Clone)]
struct InlineSpan {
    start: i64,
    end: i64,
    style: InlineStyle,
}

#[derive(Debug, Clone)]
struct RenderedDoc {
    text: String,
    blocks: Vec<Block>,
}

#[derive(Debug, Clone, Default)]
struct ReviewState {
    unresolved_comment_count: usize,
    unmapped_comment_count: usize,
    commented_block_ids: BTreeSet<String>,
    suggestion_count: usize,
}

#[derive(Debug, Clone, Default)]
struct SyncPlan {
    created_document: bool,
    full_republish: bool,
    patch_blocks: Vec<String>,
    skipped_blocks: Vec<(String, String)>,
    warnings: Vec<String>,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let config = load_config(args)?;

    if config.auth {
        let mut auth = GoogleAuth::new(&config.client_secret, &config.token_path)?;
        auth.authorize(config.open_browser)?;
        println!("authorization complete: {}", config.token_path.display());
        return Ok(());
    }

    if config.interval_seconds == 0 {
        run_once(&config)?;
        return Ok(());
    }

    loop {
        if let Err(err) = run_once(&config) {
            eprintln!("tedit-google-sync: {err:#}");
        }
        std::thread::sleep(Duration::from_secs(config.interval_seconds));
    }
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        config_path: PathBuf::from(DEFAULT_CONFIG),
        markdown: None,
        title: None,
        document_id: None,
        client_secret: None,
        token_path: None,
        state_path: None,
        once: false,
        dry_run: false,
        auth: false,
        open_browser: false,
        force: false,
        allow_suggestions: false,
        interval_seconds: 0,
    };

    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => args.config_path = PathBuf::from(next_arg(&mut iter, "--config")?),
            "--markdown" => args.markdown = Some(PathBuf::from(next_arg(&mut iter, "--markdown")?)),
            "--title" => args.title = Some(next_arg(&mut iter, "--title")?),
            "--document-id" => args.document_id = Some(next_arg(&mut iter, "--document-id")?),
            "--client-secret" => {
                args.client_secret = Some(PathBuf::from(next_arg(&mut iter, "--client-secret")?))
            }
            "--token" => args.token_path = Some(PathBuf::from(next_arg(&mut iter, "--token")?)),
            "--state" => args.state_path = Some(PathBuf::from(next_arg(&mut iter, "--state")?)),
            "--interval-seconds" => {
                args.interval_seconds = next_arg(&mut iter, "--interval-seconds")?
                    .parse()
                    .context("--interval-seconds must be an integer")?;
            }
            "--once" => args.once = true,
            "--dry-run" => args.dry_run = true,
            "--auth" => args.auth = true,
            "--open-browser" => args.open_browser = true,
            "--force" => args.force = true,
            "--allow-suggestions" => args.allow_suggestions = true,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => bail!("unknown argument: {arg}"),
        }
    }

    if args.once {
        args.interval_seconds = 0;
    }
    Ok(args)
}

fn next_arg(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    iter.next()
        .ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn print_usage() {
    eprintln!(
        "usage: tedit-google-sync [--auth] [--dry-run] [--once] [--config PATH] [--markdown PATH] [--document-id ID] [--open-browser] [--force] [--allow-suggestions]"
    );
}

fn load_config(args: Args) -> Result<Config> {
    let file_config = if args.config_path.exists() {
        let text = fs::read_to_string(&args.config_path)
            .with_context(|| format!("read {}", args.config_path.display()))?;
        serde_json::from_str::<FileConfig>(&text)
            .with_context(|| format!("parse {}", args.config_path.display()))?
    } else {
        FileConfig {
            markdown: None,
            title: None,
            document_id: None,
            client_secret: None,
            token_path: None,
            state_path: None,
            open_browser: None,
            interval_seconds: None,
        }
    };

    Ok(Config {
        markdown: args
            .markdown
            .or(file_config.markdown)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MARKDOWN)),
        title: args
            .title
            .or(file_config.title)
            .unwrap_or_else(|| DEFAULT_TITLE.to_string()),
        document_id: args.document_id.or(file_config.document_id),
        client_secret: args
            .client_secret
            .or(file_config.client_secret)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CLIENT_SECRET)),
        token_path: args
            .token_path
            .or(file_config.token_path)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_TOKEN)),
        state_path: args
            .state_path
            .or(file_config.state_path)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE)),
        dry_run: args.dry_run,
        auth: args.auth,
        open_browser: args.open_browser || file_config.open_browser.unwrap_or(false),
        force: args.force,
        allow_suggestions: args.allow_suggestions,
        interval_seconds: file_config
            .interval_seconds
            .unwrap_or(args.interval_seconds),
    })
}

fn run_once(config: &Config) -> Result<()> {
    let markdown = fs::read_to_string(&config.markdown)
        .with_context(|| format!("read markdown {}", config.markdown.display()))?;
    let previous_state = read_state(&config.state_path)?;
    let raw_blocks = parse_markdown(&markdown);
    let blocks = assign_block_ids(raw_blocks, &previous_state);
    let rendered = render_doc(blocks);

    if config.dry_run {
        println!("markdown: {}", config.markdown.display());
        println!("blocks: {}", rendered.blocks.len());
        println!(
            "document_id: {}",
            document_id(config, &previous_state).unwrap_or_else(|| "<not configured>".to_string())
        );
        for block in &rendered.blocks {
            println!(
                "{}\t{}\t{}",
                block.id,
                kind_name(block.kind),
                one_line(&block.text)
            );
        }
        return Ok(());
    }

    let mut auth = GoogleAuth::new(&config.client_secret, &config.token_path)?;
    let mut google = GoogleClient::new(&mut auth)?;

    let mut state = previous_state;
    let mut plan = SyncPlan::default();
    let doc_id = match document_id(config, &state) {
        Some(id) => id,
        None => {
            let id = google.create_document(&config.title)?;
            state.document_id = Some(id.clone());
            plan.created_document = true;
            id
        }
    };

    let doc = google.get_document(&doc_id)?;
    let review = google.review_state(&doc_id, &doc, &state)?;
    let has_reviews = review.unresolved_comment_count > 0 || review.suggestion_count > 0;

    if review.suggestion_count > 0 && !config.allow_suggestions && !config.force {
        plan.warnings.push(format!(
            "{} unresolved Google Docs suggestions present; refusing to sync without --allow-suggestions or --force",
            review.suggestion_count
        ));
        print_plan(&plan);
        return Ok(());
    }

    let structure_changed = block_order(&rendered.blocks) != state_block_order(&state.blocks);
    let changed_blocks = changed_block_ids(&rendered.blocks, &state.blocks);

    if !has_reviews || config.force {
        google.full_republish(&doc_id, &doc, &rendered)?;
        state = state_from_rendered(
            &doc_id,
            &config.title,
            &config.markdown,
            &rendered,
            Some(google.get_revision(&doc_id)?),
        );
        write_state(&config.state_path, &state)?;
        plan.full_republish = true;
        print_plan(&plan);
        return Ok(());
    }

    if structure_changed {
        plan.warnings.push(
            "unresolved comments/suggestions exist and block structure changed; skipped structural sync to avoid detaching review anchors".to_string(),
        );
    }

    if review.unmapped_comment_count > 0 {
        plan.warnings.push(format!(
            "{} unresolved comments could not be mapped to a source block; changed blocks are skipped unless --force is used",
            review.unmapped_comment_count
        ));
    }

    let mut safe_blocks = Vec::new();
    for block in &rendered.blocks {
        if !changed_blocks.contains(&block.id) {
            continue;
        }
        if structure_changed {
            plan.skipped_blocks.push((
                block.id.clone(),
                "structure changed while review comments/suggestions exist".to_string(),
            ));
            continue;
        }
        if review.unmapped_comment_count > 0 {
            plan.skipped_blocks
                .push((block.id.clone(), "unmapped comments exist".to_string()));
            continue;
        }
        if review.commented_block_ids.contains(&block.id) {
            plan.skipped_blocks.push((
                block.id.clone(),
                "block has unresolved comments".to_string(),
            ));
            continue;
        }
        safe_blocks.push(block.clone());
    }

    if !safe_blocks.is_empty() {
        google.patch_blocks(&doc_id, &safe_blocks)?;
        let patched_doc = google.get_document(&doc_id)?;
        google.style_existing_blocks(&doc_id, &patched_doc, &safe_blocks)?;
        state = state_from_rendered(
            &doc_id,
            &config.title,
            &config.markdown,
            &rendered,
            Some(google.get_revision(&doc_id)?),
        );
        write_state(&config.state_path, &state)?;
        plan.patch_blocks = safe_blocks.iter().map(|block| block.id.clone()).collect();
    }

    print_plan(&plan);
    Ok(())
}

fn read_state(path: &Path) -> Result<SyncState> {
    if !path.exists() {
        return Ok(SyncState::default());
    }
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn write_state(path: &Path, state: &SyncState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(state)?;
    fs::write(path, format!("{text}\n")).with_context(|| format!("write {}", path.display()))
}

fn document_id(config: &Config, state: &SyncState) -> Option<String> {
    config
        .document_id
        .clone()
        .or_else(|| state.document_id.clone())
}

fn state_from_rendered(
    doc_id: &str,
    title: &str,
    markdown: &Path,
    rendered: &RenderedDoc,
    revision_id: Option<String>,
) -> SyncState {
    SyncState {
        document_id: Some(doc_id.to_string()),
        revision_id,
        title: Some(title.to_string()),
        markdown: Some(markdown.to_path_buf()),
        blocks: rendered
            .blocks
            .iter()
            .map(|block| StateBlock {
                id: block.id.clone(),
                kind: kind_name(block.kind).to_string(),
                text_hash: block.hash.clone(),
                text: block.text.clone(),
                named_range_name: named_range_name(&block.id),
            })
            .collect(),
        last_sync_unix_seconds: Some(now_unix()),
    }
}

fn parse_markdown(markdown: &str) -> Vec<Block> {
    let body = strip_front_matter(markdown);
    let lines: Vec<&str> = body.lines().collect();
    let mut blocks = Vec::new();
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty()
            || line
                .trim_start()
                .starts_with("<!-- tedit-trailer-detected:")
        {
            index += 1;
            continue;
        }

        if line.trim_start().starts_with("```") {
            index += 1;
            let mut text = String::new();
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(lines[index]);
                index += 1;
            }
            if index < lines.len() {
                index += 1;
            }
            blocks.push(raw_block(BlockKind::Code, text, vec![]));
            continue;
        }

        if let Some((level, content)) = parse_heading(line) {
            let (text, inline) = parse_inline(content.trim());
            blocks.push(raw_block(BlockKind::Heading(level), text, inline));
            index += 1;
            continue;
        }

        let mut paragraph = String::new();
        while index < lines.len() {
            let next = lines[index];
            if next.trim().is_empty()
                || next.trim_start().starts_with("```")
                || parse_heading(next).is_some()
                || next
                    .trim_start()
                    .starts_with("<!-- tedit-trailer-detected:")
            {
                break;
            }
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(next.trim());
            index += 1;
        }
        let (text, inline) = parse_inline(paragraph.trim());
        if !text.trim().is_empty() {
            blocks.push(raw_block(BlockKind::Paragraph, text, inline));
        }
    }

    blocks
}

fn strip_front_matter(markdown: &str) -> &str {
    let text = markdown.strip_prefix("---\n").unwrap_or(markdown);
    if std::ptr::eq(text.as_ptr(), markdown.as_ptr()) {
        return markdown;
    }
    if let Some(pos) = text.find("\n---\n") {
        &text[pos + 5..]
    } else {
        markdown
    }
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, rest.trim_start()))
}

fn raw_block(kind: BlockKind, text: String, inline: Vec<InlineSpan>) -> Block {
    let hash = block_hash(kind, &text);
    Block {
        id: String::new(),
        kind,
        text,
        hash,
        inline,
        start_index: 0,
        end_index: 0,
    }
}

fn parse_inline(input: &str) -> (String, Vec<InlineSpan>) {
    let mut out = String::new();
    let mut spans = Vec::new();
    let mut index = 0usize;
    let bytes = input.as_bytes();

    while index < input.len() {
        if bytes[index] == b'`' {
            if let Some(end) = input[index + 1..].find('`') {
                let content = &input[index + 1..index + 1 + end];
                push_styled(
                    &mut out,
                    &mut spans,
                    content,
                    InlineStyle {
                        code: true,
                        ..InlineStyle::default()
                    },
                );
                index += end + 2;
                continue;
            }
        }
        if input[index..].starts_with("**") {
            if let Some(end) = input[index + 2..].find("**") {
                let content = &input[index + 2..index + 2 + end];
                push_styled(
                    &mut out,
                    &mut spans,
                    content,
                    InlineStyle {
                        bold: true,
                        ..InlineStyle::default()
                    },
                );
                index += end + 4;
                continue;
            }
        }
        if bytes[index] == b'*' {
            if let Some(end) = input[index + 1..].find('*') {
                let content = &input[index + 1..index + 1 + end];
                push_styled(
                    &mut out,
                    &mut spans,
                    content,
                    InlineStyle {
                        italic: true,
                        ..InlineStyle::default()
                    },
                );
                index += end + 2;
                continue;
            }
        }

        let ch = input[index..].chars().next().unwrap();
        out.push(ch);
        index += ch.len_utf8();
    }

    (out, spans)
}

fn push_styled(out: &mut String, spans: &mut Vec<InlineSpan>, text: &str, style: InlineStyle) {
    let start = utf16_len(out);
    out.push_str(text);
    let end = utf16_len(out);
    if start < end {
        spans.push(InlineSpan { start, end, style });
    }
}

fn assign_block_ids(mut blocks: Vec<Block>, state: &SyncState) -> Vec<Block> {
    let mut exact: HashMap<(String, String), VecDeque<String>> = HashMap::new();
    for block in &state.blocks {
        exact
            .entry((block.kind.clone(), block.text_hash.clone()))
            .or_default()
            .push_back(block.id.clone());
    }

    let mut used = BTreeSet::new();
    for block in &mut blocks {
        let key = (kind_name(block.kind).to_string(), block.hash.clone());
        if let Some(ids) = exact.get_mut(&key) {
            while let Some(id) = ids.pop_front() {
                if used.insert(id.clone()) {
                    block.id = id;
                    break;
                }
            }
        }
    }

    for (idx, block) in blocks.iter_mut().enumerate() {
        if !block.id.is_empty() {
            continue;
        }
        if let Some(prev) = state.blocks.get(idx) {
            if prev.kind == kind_name(block.kind) && used.insert(prev.id.clone()) {
                block.id = prev.id.clone();
                continue;
            }
        }
        let base = format!("b{:04}_{}", idx + 1, &block.hash[..12]);
        let mut id = base.clone();
        let mut suffix = 2;
        while !used.insert(id.clone()) {
            id = format!("{base}_{suffix}");
            suffix += 1;
        }
        block.id = id;
    }

    blocks
}

fn render_doc(mut blocks: Vec<Block>) -> RenderedDoc {
    let mut text = String::new();
    let mut cursor = 1i64;
    let last = blocks.len().saturating_sub(1);

    for (idx, block) in blocks.iter_mut().enumerate() {
        block.start_index = cursor;
        text.push_str(&block.text);
        cursor += utf16_len(&block.text);
        block.end_index = cursor;
        if idx == last {
            text.push('\n');
            cursor += 1;
        } else {
            text.push_str("\n\n");
            cursor += 2;
        }
    }

    RenderedDoc { text, blocks }
}

fn block_hash(kind: BlockKind, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind_name(kind).as_bytes());
    hasher.update(b"\n");
    hasher.update(normalize_text(text).as_bytes());
    hex::encode(hasher.finalize())
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn kind_name(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Paragraph => "paragraph",
        BlockKind::Heading(1) => "heading1",
        BlockKind::Heading(2) => "heading2",
        BlockKind::Heading(3) => "heading3",
        BlockKind::Heading(_) => "heading",
        BlockKind::Code => "code",
    }
}

fn named_range_name(id: &str) -> String {
    format!("{SYNC_PREFIX}{id}")
}

fn block_order(blocks: &[Block]) -> Vec<String> {
    blocks.iter().map(|block| block.id.clone()).collect()
}

fn state_block_order(blocks: &[StateBlock]) -> Vec<String> {
    blocks.iter().map(|block| block.id.clone()).collect()
}

fn changed_block_ids(blocks: &[Block], state_blocks: &[StateBlock]) -> BTreeSet<String> {
    let old: HashMap<&str, &StateBlock> = state_blocks
        .iter()
        .map(|block| (block.id.as_str(), block))
        .collect();
    blocks
        .iter()
        .filter(|block| {
            old.get(block.id.as_str())
                .map(|prev| prev.text_hash.as_str())
                != Some(block.hash.as_str())
        })
        .map(|block| block.id.clone())
        .collect()
}

struct GoogleAuth {
    client: OAuthClient,
    token_path: PathBuf,
    token: TokenFile,
}

impl GoogleAuth {
    fn new(client_secret_path: &Path, token_path: &Path) -> Result<Self> {
        if !client_secret_path.exists() {
            bail!(
                "missing Google OAuth client JSON: {}\nCreate an OAuth desktop client in Google Cloud and put it there, then run `tedit-google-sync --auth`.",
                client_secret_path.display()
            );
        }
        let text = fs::read_to_string(client_secret_path)
            .with_context(|| format!("read {}", client_secret_path.display()))?;
        let secrets: ClientSecretFile = serde_json::from_str(&text)
            .with_context(|| format!("parse {}", client_secret_path.display()))?;
        let client = secrets.installed.or(secrets.web).ok_or_else(|| {
            anyhow!("client secret JSON has neither `installed` nor `web` section")
        })?;
        let token = if token_path.exists() {
            serde_json::from_str(&fs::read_to_string(token_path)?)?
        } else {
            TokenFile::default()
        };
        Ok(Self {
            client,
            token_path: token_path.to_path_buf(),
            token,
        })
    }

    fn authorize(&mut self, open_browser: bool) -> Result<()> {
        let redirect_uri = format!("http://127.0.0.1:{DEFAULT_REDIRECT_PORT}/oauth2/callback");
        let listener = TcpListener::bind(("127.0.0.1", DEFAULT_REDIRECT_PORT))
            .with_context(|| format!("bind OAuth redirect port {DEFAULT_REDIRECT_PORT}"))?;
        let auth_uri = self
            .client
            .auth_uri
            .as_deref()
            .unwrap_or("https://accounts.google.com/o/oauth2/v2/auth");
        let scope = format!("{DOCS_SCOPE} {DRIVE_SCOPE}");
        let url = format!(
            "{auth_uri}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
            url_encode(&self.client.client_id),
            url_encode(&redirect_uri),
            url_encode(&scope)
        );

        println!("Open this URL and approve access:\n\n{url}\n");
        if open_browser {
            let _ = Command::new("icecat")
                .arg(&url)
                .spawn()
                .or_else(|_| Command::new("xdg-open").arg(&url).spawn());
        }

        let (mut stream, _) = listener.accept().context("wait for OAuth redirect")?;
        let mut request = [0u8; 8192];
        let len = stream.read(&mut request)?;
        let request = String::from_utf8_lossy(&request[..len]);
        let first_line = request.lines().next().unwrap_or_default();
        let code = parse_oauth_code(first_line)?;
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nAuthorization complete. You can close this tab.\n";
        stream.write_all(response)?;

        self.exchange_code(&code, &redirect_uri)
    }

    fn access_token(&mut self) -> Result<String> {
        let now = now_unix();
        if let (Some(token), Some(expires_at)) = (&self.token.access_token, self.token.expires_at) {
            if expires_at > now + 60 {
                return Ok(token.clone());
            }
        }
        self.refresh()?;
        self.token
            .access_token
            .clone()
            .ok_or_else(|| anyhow!("OAuth refresh returned no access token"))
    }

    fn exchange_code(&mut self, code: &str, redirect_uri: &str) -> Result<()> {
        let token_uri = self
            .client
            .token_uri
            .as_deref()
            .unwrap_or("https://oauth2.googleapis.com/token");
        let mut form = vec![
            ("client_id", self.client.client_id.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ];
        if let Some(secret) = &self.client.client_secret {
            form.push(("client_secret", secret.as_str()));
        }
        let response: Value = ureq::post(token_uri).send_form(&form)?.into_json()?;
        self.update_token(response)?;
        self.save_token()
    }

    fn refresh(&mut self) -> Result<()> {
        let refresh = self
            .token
            .refresh_token
            .clone()
            .ok_or_else(|| anyhow!("no refresh token; run `tedit-google-sync --auth`"))?;
        let token_uri = self
            .client
            .token_uri
            .as_deref()
            .unwrap_or("https://oauth2.googleapis.com/token");
        let mut form = vec![
            ("client_id", self.client.client_id.as_str()),
            ("refresh_token", refresh.as_str()),
            ("grant_type", "refresh_token"),
        ];
        if let Some(secret) = &self.client.client_secret {
            form.push(("client_secret", secret.as_str()));
        }
        let response: Value = ureq::post(token_uri).send_form(&form)?.into_json()?;
        self.update_token(response)?;
        self.save_token()
    }

    fn update_token(&mut self, response: Value) -> Result<()> {
        if let Some(access) = response.get("access_token").and_then(Value::as_str) {
            self.token.access_token = Some(access.to_string());
        }
        if let Some(refresh) = response.get("refresh_token").and_then(Value::as_str) {
            self.token.refresh_token = Some(refresh.to_string());
        }
        if let Some(token_type) = response.get("token_type").and_then(Value::as_str) {
            self.token.token_type = Some(token_type.to_string());
        }
        if let Some(scope) = response.get("scope").and_then(Value::as_str) {
            self.token.scope = Some(scope.to_string());
        }
        let expires_in = response
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3600);
        self.token.expires_at = Some(now_unix() + expires_in);
        Ok(())
    }

    fn save_token(&self) -> Result<()> {
        if let Some(parent) = self.token_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &self.token_path,
            format!("{}\n", serde_json::to_string_pretty(&self.token)?),
        )?;
        Ok(())
    }
}

struct GoogleClient<'a> {
    auth: &'a mut GoogleAuth,
    agent: ureq::Agent,
}

impl<'a> GoogleClient<'a> {
    fn new(auth: &'a mut GoogleAuth) -> Result<Self> {
        Ok(Self {
            auth,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(60))
                .build(),
        })
    }

    fn create_document(&mut self, title: &str) -> Result<String> {
        let response = self.post_json(
            "https://docs.googleapis.com/v1/documents",
            json!({ "title": title }),
        )?;
        response
            .get("documentId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("Google Docs create response had no documentId"))
    }

    fn get_document(&mut self, doc_id: &str) -> Result<Value> {
        let url = format!(
            "https://docs.googleapis.com/v1/documents/{}?includeTabsContent=true&suggestionsViewMode=SUGGESTIONS_INLINE",
            url_encode(doc_id)
        );
        self.get_json(&url)
    }

    fn get_revision(&mut self, doc_id: &str) -> Result<String> {
        let doc = self.get_document(doc_id)?;
        Ok(doc
            .get("revisionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    fn review_state(
        &mut self,
        doc_id: &str,
        doc: &Value,
        state: &SyncState,
    ) -> Result<ReviewState> {
        let mut review = ReviewState::default();
        review.suggestion_count = count_suggestions(doc);

        let comments = self.list_comments(doc_id)?;
        for comment in comments {
            if comment
                .get("resolved")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            review.unresolved_comment_count += 1;
            let quote = comment
                .get("quotedFileContent")
                .and_then(|q| q.get("value"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(quote) = quote else {
                review.unmapped_comment_count += 1;
                continue;
            };
            let mut matched = false;
            for block in &state.blocks {
                if block.text.contains(quote) {
                    review.commented_block_ids.insert(block.id.clone());
                    matched = true;
                }
            }
            if !matched {
                review.unmapped_comment_count += 1;
            }
        }
        Ok(review)
    }

    fn list_comments(&mut self, doc_id: &str) -> Result<Vec<Value>> {
        let mut comments = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut url = format!(
                "https://www.googleapis.com/drive/v3/files/{}/comments?includeDeleted=false&pageSize=100&fields=comments(id,resolved,quotedFileContent,content,anchor,modifiedTime,deleted),nextPageToken",
                url_encode(doc_id)
            );
            if let Some(token) = &page_token {
                url.push_str("&pageToken=");
                url.push_str(&url_encode(token));
            }
            let response = self.get_json(&url)?;
            if let Some(list) = response.get("comments").and_then(Value::as_array) {
                comments.extend(list.iter().cloned());
            }
            page_token = response
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }
        Ok(comments)
    }

    fn full_republish(&mut self, doc_id: &str, doc: &Value, rendered: &RenderedDoc) -> Result<()> {
        let revision = doc
            .get("revisionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut requests = Vec::new();
        for name in existing_sync_ranges(doc).keys() {
            requests.push(json!({ "deleteNamedRange": { "name": name } }));
        }
        if let Some(end_index) = body_end_index(doc) {
            if end_index > 2 {
                requests.push(json!({
                    "deleteContentRange": { "range": { "startIndex": 1, "endIndex": end_index - 1 } }
                }));
            }
        }
        requests.push(json!({
            "insertText": { "location": { "index": 1 }, "text": rendered.text }
        }));
        requests.extend(style_requests(rendered));
        for block in &rendered.blocks {
            if block.start_index < block.end_index {
                requests.push(json!({
                    "createNamedRange": {
                        "name": named_range_name(&block.id),
                        "range": { "startIndex": block.start_index, "endIndex": block.end_index }
                    }
                }));
            }
        }
        self.batch_update(doc_id, revision, requests)
    }

    fn patch_blocks(&mut self, doc_id: &str, blocks: &[Block]) -> Result<()> {
        let doc = self.get_document(doc_id)?;
        let revision = doc
            .get("revisionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let ranges = existing_sync_ranges(&doc);
        let mut requests = Vec::new();
        for block in blocks {
            let name = named_range_name(&block.id);
            if !ranges.contains_key(&name) {
                bail!("named range missing for block {}", block.id);
            }
            requests.push(json!({
                "replaceNamedRangeContent": {
                    "namedRangeName": name,
                    "text": block.text
                }
            }));
        }
        if requests.is_empty() {
            return Ok(());
        }
        self.batch_update(doc_id, revision, requests)
    }

    fn style_existing_blocks(&mut self, doc_id: &str, doc: &Value, blocks: &[Block]) -> Result<()> {
        let revision = doc
            .get("revisionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let ranges = existing_sync_ranges(doc);
        let mut patched = Vec::new();
        for block in blocks {
            let name = named_range_name(&block.id);
            let Some((start, end)) = ranges.get(&name).copied() else {
                continue;
            };
            let mut next = block.clone();
            next.start_index = start;
            next.end_index = end;
            patched.push(next);
        }
        let rendered = RenderedDoc {
            text: String::new(),
            blocks: patched,
        };
        let requests = style_requests(&rendered);
        if requests.is_empty() {
            return Ok(());
        }
        self.batch_update(doc_id, revision, requests)
    }

    fn batch_update(&mut self, doc_id: &str, revision: &str, requests: Vec<Value>) -> Result<()> {
        if requests.is_empty() {
            return Ok(());
        }
        let url = format!(
            "https://docs.googleapis.com/v1/documents/{}:batchUpdate",
            url_encode(doc_id)
        );
        let body = if revision.is_empty() {
            json!({ "requests": requests })
        } else {
            json!({ "requests": requests, "writeControl": { "requiredRevisionId": revision } })
        };
        let _ = self.post_json(&url, body)?;
        Ok(())
    }

    fn get_json(&mut self, url: &str) -> Result<Value> {
        let token = self.auth.access_token()?;
        let response = self
            .agent
            .get(url)
            .set("Authorization", &format!("Bearer {token}"))
            .call();
        response_to_json(response)
    }

    fn post_json(&mut self, url: &str, body: Value) -> Result<Value> {
        let token = self.auth.access_token()?;
        let response = self
            .agent
            .post(url)
            .set("Authorization", &format!("Bearer {token}"))
            .send_json(body);
        response_to_json(response)
    }
}

fn response_to_json(response: std::result::Result<ureq::Response, ureq::Error>) -> Result<Value> {
    match response {
        Ok(resp) => Ok(resp.into_json()?),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            bail!("Google API HTTP {code}: {text}")
        }
        Err(err) => Err(err.into()),
    }
}

fn style_requests(rendered: &RenderedDoc) -> Vec<Value> {
    let mut requests = Vec::new();
    for block in &rendered.blocks {
        if block.start_index >= block.end_index {
            continue;
        }
        let range = json!({ "startIndex": block.start_index, "endIndex": block.end_index });
        requests.push(json!({
            "updateTextStyle": {
                "range": range,
                "textStyle": base_text_style(block.kind),
                "fields": "weightedFontFamily,fontSize,foregroundColor,bold,italic,backgroundColor"
            }
        }));
        requests.push(json!({
            "updateParagraphStyle": {
                "range": { "startIndex": block.start_index, "endIndex": block.end_index },
                "paragraphStyle": paragraph_style(block.kind),
                "fields": paragraph_style_fields(block.kind)
            }
        }));
        for span in &block.inline {
            let start = block.start_index + span.start;
            let end = block.start_index + span.end;
            if start >= end {
                continue;
            }
            requests.push(json!({
                "updateTextStyle": {
                    "range": { "startIndex": start, "endIndex": end },
                    "textStyle": inline_text_style(span.style),
                    "fields": inline_style_fields(span.style)
                }
            }));
        }
    }
    requests
}

fn base_text_style(kind: BlockKind) -> Value {
    match kind {
        BlockKind::Heading(1) => json!({
            "weightedFontFamily": { "fontFamily": "Georgia" },
            "fontSize": { "magnitude": 22.0, "unit": "PT" },
            "foregroundColor": { "color": { "rgbColor": { "red": 0.08, "green": 0.08, "blue": 0.08 } } },
            "bold": true,
            "italic": false
        }),
        BlockKind::Heading(2) => json!({
            "weightedFontFamily": { "fontFamily": "Georgia" },
            "fontSize": { "magnitude": 17.0, "unit": "PT" },
            "foregroundColor": { "color": { "rgbColor": { "red": 0.08, "green": 0.08, "blue": 0.08 } } },
            "bold": true,
            "italic": false
        }),
        BlockKind::Heading(_) => json!({
            "weightedFontFamily": { "fontFamily": "Georgia" },
            "fontSize": { "magnitude": 14.0, "unit": "PT" },
            "foregroundColor": { "color": { "rgbColor": { "red": 0.08, "green": 0.08, "blue": 0.08 } } },
            "bold": true,
            "italic": false
        }),
        BlockKind::Code => json!({
            "weightedFontFamily": { "fontFamily": "Courier New" },
            "fontSize": { "magnitude": 10.0, "unit": "PT" },
            "foregroundColor": { "color": { "rgbColor": { "red": 0.08, "green": 0.08, "blue": 0.08 } } },
            "backgroundColor": { "color": { "rgbColor": { "red": 0.94, "green": 0.93, "blue": 0.88 } } },
            "bold": false,
            "italic": false
        }),
        BlockKind::Paragraph => json!({
            "weightedFontFamily": { "fontFamily": "Georgia" },
            "fontSize": { "magnitude": 11.0, "unit": "PT" },
            "foregroundColor": { "color": { "rgbColor": { "red": 0.08, "green": 0.08, "blue": 0.08 } } },
            "bold": false,
            "italic": false
        }),
    }
}

fn paragraph_style(kind: BlockKind) -> Value {
    match kind {
        BlockKind::Heading(1) => {
            json!({ "namedStyleType": "HEADING_1", "spaceAbove": { "magnitude": 18.0, "unit": "PT" }, "spaceBelow": { "magnitude": 8.0, "unit": "PT" } })
        }
        BlockKind::Heading(2) => {
            json!({ "namedStyleType": "HEADING_2", "spaceAbove": { "magnitude": 14.0, "unit": "PT" }, "spaceBelow": { "magnitude": 6.0, "unit": "PT" } })
        }
        BlockKind::Heading(_) => {
            json!({ "namedStyleType": "HEADING_3", "spaceAbove": { "magnitude": 12.0, "unit": "PT" }, "spaceBelow": { "magnitude": 5.0, "unit": "PT" } })
        }
        BlockKind::Code => json!({
            "namedStyleType": "NORMAL_TEXT",
            "spaceAbove": { "magnitude": 6.0, "unit": "PT" },
            "spaceBelow": { "magnitude": 6.0, "unit": "PT" },
            "shading": { "backgroundColor": { "color": { "rgbColor": { "red": 0.94, "green": 0.93, "blue": 0.88 } } } }
        }),
        BlockKind::Paragraph => {
            json!({ "namedStyleType": "NORMAL_TEXT", "spaceAbove": { "magnitude": 0.0, "unit": "PT" }, "spaceBelow": { "magnitude": 6.0, "unit": "PT" } })
        }
    }
}

fn paragraph_style_fields(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Code => "namedStyleType,spaceAbove,spaceBelow,shading",
        _ => "namedStyleType,spaceAbove,spaceBelow",
    }
}

fn inline_text_style(style: InlineStyle) -> Value {
    let mut map = serde_json::Map::new();
    if style.bold {
        map.insert("bold".to_string(), json!(true));
    }
    if style.italic {
        map.insert("italic".to_string(), json!(true));
    }
    if style.code {
        map.insert(
            "weightedFontFamily".to_string(),
            json!({ "fontFamily": "Courier New" }),
        );
        map.insert(
            "backgroundColor".to_string(),
            json!({ "color": { "rgbColor": { "red": 0.94, "green": 0.93, "blue": 0.88 } } }),
        );
    }
    Value::Object(map)
}

fn inline_style_fields(style: InlineStyle) -> String {
    let mut fields = Vec::new();
    if style.bold {
        fields.push("bold");
    }
    if style.italic {
        fields.push("italic");
    }
    if style.code {
        fields.push("weightedFontFamily");
        fields.push("backgroundColor");
    }
    fields.join(",")
}

fn existing_sync_ranges(doc: &Value) -> BTreeMap<String, (i64, i64)> {
    let mut out = BTreeMap::new();
    let Some(named_ranges) = doc.get("namedRanges").and_then(Value::as_object) else {
        return out;
    };
    for (name, value) in named_ranges {
        if !name.starts_with(SYNC_PREFIX) {
            continue;
        }
        let Some(first) = value
            .get("namedRanges")
            .and_then(Value::as_array)
            .and_then(|ranges| ranges.first())
        else {
            continue;
        };
        let Some(range) = first
            .get("ranges")
            .and_then(Value::as_array)
            .and_then(|ranges| ranges.first())
        else {
            continue;
        };
        let Some(start) = range.get("startIndex").and_then(Value::as_i64) else {
            continue;
        };
        let Some(end) = range.get("endIndex").and_then(Value::as_i64) else {
            continue;
        };
        out.insert(name.clone(), (start, end));
    }
    out
}

fn body_end_index(doc: &Value) -> Option<i64> {
    doc.get("body")
        .and_then(|body| body.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| content.last())
        .and_then(|last| last.get("endIndex"))
        .and_then(Value::as_i64)
}

fn count_suggestions(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            let mut count = 0;
            for (key, value) in map {
                if key.starts_with("suggested") {
                    match value {
                        Value::Array(items) if !items.is_empty() => count += items.len(),
                        Value::Object(items) if !items.is_empty() => count += items.len(),
                        _ => {}
                    }
                }
                count += count_suggestions(value);
            }
            count
        }
        Value::Array(items) => items.iter().map(count_suggestions).sum(),
        _ => 0,
    }
}

fn parse_oauth_code(first_line: &str) -> Result<String> {
    let path = first_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("malformed OAuth redirect request"))?;
    let query = path
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    for part in query.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key == "code" {
            return Ok(url_decode(value));
        }
        if key == "error" {
            bail!("OAuth error: {}", url_decode(value));
        }
    }
    bail!("OAuth redirect had no code")
}

fn url_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn url_decode(input: &str) -> String {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                    if let Ok(value) = u8::from_str_radix(hex, 16) {
                        out.push(value);
                        index += 3;
                        continue;
                    }
                }
                out.push(bytes[index]);
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn utf16_len(text: &str) -> i64 {
    text.encode_utf16().count() as i64
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn one_line(text: &str) -> String {
    let mut out = text.replace('\n', " ⏎ ");
    if out.len() > 90 {
        out.truncate(87);
        out.push_str("...");
    }
    out
}

fn print_plan(plan: &SyncPlan) {
    if plan.created_document {
        println!("created Google Doc");
    }
    if plan.full_republish {
        println!("full republish applied");
    }
    for id in &plan.patch_blocks {
        println!("patched block {id}");
    }
    for (id, reason) in &plan.skipped_blocks {
        println!("skipped block {id}: {reason}");
    }
    for warning in &plan.warnings {
        println!("warning: {warning}");
    }
    if !plan.created_document
        && !plan.full_republish
        && plan.patch_blocks.is_empty()
        && plan.skipped_blocks.is_empty()
        && plan.warnings.is_empty()
    {
        println!("no Google Doc changes needed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_front_matter_heading_and_trailer_comment() {
        let markdown = r#"---
source: "bitcoin"
---

# **How Bitcoin evolves**
Body text with `code`.

<!-- tedit-trailer-detected: yes; TEdit piece formatting was decoded where possible. -->
"#;
        let blocks = assign_block_ids(parse_markdown(markdown), &SyncState::default());

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(blocks[0].text, "How Bitcoin evolves");
        assert!(blocks[0].inline.iter().any(|span| span.style.bold));
        assert_eq!(blocks[1].kind, BlockKind::Paragraph);
        assert_eq!(blocks[1].text, "Body text with code.");
        assert!(blocks[1].inline.iter().any(|span| span.style.code));
    }

    #[test]
    fn changed_block_keeps_previous_id_by_position() {
        let mut initial = assign_block_ids(
            parse_markdown("# Title\n\nFirst paragraph."),
            &SyncState::default(),
        );
        let rendered = render_doc(initial.clone());
        let state = state_from_rendered("doc", "Bitcoin", Path::new("bitcoin.md"), &rendered, None);

        let changed = assign_block_ids(parse_markdown("# Title\n\nChanged paragraph."), &state);

        assert_eq!(changed[0].id, initial.remove(0).id);
        assert_eq!(changed[1].id, state.blocks[1].id);
        assert_ne!(changed[1].hash, state.blocks[1].text_hash);
    }

    #[test]
    fn default_config_targets_one_explicit_bitcoin_markdown_file() {
        let args = Args {
            config_path: PathBuf::from("/path/that/does/not/exist"),
            markdown: None,
            title: None,
            document_id: None,
            client_secret: None,
            token_path: None,
            state_path: None,
            once: true,
            dry_run: true,
            auth: false,
            open_browser: false,
            force: false,
            allow_suggestions: false,
            interval_seconds: 0,
        };
        let config = load_config(args).unwrap();

        assert_eq!(config.markdown, PathBuf::from(DEFAULT_MARKDOWN));
        assert!(!config.markdown.to_string_lossy().contains("~"));
    }
}
