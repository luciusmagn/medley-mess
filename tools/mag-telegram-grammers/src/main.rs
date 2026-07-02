use anyhow::{anyhow, bail, Context, Result};
use grammers_client::message::Message;
use grammers_client::{Client, SenderPool};
use grammers_session::storages::SqliteSession;
use grammers_session::types::{DcOption, PeerId, PeerInfo, PeerKind, UpdatesState};
use grammers_session::{Session, SessionData};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime;

const DEFAULT_CONFIG: &str = "/home/mag/.config/mag-telegram/config";
const DEFAULT_SHODAN_CREDENTIALS: &str =
    "/home/mag/shodan-telegram-session/telegram-credentials.env";
const DEFAULT_SOURCE_SESSION: &str = "/home/mag/shodan-telegram-session/telegram-user.session";
const DEFAULT_SQLITE_SESSION: &str = "/home/mag/.local/share/mag-telegram/grammers/session.sqlite";
const DEFAULT_DAEMON_REQUEST: &str = "/tmp/mag-telegram-grammers-request";
const DEFAULT_DAEMON_RESPONSE: &str = "/tmp/mag-telegram-grammers-response";
const DEFAULT_DAEMON_PID: &str = "/tmp/mag-telegram-grammers.pid";
const DEFAULT_TIMEOUT_SECONDS: u64 = 20;
const DAEMON_LOOP_SLEEP_MS: u64 = 100;
const DIALOG_CACHE_TTL_SECONDS: u64 = 180;
const MESSAGE_CACHE_TTL_SECONDS: u64 = 180;
const MESSAGE_CACHE_MAX_PAGES: usize = 64;

#[derive(Debug, Clone)]
struct Config {
    api_id: i32,
    api_hash: String,
    config_path: PathBuf,
    shodan_credentials: PathBuf,
    source_session: PathBuf,
    sqlite_session: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RawSessionData {
    home_dc: i32,
    dc_options: BTreeMap<String, DcOption>,
    peer_infos: BTreeMap<String, PeerInfo>,
    updates_state: UpdatesState,
}

#[derive(Debug, Clone, Copy)]
struct OnlineOptions {
    import_if_missing: bool,
    timeout: Duration,
}

#[derive(Debug, Clone)]
struct DialogRow {
    id: i64,
    kind: &'static str,
    name: String,
    unread_count: i32,
    preview: Option<String>,
}

#[derive(Debug, Clone)]
struct MessageRow {
    id: i32,
    sender: String,
    body: String,
}

#[derive(Debug, Clone)]
struct MessagePage {
    peer_dialog_id: i64,
    from_message_id: i64,
    limit: usize,
    show_names: bool,
    rows: Vec<MessageRow>,
    oldest_id: i32,
    newest_id: i32,
    loaded_at: Instant,
}

#[derive(Debug)]
struct DaemonState {
    dialogs_total: usize,
    dialogs: Vec<DialogRow>,
    dialogs_show_names: bool,
    dialogs_loaded_at: Option<Instant>,
    message_pages: Vec<MessagePage>,
}

#[derive(Debug, Clone)]
enum Command {
    Doctor,
    SessionInfo,
    ImportSession {
        force: bool,
    },
    OnlineStatus,
    Chats {
        limit: usize,
        show_names: bool,
    },
    Messages {
        peer: i64,
        limit: usize,
        show_names: bool,
    },
    Send {
        peer: i64,
        text: String,
    },
    RequestFile {
        path: PathBuf,
    },
    Daemon,
    Help,
}

fn main() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let command = parse_command(&mut args)?;
    let config = load_config()?;

    match command {
        Command::Doctor => print_doctor(&config),
        Command::SessionInfo => print_session_info(&config.source_session),
        Command::ImportSession { force } => {
            let imported = import_session(&config, force)?;
            println!("imported={}", yesno(imported));
            println!("sqlite-session={}", config.sqlite_session.display());
            Ok(())
        }
        Command::OnlineStatus => with_runtime(online_status(&config, OnlineOptions::default())),
        Command::Chats { limit, show_names } => with_runtime(print_chats(
            &config,
            limit,
            show_names,
            OnlineOptions::default(),
        )),
        Command::Messages {
            peer,
            limit,
            show_names,
        } => with_runtime(print_messages(
            &config,
            peer,
            limit,
            show_names,
            OnlineOptions::default(),
        )),
        Command::Send { peer, text } => {
            with_runtime(send_message(&config, peer, &text, OnlineOptions::default()))
        }
        Command::RequestFile { path } => handle_request_file(&config, &path),
        Command::Daemon => with_runtime(run_daemon(&config)),
        Command::Help => {
            print_help();
            Ok(())
        }
    }
}

impl Default for OnlineOptions {
    fn default() -> Self {
        Self {
            import_if_missing: true,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        }
    }
}

fn with_runtime<T, F>(future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?
        .block_on(future)
}

fn parse_command(args: &mut Vec<String>) -> Result<Command> {
    let mut limit = 30usize;
    let mut show_names = false;
    let mut force = false;
    let mut i = 0usize;

    while i < args.len() {
        match args[i].as_str() {
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--limit needs a value"))?
                    .parse::<usize>()
                    .context("--limit must be a positive integer")?;
                limit = value.max(1);
                args.drain(i..=i + 1);
            }
            "--show-names" => {
                show_names = true;
                args.remove(i);
            }
            "--redact" => {
                show_names = false;
                args.remove(i);
            }
            "--force" => {
                force = true;
                args.remove(i);
            }
            "--help" | "-h" => return Ok(Command::Help),
            _ => i += 1,
        }
    }

    match args.first().map(String::as_str) {
        None | Some("help") => Ok(Command::Help),
        Some("doctor") => Ok(Command::Doctor),
        Some("session-info") => Ok(Command::SessionInfo),
        Some("import-session") => Ok(Command::ImportSession { force }),
        Some("online-status") => Ok(Command::OnlineStatus),
        Some("chats") => Ok(Command::Chats { limit, show_names }),
        Some("messages") => {
            let peer = args
                .get(1)
                .ok_or_else(|| anyhow!("messages needs a peer dialog id"))?
                .parse::<i64>()
                .context("messages peer must be an integer dialog id")?;
            Ok(Command::Messages {
                peer,
                limit,
                show_names,
            })
        }
        Some("send") => {
            let peer = args
                .get(1)
                .ok_or_else(|| anyhow!("send needs a peer dialog id"))?
                .parse::<i64>()
                .context("send peer must be an integer dialog id")?;
            let text = args.get(2..).unwrap_or(&[]).join(" ");
            if text.trim().is_empty() {
                bail!("send needs message text");
            }
            Ok(Command::Send { peer, text })
        }
        Some("--request-file") => {
            let path = args
                .get(1)
                .ok_or_else(|| anyhow!("--request-file needs a path"))?;
            Ok(Command::RequestFile {
                path: PathBuf::from(path),
            })
        }
        Some("--daemon") => Ok(Command::Daemon),
        Some(other) => bail!("unknown command: {other}"),
    }
}

