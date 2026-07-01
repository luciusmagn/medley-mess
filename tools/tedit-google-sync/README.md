# tedit-google-sync

`tedit-google-sync` publishes one rendered TEdit Markdown document to Google
Docs without treating Google Docs as the source of truth.

Current target on this machine:

- source Markdown: `/home/mag/docs/md/bitcoin.md`
- Google title: `Bitcoin`
- local config: `~/.config/tedit-google-sync/bitcoin.json`
- local state: `~/.local/state/tedit-google-sync/bitcoin.state.json`

The tool intentionally targets one explicit Markdown file. It does not glob
`/home/mag/docs/md`, because Interlisp/TEdit version files such as `.~1~` must
not create duplicate Google Docs or unexpectedly replace the book document.

## Review Safety Policy

The Google Doc is a review surface only. TEdit remains the authoring source.

The syncer never accepts Google Docs edits back into TEdit. It also avoids the
unsafe delete-and-recreate workflow after review begins:

- with no unresolved comments or suggestions, it may fully republish the body
- with unresolved comments/suggestions, it refuses structural rewrites
- changed blocks with unresolved comments are skipped
- unknown/unmapped comments make changed blocks unsafe
- suggestions make the sync refuse by default unless `--allow-suggestions` or
  `--force` is passed

This preserves active review context by not destroying the text ranges reviewers
commented on. If a skipped block matters, process the comment/suggestion by hand
in TEdit, resolve it in Google Docs, rerender Markdown, then sync again.

## Install

```sh
./install.sh
```

The installed command is:

```sh
~/.local/bin/tedit-google-sync
```

On Guix, Rustup binaries need store-library runpaths. This machine has:

```sh
~/.local/bin/rustup-guix-fix
```

Run it after `rustup update` if `cargo` or `rustc` complains about missing
`libgcc_s.so.1` or `libz.so.1`.

## OAuth Setup

Create a Google Cloud OAuth client for a desktop app, enable the Google Docs API
and Google Drive API, then put the downloaded JSON here:

```sh
~/.config/tedit-google-sync/client_secret.json
```

Authorize once:

```sh
tedit-google-sync --auth --open-browser
```

The OAuth token is stored outside the docs git repo:

```sh
~/.local/state/tedit-google-sync/token.json
```

## Dry Run

```sh
tedit-google-sync --dry-run --once
```

This parses the Markdown and prints the source blocks and stable block IDs
without touching Google.

## Publish

```sh
tedit-google-sync --once
```

First publish creates a Google Doc unless `document_id` is already set in the
config. The created document ID is stored in the state file.

To connect to an existing Google Doc, add this to the config:

```json
{
  "document_id": "your-google-doc-id"
}
```

Keep `client_secret.json`, `token.json`, and `bitcoin.state.json` out of the
`/home/mag/docs` git repo.