fn print_help() {
    println!(
        "mag-telegram-grammers commands:
  doctor
  session-info
  import-session [--force]
  online-status
  chats [--limit N] [--show-names|--redact]
  messages PEER_DIALOG_ID [--limit N] [--show-names|--redact]
  send PEER_DIALOG_ID TEXT
  --request-file PATH
  --daemon

Defaults:
  config:         {DEFAULT_CONFIG}
  source session: {DEFAULT_SOURCE_SESSION}
  sqlite session: {DEFAULT_SQLITE_SESSION}
  daemon request: {DEFAULT_DAEMON_REQUEST}
  daemon response:{DEFAULT_DAEMON_RESPONSE}"
    );
}

fn load_config() -> Result<Config> {
    let config_path = PathBuf::from(
        env::var("MAG_TELEGRAM_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG.to_string()),
    );
    let shodan_credentials = PathBuf::from(
        env::var("MAG_TELEGRAM_SHODAN_CREDENTIALS")
            .unwrap_or_else(|_| DEFAULT_SHODAN_CREDENTIALS.to_string()),
    );
    let source_session = PathBuf::from(
        env::var("MAG_TELEGRAM_GRAMMERS_SOURCE_SESSION")
            .unwrap_or_else(|_| DEFAULT_SOURCE_SESSION.to_string()),
    );
    let sqlite_session = PathBuf::from(
        env::var("MAG_TELEGRAM_GRAMMERS_SESSION")
            .unwrap_or_else(|_| DEFAULT_SQLITE_SESSION.to_string()),
    );

    let mut api_id = env::var("MAG_TELEGRAM_API_ID")
        .ok()
        .and_then(|v| v.parse::<i32>().ok());
    let mut api_hash = env::var("MAG_TELEGRAM_API_HASH").ok();

    if config_path.exists() {
        let pairs = read_key_value_file(&config_path)?;
        if api_id.is_none() {
            api_id = pairs.get("api_id").and_then(|v| v.parse::<i32>().ok());
        }
        if api_hash.is_none() {
            api_hash = pairs.get("api_hash").cloned();
        }
    }

    if shodan_credentials.exists() {
        let pairs = read_key_value_file(&shodan_credentials)?;
        if api_id.is_none() {
            api_id = pairs
                .get("SHODAN_TELEGRAM_API_ID")
                .and_then(|v| v.parse::<i32>().ok());
        }
        if api_hash.is_none() {
            api_hash = pairs.get("SHODAN_TELEGRAM_API_HASH").cloned();
        }
    }

    Ok(Config {
        api_id: api_id.ok_or_else(|| anyhow!("missing Telegram api_id"))?,
        api_hash: api_hash.ok_or_else(|| anyhow!("missing Telegram api_hash"))?,
        config_path,
        shodan_credentials,
        source_session,
        sqlite_session,
    })
}

fn read_key_value_file(path: &Path) -> Result<HashMap<String, String>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read config file {}", path.display()))?;
    let mut out = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        out.insert(key.trim().to_string(), value);
    }
    Ok(out)
}

fn print_doctor(config: &Config) -> Result<()> {
    print!("{}", doctor_text(config)?);
    Ok(())
}

fn print_session_info(path: &Path) -> Result<()> {
    print!("{}", session_info_text(path)?);
    Ok(())
}

fn doctor_text(config: &Config) -> Result<String> {
    let mut out = String::new();
    push_line(&mut out, "Mag Telegram grammers doctor");
    push_line(&mut out, format!("config={}", config.config_path.display()));
    push_line(
        &mut out,
        format!(
            "shodan-credentials={} exists={}",
            config.shodan_credentials.display(),
            yesno(config.shodan_credentials.exists())
        ),
    );
    push_line(&mut out, "api-id=set");
    push_line(
        &mut out,
        format!(
            "api-hash={}",
            if config.api_hash.is_empty() {
                "empty"
            } else {
                "set"
            }
        ),
    );
    push_line(
        &mut out,
        format!(
            "source-session={} exists={}",
            config.source_session.display(),
            yesno(config.source_session.exists())
        ),
    );
    push_line(
        &mut out,
        format!(
            "sqlite-session={} exists={}",
            config.sqlite_session.display(),
            yesno(config.sqlite_session.exists())
        ),
    );
    out.push_str(&session_info_text(&config.source_session)?);
    Ok(out)
}

fn session_info_text(path: &Path) -> Result<String> {
    let data = load_json_session(path)?;
    let auth_keys = data
        .dc_options
        .values()
        .filter(|dc| dc.auth_key.is_some())
        .count();
    let channels = data.updates_state.channels.len();
    let users = data
        .peer_infos
        .values()
        .filter(|p| matches!(p, PeerInfo::User { .. }))
        .count();
    let chats = data
        .peer_infos
        .values()
        .filter(|p| matches!(p, PeerInfo::Chat { .. }))
        .count();
    let channel_peers = data
        .peer_infos
        .values()
        .filter(|p| matches!(p, PeerInfo::Channel { .. }))
        .count();

    let mut out = String::new();
    push_line(&mut out, "Mag Telegram grammers session");
    push_line(&mut out, format!("path={}", path.display()));
    push_line(&mut out, format!("home-dc={}", data.home_dc));
    push_line(&mut out, format!("dc-count={}", data.dc_options.len()));
    push_line(&mut out, format!("dc-auth-key-count={auth_keys}"));
    push_line(&mut out, format!("peer-count={}", data.peer_infos.len()));
    push_line(&mut out, format!("peer-users={users}"));
    push_line(&mut out, format!("peer-chats={chats}"));
    push_line(&mut out, format!("peer-channels={channel_peers}"));
    push_line(&mut out, format!("update-channel-count={channels}"));
    Ok(out)
}

fn load_json_session(path: &Path) -> Result<SessionData> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read grammers JSON session {}", path.display()))?;
    let raw: RawSessionData =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;

    let mut dc_options = HashMap::new();
    for (key, dc) in raw.dc_options {
        let key_id = key
            .parse::<i32>()
            .with_context(|| format!("invalid dc_options key {key:?}"))?;
        if key_id != dc.id {
            bail!("dc_options key {key_id} does not match dc id {}", dc.id);
        }
        dc_options.insert(dc.id, dc);
    }

    let mut peer_infos = HashMap::new();
    for (_, peer) in raw.peer_infos {
        peer_infos.insert(peer.id(), peer);
    }

    Ok(SessionData {
        home_dc: raw.home_dc,
        dc_options,
        peer_infos,
        updates_state: raw.updates_state,
    })
}

fn import_session(config: &Config, force: bool) -> Result<bool> {
    with_runtime(import_session_async(config, force))
}

async fn import_session_async(config: &Config, force: bool) -> Result<bool> {
    if config.sqlite_session.exists() && !force {
        return Ok(false);
    }

    if let Some(parent) = config.sqlite_session.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod {}", parent.display()))?;
    }
    if force {
        let _ = fs::remove_file(&config.sqlite_session);
    }

    let data = load_json_session(&config.source_session)?;
    let session = SqliteSession::open(&config.sqlite_session)
        .await
        .with_context(|| format!("open {}", config.sqlite_session.display()))?;
    data.import_to(&session).await?;

    fs::set_permissions(&config.sqlite_session, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod {}", config.sqlite_session.display()))?;
    Ok(true)
}

async fn prepare_sqlite_session(
    config: &Config,
    options: OnlineOptions,
) -> Result<Arc<SqliteSession>> {
    if !config.sqlite_session.exists() {
        if !options.import_if_missing {
            bail!(
                "sqlite session missing: {}",
                config.sqlite_session.display()
            );
        }
        import_session_async(config, false).await?;
    }
    let session = SqliteSession::open(&config.sqlite_session)
        .await
        .with_context(|| format!("open {}", config.sqlite_session.display()))?;
    Ok(Arc::new(session))
}

async fn with_client<T, F, Fut>(config: &Config, options: OnlineOptions, f: F) -> Result<T>
where
    F: FnOnce(Client, Arc<SqliteSession>) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let session = prepare_sqlite_session(config, options).await?;
    let SenderPool { runner, handle, .. } = SenderPool::new(Arc::clone(&session), config.api_id);
    let client = Client::new(handle);
    let runner_task = tokio::spawn(runner.run());

    let result =
        tokio::time::timeout(options.timeout, f(client.clone(), Arc::clone(&session))).await;
    drop(client);
    runner_task.abort();
    let _ = runner_task.await;

    match result {
        Ok(inner) => inner,
        Err(_) => bail!(
            "Telegram request timed out after {}s",
            options.timeout.as_secs()
        ),
    }
}

async fn online_status(config: &Config, options: OnlineOptions) -> Result<()> {
    with_client(config, options, |client, _session| async move {
        let authorized = client.is_authorized().await?;
        println!("Mag Telegram grammers status");
        println!("backend=grammers");
        println!("authorized={}", yesno(authorized));
        println!("sqlite-session={}", DEFAULT_SQLITE_SESSION);
        Ok(())
    })
    .await
}

async fn online_status_text(config: &Config, client: &Client) -> Result<String> {
    let authorized = client.is_authorized().await?;
    let mut out = String::new();
    push_line(&mut out, "Mag Telegram grammers status");
    push_line(&mut out, "backend=grammers");
    push_line(&mut out, format!("authorized={}", yesno(authorized)));
    push_line(
        &mut out,
        format!("sqlite-session={}", config.sqlite_session.display()),
    );
    Ok(out)
}

async fn bridge_status_text(config: &Config, client: &Client) -> Result<String> {
    let authorized = client.is_authorized().await?;
    let mut out = String::new();
    push_line(&mut out, "Mag Telegram bridge");
    push_line(&mut out, "backend=grammers");
    push_line(
        &mut out,
        format!("auth={}", if authorized { "ready" } else { "wait-session" }),
    );
    push_line(
        &mut out,
        format!(
            "auth-action={}",
            if authorized { "none" } else { "configure" }
        ),
    );
    push_line(
        &mut out,
        format!(
            "auth-hint={}",
            if authorized {
                "Authorized through grammers session."
            } else {
                "Import or refresh the grammers session."
            }
        ),
    );
    push_line(&mut out, format!("live={}", yesno(authorized)));
    push_line(&mut out, "chats=ondemand");
    push_line(&mut out, "messages=ondemand");
    push_line(&mut out, format!("config={}", config.config_path.display()));
    push_line(
        &mut out,
        format!("data-dir={}", config.sqlite_session.display()),
    );
    push_line(&mut out, "tdlib-library=not-used");
    push_line(&mut out, "last-error=none");
    Ok(out)
}

async fn print_bridge_status(config: &Config, options: OnlineOptions) -> Result<()> {
    with_client(config, options, |client, _session| async move {
        print!("{}", bridge_status_text(config, &client).await?);
        Ok(())
    })
    .await
}

async fn auth_status_text(client: &Client) -> Result<String> {
    let authorized = client.is_authorized().await?;
    let mut out = String::new();
    push_line(&mut out, "Mag Telegram auth");
    push_line(
        &mut out,
        format!(
            "state={}",
            if authorized { "ready" } else { "wait-session" }
        ),
    );
    push_line(
        &mut out,
        format!("action={}", if authorized { "none" } else { "configure" }),
    );
    push_line(
        &mut out,
        format!(
            "hint={}",
            if authorized {
                "Authorized through grammers session."
            } else {
                "Import or refresh the grammers session."
            }
        ),
    );
    push_line(&mut out, "last-error=none");
    Ok(out)
}

async fn print_auth_status(config: &Config, options: OnlineOptions) -> Result<()> {
    with_client(config, options, |client, _session| async move {
        print!("{}", auth_status_text(&client).await?);
        Ok(())
    })
    .await
}

async fn collect_dialog_rows_with_client(
    client: &Client,
    limit: usize,
    show_names: bool,
) -> Result<(usize, Vec<DialogRow>)> {
    if !client.is_authorized().await? {
        bail!("session is not authorized");
    }
    let mut dialogs = client.iter_dialogs();
    let total = dialogs.total().await?;
    let mut rows = Vec::new();
    while rows.len() < limit {
        let Some(dialog) = dialogs.next().await? else {
            break;
        };
        let peer = dialog.peer();
        let id = peer.id().bot_api_dialog_id().unwrap_or(0);
        let preview = dialog.last_message.as_ref().map(|message| {
            let sender = sender_label_for_message(message, show_names);
            let body = message_body_for_display(message, show_names, 220);
            format!("{sender}: {body}")
        });
        rows.push(DialogRow {
            id,
            kind: peer_kind_name(peer.id().kind()),
            name: display_name(peer.name(), show_names),
            unread_count: dialog.unread_count(),
            preview,
        });
    }
    Ok((total, rows))
}

async fn collect_dialog_rows(
    config: &Config,
    limit: usize,
    show_names: bool,
    options: OnlineOptions,
) -> Result<(usize, Vec<DialogRow>)> {
    with_client(config, options, move |client, _session| async move {
        collect_dialog_rows_with_client(&client, limit, show_names).await
    })
    .await
}

impl DaemonState {
    fn new() -> Self {
        Self {
            dialogs_total: 0,
            dialogs: Vec::new(),
            dialogs_show_names: true,
            dialogs_loaded_at: None,
            message_pages: Vec::new(),
        }
    }

    fn dialogs_fresh_for(&self, min_rows: usize, show_names: bool) -> bool {
        self.dialogs_show_names == show_names
            && self.dialogs.len() >= min_rows
            && self
                .dialogs_loaded_at
                .map(|loaded| loaded.elapsed() < Duration::from_secs(DIALOG_CACHE_TTL_SECONDS))
                .unwrap_or(false)
    }

    fn clear_messages_for(&mut self, peer_dialog_id: i64) {
        self.message_pages
            .retain(|page| page.peer_dialog_id != peer_dialog_id);
    }
}

async fn ensure_dialog_cache(
    client: &Client,
    state: &mut DaemonState,
    min_rows: usize,
    show_names: bool,
    force: bool,
) -> Result<()> {
    if !force && state.dialogs_fresh_for(min_rows, show_names) {
        return Ok(());
    }
    let fetch_rows = min_rows.max(60);
    let (total, rows) = collect_dialog_rows_with_client(client, fetch_rows, show_names).await?;
    state.dialogs_total = total;
    state.dialogs = rows;
    state.dialogs_show_names = show_names;
    state.dialogs_loaded_at = Some(Instant::now());
    Ok(())
}

fn dialog_line(row: &DialogRow) -> String {
    let badge = if row.unread_count > 0 {
        format!("[{}]", row.unread_count)
    } else {
        "[ ]".to_string()
    };
    let mut line = format!("{badge} {} [{}] {}", row.id, row.kind, row.name);
    if let Some(preview) = &row.preview {
        if !preview.is_empty() {
            line.push_str(" :: ");
            line.push_str(preview);
        }
    }
    line
}

async fn cached_chats_bridge_text(
    client: &Client,
    state: &mut DaemonState,
    limit: usize,
    show_names: bool,
    force: bool,
) -> Result<String> {
    ensure_dialog_cache(client, state, limit, show_names, force).await?;
    let mut out = String::new();
    push_line(
        &mut out,
        format!("Mag Telegram chats ({})", state.dialogs_total),
    );
    for row in state.dialogs.iter().take(limit) {
        push_line(&mut out, dialog_line(row));
    }
    Ok(out)
}

async fn print_chats_bridge(
    config: &Config,
    limit: usize,
    show_names: bool,
    options: OnlineOptions,
) -> Result<()> {
    let (total, rows) = collect_dialog_rows(config, limit, show_names, options).await?;
    println!("Mag Telegram chats ({total})");
    for row in &rows {
        println!("{}", dialog_line(row));
    }
    Ok(())
}

async fn cached_chats_view_text(
    client: &Client,
    state: &mut DaemonState,
    selected: isize,
    top: isize,
    visible: usize,
    show_names: bool,
    force: bool,
) -> Result<String> {
    let visible = visible.max(1);
    let initial_selected = selected.max(0) as usize;
    let initial_top = top.max(0) as usize;
    let needed = initial_top
        .saturating_add(visible)
        .max(initial_selected.saturating_add(1));
    ensure_dialog_cache(client, state, needed, show_names, force).await?;

    let count = state.dialogs_total;
    let selected = if count > 0 {
        initial_selected.min(count - 1)
    } else {
        0
    };
    let mut top = initial_top;
    if count == 0 {
        top = 0;
    } else if selected < top {
        top = selected;
    } else if selected >= top.saturating_add(visible) {
        top = selected.saturating_sub(visible - 1);
    }
    if top.saturating_add(visible) > count {
        top = count.saturating_sub(visible);
    }
    let end = top
        .saturating_add(visible)
        .min(state.dialogs.len())
        .min(count);

    let mut out = String::new();
    push_line(&mut out, format!("Mag Telegram chats ({count})"));
    push_line(
        &mut out,
        format!("selected={selected} top={top} count={count} visible={visible}"),
    );
    push_line(
        &mut out,
        "arrows: select, Enter/right: open, left: dashboard, r: reload, s: start, q: close",
    );
    out.push('\n');
    if count == 0 {
        push_line(
            &mut out,
            "No chats cached yet. Start/auth the bridge, then refresh.",
        );
        return Ok(out);
    }
    for index in top..end {
        let prefix = if index == selected { "> " } else { "  " };
        push_line(
            &mut out,
            format!("{prefix}{}", dialog_line(&state.dialogs[index])),
        );
    }
    Ok(out)
}

async fn print_chats_view(
    config: &Config,
    selected: isize,
    top: isize,
    visible: usize,
    show_names: bool,
    options: OnlineOptions,
) -> Result<()> {
    let visible = visible.max(1);
    let initial_selected = selected.max(0) as usize;
    let initial_top = top.max(0) as usize;
    let needed = initial_top
        .saturating_add(visible)
        .max(initial_selected.saturating_add(1));
    let (total, rows) = collect_dialog_rows(config, needed, show_names, options).await?;
    let count = total;
    let selected = if count > 0 {
        initial_selected.min(count - 1)
    } else {
        0
    };
    let mut top = initial_top;
    if count == 0 {
        top = 0;
    } else if selected < top {
        top = selected;
    } else if selected >= top.saturating_add(visible) {
        top = selected.saturating_sub(visible - 1);
    }
    if top.saturating_add(visible) > count {
        top = count.saturating_sub(visible);
    }
    let end = top.saturating_add(visible).min(rows.len()).min(count);

    println!("Mag Telegram chats ({count})");
    println!("selected={selected} top={top} count={count} visible={visible}");
    println!("arrows: select, Enter/right: open, left: dashboard, r: reload, s: start, q: close");
    println!();
    if count == 0 {
        println!("No chats cached yet. Start/auth the bridge, then refresh.");
        return Ok(());
    }
    for index in top..end {
        let prefix = if index == selected { "> " } else { "  " };
        println!("{prefix}{}", dialog_line(&rows[index]));
    }
    Ok(())
}

async fn cached_chat_at_text(
    client: &Client,
    state: &mut DaemonState,
    selected: isize,
    show_names: bool,
    force: bool,
) -> Result<String> {
    let index = selected.max(0) as usize;
    ensure_dialog_cache(client, state, index.saturating_add(1), show_names, force).await?;
    let mut out = String::new();
    if state.dialogs_total == 0 || index >= state.dialogs.len() {
        push_line(&mut out, "chat-id=0");
        push_line(&mut out, "status=no-chats");
        return Ok(out);
    }
    push_line(&mut out, format!("chat-id={}", state.dialogs[index].id));
    push_line(&mut out, format!("index={index}"));
    push_line(
        &mut out,
        format!("line={}", dialog_line(&state.dialogs[index])),
    );
    Ok(out)
}

async fn print_chat_at(
    config: &Config,
    selected: isize,
    show_names: bool,
    options: OnlineOptions,
) -> Result<()> {
    let index = selected.max(0) as usize;
    let (total, rows) =
        collect_dialog_rows(config, index.saturating_add(1), show_names, options).await?;
    if total == 0 || index >= rows.len() {
        println!("chat-id=0");
        println!("status=no-chats");
        return Ok(());
    }
    println!("chat-id={}", rows[index].id);
    println!("index={index}");
    println!("line={}", dialog_line(&rows[index]));
    Ok(())
}

async fn print_chats(
    config: &Config,
    limit: usize,
    show_names: bool,
    options: OnlineOptions,
) -> Result<()> {
    with_client(config, options, |client, _session| async move {
        if !client.is_authorized().await? {
            bail!("session is not authorized");
        }
        let mut dialogs = client.iter_dialogs();
        let total = dialogs.total().await?;
        println!("Mag Telegram grammers chats");
        println!("total={total}");
        println!("limit={limit}");
        let mut index = 0usize;
        while index < limit {
            let Some(dialog) = dialogs.next().await? else {
                break;
            };
            let peer = dialog.peer();
            let id = peer.id();
            println!(
                "{} {} [{}] {}",
                index,
                display_dialog_id(id),
                peer_kind_name(id.kind()),
                display_name(peer.name(), show_names)
            );
            index += 1;
        }
        Ok(())
    })
    .await
}

async fn cached_messages_page_text(
    client: &Client,
    session: &Arc<SqliteSession>,
    state: &mut DaemonState,
    peer_dialog_id: i64,
    from_message_id: i64,
    limit: usize,
    show_names: bool,
    force: bool,
) -> Result<String> {
    let limit = limit.max(1);
    if !force {
        if let Some(page) = state.message_pages.iter().find(|page| {
            page.peer_dialog_id == peer_dialog_id
                && page.from_message_id == from_message_id
                && page.limit == limit
                && page.show_names == show_names
                && page.loaded_at.elapsed() < Duration::from_secs(MESSAGE_CACHE_TTL_SECONDS)
        }) {
            return Ok(message_page_output(page));
        }
    }

    if !client.is_authorized().await? {
        bail!("session is not authorized");
    }
    let peer_id = peer_id_from_dialog_id(peer_dialog_id)?;
    let peer_ref = session
        .peer_ref(peer_id)
        .await?
        .ok_or_else(|| anyhow!("peer not found in session cache: {peer_dialog_id}"))?;
    let mut messages = client.iter_messages(peer_ref).limit(limit);
    if from_message_id > 0 {
        messages = messages.offset_id(message_id_i32(from_message_id));
    }
    let mut rows = Vec::new();
    while let Some(message) = messages.next().await? {
        rows.push(MessageRow {
            id: message.id(),
            sender: sender_label_for_message(&message, show_names),
            body: message_body_for_display(&message, show_names, 512),
        });
    }
    rows.reverse();
    let oldest_id = rows.first().map(|row| row.id).unwrap_or(0);
    let newest_id = rows.last().map(|row| row.id).unwrap_or(0);
    if from_message_id == 0 {
        let _ = client.mark_as_read(peer_ref).await;
    }

    let page = MessagePage {
        peer_dialog_id,
        from_message_id,
        limit,
        show_names,
        rows,
        oldest_id,
        newest_id,
        loaded_at: Instant::now(),
    };
    let out = message_page_output(&page);
    state.message_pages.push(page);
    if state.message_pages.len() > MESSAGE_CACHE_MAX_PAGES {
        let remove_count = state.message_pages.len() - MESSAGE_CACHE_MAX_PAGES;
        state.message_pages.drain(0..remove_count);
    }
    Ok(out)
}

fn message_page_output(page: &MessagePage) -> String {
    let mut out = String::new();
    push_line(
        &mut out,
        format!("Mag Telegram messages chat={}", page.peer_dialog_id),
    );
    push_line(
        &mut out,
        format!(
            "page={} cached-total={} page-count={} oldest-id={} newest-id={}",
            if page.from_message_id > 0 {
                "older"
            } else {
                "latest"
            },
            page.rows.len(),
            page.rows.len(),
            page.oldest_id,
            page.newest_id
        ),
    );
    for row in &page.rows {
        push_line(
            &mut out,
            format!("{}: {}", row.sender, row.body),
        );
    }
    if page.rows.is_empty() {
        push_line(&mut out, "No cached text messages for this chat/page yet.");
    }
    out
}

async fn print_messages_page(
    config: &Config,
    peer_dialog_id: i64,
    from_message_id: i64,
    limit: usize,
    show_names: bool,
    options: OnlineOptions,
) -> Result<()> {
    with_client(config, options, move |client, session| async move {
        if !client.is_authorized().await? {
            bail!("session is not authorized");
        }
        let peer_id = peer_id_from_dialog_id(peer_dialog_id)?;
        let peer_ref = session
            .peer_ref(peer_id)
            .await?
            .ok_or_else(|| anyhow!("peer not found in session cache: {peer_dialog_id}"))?;
        let mut messages = client.iter_messages(peer_ref).limit(limit.max(1));
        if from_message_id > 0 {
            messages = messages.offset_id(message_id_i32(from_message_id));
        }
        let mut rows: Vec<(i32, String, String)> = Vec::new();
        while let Some(message) = messages.next().await? {
            rows.push((
                message.id(),
                sender_label_for_message(&message, show_names),
                message_body_for_display(&message, show_names, 512),
            ));
        }
        rows.reverse();
        let oldest_id = rows.first().map(|row| row.0).unwrap_or(0);
        let newest_id = rows.last().map(|row| row.0).unwrap_or(0);
        if from_message_id == 0 {
            let _ = client.mark_as_read(peer_ref).await;
        }

        println!("Mag Telegram messages chat={peer_dialog_id}");
        println!(
            "page={} cached-total={} page-count={} oldest-id={} newest-id={}",
            if from_message_id > 0 {
                "older"
            } else {
                "latest"
            },
            rows.len(),
            rows.len(),
            oldest_id,
            newest_id
        );
        for (id, sender, body) in &rows {
            let _ = id;
            println!("{sender}: {body}");
        }
        if rows.is_empty() {
            println!("No cached text messages for this chat/page yet.");
        }
        Ok(())
    })
    .await
}

async fn print_messages(
    config: &Config,
    peer_dialog_id: i64,
    limit: usize,
    show_names: bool,
    options: OnlineOptions,
) -> Result<()> {
    with_client(config, options, move |client, session| async move {
        if !client.is_authorized().await? {
            bail!("session is not authorized");
        }
        let peer_id = peer_id_from_dialog_id(peer_dialog_id)?;
        let peer_ref = session
            .peer_ref(peer_id)
            .await?
            .ok_or_else(|| anyhow!("peer not found in session cache: {peer_dialog_id}"))?;
        let mut messages = client.iter_messages(peer_ref).limit(limit);
        println!("Mag Telegram grammers messages");
        println!("peer={peer_dialog_id}");
        println!("limit={limit}");
        while let Some(message) = messages.next().await? {
            let sender = message
                .sender()
                .and_then(|peer| peer.name())
                .map(|s| display_name(Some(s), show_names))
                .unwrap_or_else(|| "?".to_string());
            let body = if show_names {
                sanitize_line(message.text(), 512)
            } else {
                "(redacted)".to_string()
            };
            println!("{} {} {}", message.id(), sender, body);
        }
        Ok(())
    })
    .await
}

async fn mark_read_text(
    client: &Client,
    session: &Arc<SqliteSession>,
    peer_dialog_id: i64,
    message_id: i64,
) -> Result<String> {
    if !client.is_authorized().await? {
        bail!("session is not authorized");
    }
    let peer_id = peer_id_from_dialog_id(peer_dialog_id)?;
    let peer_ref = session
        .peer_ref(peer_id)
        .await?
        .ok_or_else(|| anyhow!("peer not found in session cache: {peer_dialog_id}"))?;
    client.mark_as_read(peer_ref).await?;
    let mut out = String::new();
    push_line(
        &mut out,
        format!("marked read chat={peer_dialog_id} message={message_id}"),
    );
    Ok(out)
}

async fn mark_read(
    config: &Config,
    peer_dialog_id: i64,
    message_id: i64,
    options: OnlineOptions,
) -> Result<()> {
    with_client(config, options, move |client, session| async move {
        if !client.is_authorized().await? {
            bail!("session is not authorized");
        }
        let peer_id = peer_id_from_dialog_id(peer_dialog_id)?;
        let peer_ref = session
            .peer_ref(peer_id)
            .await?
            .ok_or_else(|| anyhow!("peer not found in session cache: {peer_dialog_id}"))?;
        client.mark_as_read(peer_ref).await?;
        println!("marked read chat={peer_dialog_id} message={message_id}");
        Ok(())
    })
    .await
}

async fn send_message_text(
    client: &Client,
    session: &Arc<SqliteSession>,
    peer_dialog_id: i64,
    text: &str,
) -> Result<String> {
    if !client.is_authorized().await? {
        bail!("session is not authorized");
    }
    let peer_id = peer_id_from_dialog_id(peer_dialog_id)?;
    let peer_ref = session
        .peer_ref(peer_id)
        .await?
        .ok_or_else(|| anyhow!("peer not found in session cache: {peer_dialog_id}"))?;
    let sent = client.send_message(peer_ref, text.to_string()).await?;
    let mut out = String::new();
    push_line(&mut out, format!("sent id={}", sent.id()));
    Ok(out)
}

async fn send_message(
    config: &Config,
    peer_dialog_id: i64,
    text: &str,
    options: OnlineOptions,
) -> Result<()> {
    let text = text.to_string();
    with_client(config, options, move |client, session| async move {
        if !client.is_authorized().await? {
            bail!("session is not authorized");
        }
        let peer_id = peer_id_from_dialog_id(peer_dialog_id)?;
        let peer_ref = session
            .peer_ref(peer_id)
            .await?
            .ok_or_else(|| anyhow!("peer not found in session cache: {peer_dialog_id}"))?;
        let sent = client.send_message(peer_ref, text).await?;
        println!("sent id={}", sent.id());
        Ok(())
    })
    .await
}

fn handle_request_file(config: &Config, path: &Path) -> Result<()> {
    let request = fs::read_to_string(path)
        .with_context(|| format!("read request file {}", path.display()))?;
    handle_request_text(config, request.trim())
}

fn handle_request_text(config: &Config, request: &str) -> Result<()> {
    let mut parts = request.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or_default();
    let arg = parts.next().unwrap_or_default().trim();

    match command {
        "status" => with_runtime(print_bridge_status(config, OnlineOptions::default())),
        "doctor" => print_doctor(config),
        "session-info" => print_session_info(&config.source_session),
        "import-session" => {
            let imported = import_session(config, false)?;
            println!("imported={}", yesno(imported));
            Ok(())
        }
        "auth-status" => with_runtime(print_auth_status(config, OnlineOptions::default())),
        "online-status" => with_runtime(online_status(config, OnlineOptions::default())),
        "chats" => with_runtime(print_chats_bridge(
            config,
            60,
            true,
            OnlineOptions::default(),
        )),
        "chats-view" => {
            let mut bits = arg.split_whitespace();
            let selected = bits
                .next()
                .and_then(|s| s.parse::<isize>().ok())
                .unwrap_or(0);
            let top = bits
                .next()
                .and_then(|s| s.parse::<isize>().ok())
                .unwrap_or(0);
            let visible = bits
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(20);
            with_runtime(print_chats_view(
                config,
                selected,
                top,
                visible,
                true,
                OnlineOptions::default(),
            ))
        }
        "chat-at" => {
            let selected = arg.parse::<isize>().unwrap_or(0);
            with_runtime(print_chat_at(
                config,
                selected,
                true,
                OnlineOptions::default(),
            ))
        }
        "messages" => {
            let mut bits = arg.split_whitespace();
            let peer = bits
                .next()
                .ok_or_else(|| anyhow!("messages needs peer dialog id"))?
                .parse::<i64>()
                .context("messages peer must be an integer")?;
            let from_message_id = bits.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            let limit = bits
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(30);
            with_runtime(print_messages_page(
                config,
                peer,
                from_message_id,
                limit,
                true,
                OnlineOptions::default(),
            ))
        }
        "older" => {
            let mut bits = arg.split_whitespace();
            let peer = bits
                .next()
                .ok_or_else(|| anyhow!("older needs peer dialog id"))?
                .parse::<i64>()
                .context("older peer must be an integer")?;
            let from_message_id = bits
                .next()
                .ok_or_else(|| anyhow!("older needs from message id"))?
                .parse::<i64>()
                .context("older from message id must be an integer")?;
            let limit = bits
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(30);
            with_runtime(print_messages_page(
                config,
                peer,
                from_message_id,
                limit,
                true,
                OnlineOptions::default(),
            ))
        }
        "send" => {
            let mut bits = arg.splitn(2, char::is_whitespace);
            let peer = bits
                .next()
                .ok_or_else(|| anyhow!("send needs peer dialog id"))?
                .parse::<i64>()
                .context("send peer must be an integer")?;
            let text = bits.next().unwrap_or_default().trim();
            if text.is_empty() {
                bail!("send needs message text");
            }
            with_runtime(send_message(config, peer, text, OnlineOptions::default()))
        }
        "mark-read" => {
            let mut bits = arg.split_whitespace();
            let peer = bits
                .next()
                .ok_or_else(|| anyhow!("mark-read needs peer dialog id"))?
                .parse::<i64>()
                .context("mark-read peer must be an integer")?;
            let message_id = bits.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            with_runtime(mark_read(
                config,
                peer,
                message_id,
                OnlineOptions::default(),
            ))
        }
        "auth-phone" | "auth-code" | "auth-password" | "auth-register" => {
            println!("Mag Telegram auth");
            println!("state=ready");
            println!("action=none");
            println!("hint=Already authorized through grammers session.");
            println!("last-error=none");
            Ok(())
        }
        "" => {
            println!("empty request");
            Ok(())
        }
        other => {
            println!("unknown request: {other}");
            Ok(())
        }
    }
}

async fn run_daemon(config: &Config) -> Result<()> {
    if daemon_already_running()? {
        eprintln!("mag-telegram-grammers daemon already running");
        return Ok(());
    }
    write_pid_file()?;
    let _ = fs::remove_file(DEFAULT_DAEMON_REQUEST);
    let _ = fs::remove_file(DEFAULT_DAEMON_RESPONSE);

    let session = prepare_sqlite_session(config, OnlineOptions::default()).await?;
    let SenderPool { runner, handle, .. } = SenderPool::new(Arc::clone(&session), config.api_id);
    let client = Client::new(handle);
    let runner_task = tokio::spawn(runner.run());

    if !client.is_authorized().await? {
        let _ = fs::remove_file(DEFAULT_DAEMON_PID);
        runner_task.abort();
        let _ = runner_task.await;
        bail!("grammers session is not authorized");
    }
    eprintln!(
        "mag-telegram-grammers daemon backend=grammers auth=ready request={DEFAULT_DAEMON_REQUEST}"
    );
    let mut state = DaemonState::new();

    loop {
        if let Some(request) = take_daemon_request()? {
            let response = tokio::time::timeout(
                Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
                handle_daemon_request_text(config, &client, &session, &mut state, request.trim()),
            )
            .await;
            let (text, stop) = match response {
                Ok(Ok(result)) => result,
                Ok(Err(err)) => (format!("error: {err}\n"), false),
                Err(_) => (
                    format!("error: Telegram request timed out after {DEFAULT_TIMEOUT_SECONDS}s\n"),
                    false,
                ),
            };
            write_text_file_atomic(Path::new(DEFAULT_DAEMON_RESPONSE), &text)?;
            if stop {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(DAEMON_LOOP_SLEEP_MS)).await;
    }

    let _ = fs::remove_file(DEFAULT_DAEMON_PID);
    let _ = fs::remove_file(DEFAULT_DAEMON_REQUEST);
    drop(client);
    runner_task.abort();
    let _ = runner_task.await;
    Ok(())
}

async fn handle_daemon_request_text(
    config: &Config,
    client: &Client,
    session: &Arc<SqliteSession>,
    state: &mut DaemonState,
    request: &str,
) -> Result<(String, bool)> {
    let mut parts = request.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or_default();
    let arg = parts.next().unwrap_or_default().trim();

    let text = match command {
        "status" => bridge_status_text(config, client).await?,
        "doctor" => doctor_text(config)?,
        "session-info" => session_info_text(&config.source_session)?,
        "import-session" => {
            "import-session unavailable in daemon mode; stop and restart the daemon after importing\n"
                .to_string()
        }
        "auth-status" => auth_status_text(client).await?,
        "online-status" => online_status_text(config, client).await?,
        "chats" => cached_chats_bridge_text(client, state, 60, true, false).await?,
        "chats-view" => {
            let mut bits = arg.split_whitespace();
            let selected = bits
                .next()
                .and_then(|s| s.parse::<isize>().ok())
                .unwrap_or(0);
            let top = bits
                .next()
                .and_then(|s| s.parse::<isize>().ok())
                .unwrap_or(0);
            let visible = bits
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(20);
            cached_chats_view_text(client, state, selected, top, visible, true, false).await?
        }
        "chat-at" => {
            let selected = arg.parse::<isize>().unwrap_or(0);
            cached_chat_at_text(client, state, selected, true, false).await?
        }
        "messages" => {
            let mut bits = arg.split_whitespace();
            let peer = bits
                .next()
                .ok_or_else(|| anyhow!("messages needs peer dialog id"))?
                .parse::<i64>()
                .context("messages peer must be an integer")?;
            let from_message_id = bits.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            let limit = bits
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(30);
            cached_messages_page_text(client, session, state, peer, from_message_id, limit, true, false).await?
        }
        "older" => {
            let mut bits = arg.split_whitespace();
            let peer = bits
                .next()
                .ok_or_else(|| anyhow!("older needs peer dialog id"))?
                .parse::<i64>()
                .context("older peer must be an integer")?;
            let from_message_id = bits
                .next()
                .ok_or_else(|| anyhow!("older needs from message id"))?
                .parse::<i64>()
                .context("older from message id must be an integer")?;
            let limit = bits
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(30);
            cached_messages_page_text(client, session, state, peer, from_message_id, limit, true, false).await?
        }
        "send" => {
            let mut bits = arg.splitn(2, char::is_whitespace);
            let peer = bits
                .next()
                .ok_or_else(|| anyhow!("send needs peer dialog id"))?
                .parse::<i64>()
                .context("send peer must be an integer")?;
            let text = bits.next().unwrap_or_default().trim();
            if text.is_empty() {
                bail!("send needs message text");
            }
            let result = send_message_text(client, session, peer, text).await?;
            state.clear_messages_for(peer);
            state.dialogs_loaded_at = None;
            result
        }
        "mark-read" => {
            let mut bits = arg.split_whitespace();
            let peer = bits
                .next()
                .ok_or_else(|| anyhow!("mark-read needs peer dialog id"))?
                .parse::<i64>()
                .context("mark-read peer must be an integer")?;
            let message_id = bits.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            mark_read_text(client, session, peer, message_id).await?
        }
        "auth-phone" | "auth-code" | "auth-password" | "auth-register" => {
            "Mag Telegram auth\nstate=ready\naction=none\nhint=Already authorized through grammers session.\nlast-error=none\n"
                .to_string()
        }
        "quit" => "stopping mag-telegram-grammers\n".to_string(),
        "" => "empty request\n".to_string(),
        other => format!("unknown request: {other}\n"),
    };

    Ok((text, command == "quit"))
}

fn take_daemon_request() -> Result<Option<String>> {
    let path = Path::new(DEFAULT_DAEMON_REQUEST);
    if !path.exists() {
        return Ok(None);
    }
    let request = fs::read_to_string(path)
        .with_context(|| format!("read daemon request {}", path.display()))?;
    let _ = fs::remove_file(path);
    Ok(Some(request))
}

fn daemon_already_running() -> Result<bool> {
    let path = Path::new(DEFAULT_DAEMON_PID);
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path).unwrap_or_default();
    let pid = text.trim().parse::<u32>().unwrap_or(0);
    if pid > 0 && Path::new(&format!("/proc/{pid}")).exists() {
        return Ok(true);
    }
    let _ = fs::remove_file(path);
    Ok(false)
}

fn write_pid_file() -> Result<()> {
    write_text_file_atomic(
        Path::new(DEFAULT_DAEMON_PID),
        &format!("{}\n", std::process::id()),
    )
}

fn write_text_file_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} to {}", tmp.display(), path.display()))
}

fn peer_id_from_dialog_id(id: i64) -> Result<PeerId> {
    if id > 0 {
        PeerId::user(id).ok_or_else(|| anyhow!("invalid user dialog id {id}"))
    } else if id <= -1_000_000_000_001 {
        let bare = -id - 1_000_000_000_000;
        PeerId::channel(bare).ok_or_else(|| anyhow!("invalid channel dialog id {id}"))
    } else if id < 0 {
        PeerId::chat(-id).ok_or_else(|| anyhow!("invalid chat dialog id {id}"))
    } else {
        bail!("invalid peer dialog id 0")
    }
}

fn peer_kind_name(kind: PeerKind) -> &'static str {
    match kind {
        PeerKind::User => "user",
        PeerKind::Chat => "chat",
        PeerKind::Channel => "channel",
    }
}

fn display_dialog_id(id: PeerId) -> String {
    id.bot_api_dialog_id()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn sender_label_for_message(message: &Message, show_names: bool) -> String {
    if message.outgoing() {
        return "me".to_string();
    }
    if let Some(peer) = message.sender() {
        return display_name(peer.name(), show_names);
    }
    if let Some(id) = message
        .sender_id()
        .and_then(|peer_id| peer_id.bot_api_dialog_id())
    {
        return id.to_string();
    }
    "system".to_string()
}

fn message_body_for_display(message: &Message, show_names: bool, max_chars: usize) -> String {
    if !show_names {
        return "(redacted)".to_string();
    }
    let text = message.text();
    if text.trim().is_empty() {
        "[non-text message]".to_string()
    } else {
        sanitize_line(text, max_chars)
    }
}

fn message_id_i32(id: i64) -> i32 {
    if id <= 0 {
        0
    } else if id > i32::MAX as i64 {
        i32::MAX
    } else {
        id as i32
    }
}

fn display_name(name: Option<&str>, show_names: bool) -> String {
    if show_names {
        sanitize_line(name.unwrap_or("(unnamed)"), 120)
    } else if name.is_some() {
        "(redacted)".to_string()
    } else {
        "(unnamed)".to_string()
    }
}

fn sanitize_line(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        let mapped = match ch {
            '\r' | '\n' | '\t' => ' ',
            ch if ch.is_control() => ' ',
            ch => ch,
        };
        out.push(mapped);
        if out.chars().count() >= max_chars {
            out.push_str("...");
            break;
        }
    }
    out
}

fn push_line(out: &mut String, line: impl AsRef<str>) {
    out.push_str(line.as_ref());
    out.push('\n');
}

fn yesno(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
