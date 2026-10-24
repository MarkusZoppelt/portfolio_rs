<p align="center">
    <img src="https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/logo.png" alt="portfolio_rs logo"><br>
    <img src="https://github.com/markuszoppelt/portfolio_rs/actions/workflows/rust.yml/badge.svg" alt="build status badge">
</p>

Local-first portfolio management for humans and AI agents — written in Rust.

`portfolio_rs` is a CLI + interactive TUI for tracking financial investment
portfolios, and a toolkit for the AI era: machine-readable investment
policies, policy-aware reviews, rebalancing simulations, durable financial
memory (diary, decisions, reports), a portable agent skill, and a local HTTP
API. It is also a Rust library that GUIs (e.g. a desktop app) can embed.

> _This project is the modern successor of [finance](https://github.com/MarkusZoppelt/finance)._

## Installation

Available in [nixpkgs](https://search.nixos.org/packages?query=portfolio_rs): `nix-shell -p portfolio_rs` or `nix run nixpkgs#portfolio_rs`

Install via [pkgx](https://pkgx.sh): `pkgx portfolio_rs`

Install from cargo: `cargo install portfolio_rs`

## Quick Start

### Simple mode: a single positions file

Create a JSON file with your portfolio positions (see
[example data](example_data.json) for the schema), then launch the TUI:

    portfolio_rs my_positions.json

### Workspace mode: a durable finance workspace

For the full experience — policies, diary, decisions, and reports — create a
workspace:

    portfolio_rs init-workspace my-portfolio
    portfolio_rs policy init --strategy balanced-growth my-portfolio
    portfolio_rs context my-portfolio/positions.json

This creates a directory with an `INVESTMENT_POLICY.md` (your financial
constitution), a machine-readable `portfolio/policy.toml`, folders for diary
entries, decision records, theses, and reports, agent instruction files
(`AGENTS.md`, `CLAUDE.md`), and a `.gitignore` that protects your private
data.

## Commands

| Command | Description |
|---|---|
| `portfolio_rs [FILE]` | Interactive TUI (default) |
| `balances [FILE]` | Balances table with PnL |
| `allocation [FILE]` | Allocation pie chart and breakdown |
| `performance [FILE]` | Performance metrics (YTD, total return) |
| `sort [FILE]` | Positions sorted by value (display only) |
| `context [FILE]` | Agent-friendly portfolio briefing (Markdown/JSON) |
| `review [FILE]` | Policy-aware review: drift, violations, actions |
| `simulate [FILE]` | Rebalancing what-if scenarios (never trades) |
| `validate [FILE]` | Validate a portfolio JSON file |
| `policy init/validate` | Create/check a machine-readable `policy.toml` |
| `decision draft` | Draft a structured decision record |
| `report weekly` | Generate a weekly Markdown report |
| `doctor [DIR]` | Workspace health check |
| `init-workspace [DIR]` | Create a new finance workspace |
| `agent init/skill` | Agent instructions + portable skill management |
| `api` | Local HTTP API server |
| `mcp` | Read-only MCP server for AI agents (stdio) |
| `config` | Show config file location |
| `components` | List TUI components for `--disable` |

If no file is specified, commands use the portfolio file (or workspace) from
your config. Run `portfolio_rs <COMMAND> --help` for details.

Quotes come from Yahoo Finance by default. Set `PORTFOLIO_RS_QUOTES` to choose
another source: `offline` makes no network calls (nothing gets a price), and
`fixture:<path>` serves prices from a JSON file with the shape of
[`tests/fixtures/quotes_example.json`](tests/fixtures/quotes_example.json). An
invalid value prints a warning and uses offline quotes; it never falls back to
the network.

    PORTFOLIO_RS_QUOTES=fixture:tests/fixtures/quotes_example.json portfolio_rs context example_data.json

`performance` period returns (1W, 1M, 3M, 6M, YTD, 1Y) are the market return
of the ticker positions you hold now, at current quantities, compared like for
like: cash, ticker-less holdings, unpriced positions, and positions without a
price at the period start are left out of both sides. Deposits, withdrawals,
and trades during the period are not accounted for. 1D and "Since" compare
total portfolio values (including cash).

## AI & Agent Integration

`portfolio_rs` is designed to be operated by coding agents and LLMs, locally:

- **Structured output**: `context`, `review`, and `simulate` support
  `--format json` (camelCase) for scripts and agents, and Markdown for
  humans and LLM prompts.

      portfolio_rs context positions.json --format json
      portfolio_rs review positions.json --policy portfolio/policy.toml --format json

- **Machine-readable policy**: `portfolio/policy.toml` encodes your goals,
  risk profile, target allocations, and constraints. Strategy templates:
  `balanced-growth`, `capital-preservation`, `aggressive-growth`, `custom`.

- **Portable agent skill**: install the built-in `portfolio-rs`
  skill into any agent harness that supports skill files (opencode, Claude
  Code, Cursor, ...):

      portfolio_rs agent skill export ~/.config/opencode/skills

- **Workspace instructions**: `portfolio_rs agent init` creates `AGENTS.md`
  and `CLAUDE.md` with local paths and safety rules (no trades, no broker
  interaction, private data stays local).

- **Durable memory**: decisions and reports are plain Markdown files in your
  workspace — reviewable in six months, greppable forever. Use `--dry-run`
  to preview any file mutation.

- **MCP server**: `portfolio_rs mcp` serves your portfolio read-only to MCP
  clients over stdio (see [MCP server](#mcp-server)).

## MCP server

`portfolio_rs mcp` starts a local, read-only [Model Context Protocol](https://modelcontextprotocol.io)
server on stdio. The MCP client spawns it as a child process; JSON-RPC goes over
stdin/stdout and diagnostics go to stderr. A line that is not JSON gets a
JSON-RPC `-32700` parse error (`id: null`) and the server keeps serving; a
`tools/call` without a `name` gets `-32602`. Wrong argument types and unknown
fields are tool errors with code `invalid_argument`. Only tools and resources
are served (no prompts). With `--offline` or a quote fixture every tool reports
`openWorldHint: false`. It never writes your positions file or the
app config (the only file it writes is the quote cache below), and it has no
network listener.

    portfolio_rs mcp example_data.json                    # one positions file
    portfolio_rs mcp example_data.json --policy policy.toml
    portfolio_rs mcp --workspace ~/finance                # positions.json + portfolio/policy.toml
    portfolio_rs mcp --offline --portfolio demo example_data.json   # portfolio id (default: main)

With no FILE or `--workspace`, it serves the configured workspace (if the last
mode was workspace) or the configured portfolio file. `--policy` has no
default, so nothing is picked up from the current directory.

### Last-known-price cache

Every successful quote fetch is stored in `.<datafile>.quotes-cache.json` next
to the positions file (e.g. `.positions.json.quotes-cache.json`), written
atomically with mode `0600`. It holds, per symbol, the price, previous close,
currency, `asOf` and `fetchedAt`, plus the FX pairs (`EURUSD=X`, ...) in a
separate `fx` map; no quantities or other portfolio data. When a fetch fails,
the cached value is used if it is younger than the max age (default 7 days,
measured from `asOf`): the position gets `priceSource: "cache"`,
`isStale: true`, `priceStatus: "stale"` and the cached `priceAsOf`. Stale prices
count toward `complete`; every response adds `hasStale`, `stalePositions` and
`priceAsOf` (the oldest price used).

    portfolio_rs mcp --cache-max-age 3 example_data.json   # days, 0 disables fallback
    portfolio_rs mcp --no-cache example_data.json          # neither read nor write
    portfolio_rs mcp --offline example_data.json           # read the cache, never fetch

Quote fixtures do not use the cache, and encrypted (`.gpg`) files never get one,
so no plaintext ticker list ends up next to an encrypted portfolio. The cache
pattern is in the workspace `.gitignore` template and checked by `doctor`.

### Client configuration

Most clients (Claude Desktop, Cursor, opencode, ...) take a command, arguments
and environment. A demo setup with the bundled fake data and offline quotes:

```json
{
  "mcpServers": {
    "portfolio": {
      "command": "portfolio_rs",
      "args": ["mcp", "--portfolio", "demo", "/path/to/portfolio_rs/example_data.json"],
      "env": { "PORTFOLIO_RS_QUOTES": "offline" }
    }
  }
}
```

Use absolute paths: clients start servers from an arbitrary working directory.
For deterministic prices without network access, point the env var at a quote
fixture, e.g. `"PORTFOLIO_RS_QUOTES": "fixture:/path/to/portfolio_rs/tests/fixtures/quotes_example.json"`.
Drop the `env` entry to use live quotes (Yahoo Finance).

### Tools

All tools are read-only. Outputs are camelCase JSON with `schemaVersion`,
`portfolioId`, `asOf` (RFC 3339), `baseCurrency` and `warnings`; unknown values
are `null`, never `0`. Every tool accepts an optional `portfolioId`.

Missing quotes or FX rates are never hidden: every response carries
`complete` (false when any position is unpriced) and `dataQuality`
(`missingPrices`, `missingFx`, `affectedPositions`, `affectedClasses`).
Totals then cover only priced positions; a class with an unpriced position has
a `null` value, targeted classes get status `unknown`, `amountToTarget` and
drift findings are suppressed, and `check_policy` adds `checks_incomplete`.
`networkStatus` (in every tool response and in `get_context`, also via `/api/context`) is `connected` (every market position has a fresh quote and FX
rate), `partial` (some fresh quotes, or quotes without FX) or `offline` (no
fresh quotes; cached prices do not count as connected). Each position row
carries `priceSource` (`live`, `cache`, `fixture` or `none`) and `isStale`.

The policy's `single_position_limit_percent` applies to non-cash positions
only, identically in `portfolio_rs review`, `check_policy` and the
`get_context` risk flags (which fall back to 25 % without a policy). Cash is
governed by the cash-buffer and asset-class rules instead.

| Tool | Returns |
| --- | --- |
| `get_positions` | Positions with price status, value in base currency, weight, cost basis and P&L, `currency` per row (base currency for cash), `priceAsOf` (provider timestamp, null when not reported); portfolio-level `invested`, `unrealizedPnl`, `unrealizedPnlPercent`, `dayPnl` over market positions (null if any input is unknown). Filters: `assetClass`, `includeManual`; `sortBy` value/name/pnl/dayChange |
| `get_allocation` | Allocation by asset class against policy targets and bands, drift and amount to target |
| `check_policy` | Policy findings with stable codes and severities, constraint checks, overall status (`ok`/`warnings`/`breach`); `breaches` = critical findings (deduplicated), top-level `warnings` = data quality + warning-severity findings; `cashBuffer` {minimum, actual, surplus} against `minimum_cash_amount` (also in `get_context`); `minSeverity` filter |
| `get_transactions` | Buys and sells synthesized from purchase history; cash entries are `deposit`/`withdrawal` at par (a cash position without entries is one opening deposit with a null date); `fees` is 0 when none is recorded; date, ticker, class and type filters; `limit` (max 500) and `cursor` paging |
| `list_portfolios` | The served portfolio with its `quoteSource` (`yahoo`, `offline` or `fixture`; paths only with `includePaths`) |
| `get_context` | The `portfolio_rs context` report as Markdown or JSON (money and percentages rounded to 2 decimals, as in `/api/context`) |

Errors come back as tool results with `isError: true` and
`{"error": {"code", "message"}}`, codes `unknown_portfolio`,
`invalid_argument` (also an `assetClass` that no position or policy uses) and
`not_found` (an unknown `positionId`). Error and finding messages never contain amounts.

### Resources

| URI | Content |
| --- | --- |
| `portfolio://<id>/context.md` | Portfolio context report (Markdown) |
| `portfolio://<id>/policy` | Parsed investment policy (JSON), when one is loaded |
| `portfolio://<id>/policy.md` | Workspace `INVESTMENT_POLICY.md`, when present |
| `portfolio://schema/positions` | JSON Schema of a position row |
| `portfolio://schema/findings` | Finding schema and the catalog of finding codes |

## HTTP API

Serve your portfolio to local scripts, agents, and GUIs:

    portfolio_rs api positions.json [--policy portfolio/policy.toml] [--host 127.0.0.1] [--port 3000] [--token-file PATH]

| Endpoint | Description |
|---|---|
| `GET /health` | Liveness check |
| `GET /api/portfolio` | Full portfolio summary |
| `GET/POST /api/positions` | List / create positions |
| `GET/PUT/DELETE /api/positions/{id}` | Read / update / delete a position by stable `pos_` id (`positionId`; a numeric index still works) |
| `GET /api/allocation` | Allocation breakdown |
| `GET /api/performance` | Performance metrics |
| `GET /api/context` | Agent briefing (JSON) |
| `GET /api/review` | Policy review (requires `--policy`) |
| `GET /api/simulate` | Rebalance simulation (requires `--policy`) |
| `GET /api/validate` | Portfolio file validation |
| `GET /api/doctor?dir=DIR` | Workspace health check |
| `POST /api/refresh` | Force a live quote refresh |

Notes:

- The API is intended for **local use only**; binding to a non-loopback host
  prints a loud warning. Requests must use `Host: localhost:<port>`,
  `127.0.0.1:<port>` or `[::1]:<port>` (anything else gets `421`), which
  blocks DNS-rebinding attacks from web pages.
- `--token-file PATH` turns on bearer auth: a random token is written to
  `PATH` (mode 0600, never printed or logged) and every `/api/*` request
  needs `Authorization: Bearer $(cat PATH)` (`401` otherwise). `/health`
  stays open.
- Positions carry a stable `positionId` (`pos_…`, generated if missing and
  persisted on the next save); use it in `/api/positions/{id}`. The numeric
  `id` is the array index, kept for older clients.
- Purchase `price` is the recorded price only; prices estimated from
  historic closes are `null`.
- Position mutations are persisted back to the portfolio file — except for
  `.gpg` files, which are never rewritten.
- Live quotes are cached for a short TTL; `POST /api/refresh` bypasses the
  cache.
- Application config is not an API payload. Embedding clients that read
  `AppConfigDto` still never receive the LLM API key: `llmApiKey` is always
  `null`, and `hasLlmApiKey` reports whether a non-empty key is configured.
  A null or missing `llmApiKey` does not clear a stored key; an empty string
  passed to `AppConfig::set_llm_api_key` (or `AppConfigDto::apply_to`) does.

## Library Usage

The crate is also a library: `state::AppState` is an embedding-friendly
facade used by the HTTP API and external GUIs (e.g. a Tauri desktop app).
It loads portfolios/workspaces/policies, does position CRUD, and runs every
analysis (context, review, simulate, doctor, validate, reports) with
camelCase-serializable DTOs.

```rust
use portfolio_rs::AppState;

async fn total_value() -> eyre::Result<f64> {
    let state = AppState::new("EUR".to_string());
    state.load_file("positions.json").await?;
    let summary = state.get_portfolio_summary().await;
    Ok(summary.total_value)
}
```

## TUI Features

The interactive Terminal User Interface (default mode) provides:

- **Overview & Allocation Tab**: Large display of total portfolio value, visual bar chart, and detailed allocation breakdown
- **Balances Tab**: Detailed table of all positions with amounts, current values, and **edit functionality**

### TUI Customization

You may optionally specify which tab to open at start-up:

    portfolio_rs [JSON_FILE] --tab overview     # Start on Overview & Allocation tab (default)
    portfolio_rs [JSON_FILE] --tab balances     # Start on Balances tab

You can disable specific UI components using the `--disable` flag with comma-separated component names:

**Overview Tab Components:**

- `tab_bar` - Top navigation tabs
- `total_value` - Large portfolio value display
- `asset_allocation` - Visual bar chart
- `detailed_allocation` - Allocation percentage list
- `help` - Help text at bottom

**Balances Tab Components:**

- `tab_bar` - Top navigation tabs
- `name` - Position name column
- `asset_class` - Asset class column
- `amount` - Amount/quantity column
- `balance` - Balance/value column

**Examples:**

```bash
# Hide tab bar and help text
portfolio_rs --disable tab_bar,help example_data.json

# Show only the allocation chart (hide detailed list)
portfolio_rs --disable detailed_allocation example_data.json

# Minimal balances view (name and balance only)
portfolio_rs --disable asset_class,amount example_data.json
```

### TUI Navigation

- `h` / `l` : Switch tabs left/right (vim-style)
- `j` / `k` : Navigate up/down (select positions in Balances tab)
- `e` : Edit selected position amount (in Balances tab)
- `Tab` / `←` `→` : Switch between tabs
- `1-2` : Jump directly to specific tabs
- `q` / `Esc` : Quit the application

### Edit Functionality

- Select any position with `j`/`k` and press `e` to edit
- Real-time balance preview and input validation
- Changes are saved automatically to your data file
- Supports decimal precision for crypto and fractional shares

## Screenshots

### Overview & Allocation Tab

![TUI Overview](https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/tui_overview.png)

### Balances Tab with Edit Functionality

![TUI Balances](https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/tui_balances.png)

## Demo

![demo](https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/demo.gif)

## Configuration

Upon first run, `portfolio_rs` will create a default config file.
Use `portfolio_rs config` to show the config file location.

The most useful config entry is `portfolio_file` where you can set the
**absolute** path to your data file. This will be used when no file is
specified as an argument. Workspace users can point `workspace_dir` at their
workspace instead; the workspace's `positions.json` then takes precedence.

`llm_api_key` lives in the same file but is a write-only secret. The
`AppConfigDto` JSON view used by GUIs always serializes it as `null` and
exposes `hasLlmApiKey` instead. To clear a stored key, set it to an empty
string. A null or omitted key on an update does not wipe it.

`currency` (default `EUR`) is the base currency for totals. Quotes in another
currency (as reported by Yahoo) are converted with one FX quote per currency,
e.g. `EURUSD=X`; only the ticker symbol is sent. Prices and average cost are
shown in the quote currency (e.g. `512.30 USD`); values, invested amounts, and
totals are in the base currency. Purchase prices are treated as quote-currency
prices and converted at the current FX rate, so cost basis ignores past FX
moves (MCP responses flag this as `cost_basis_current_fx`). If an FX quote, or
the quote currency itself, is unknown, the affected positions are flagged, their
invested amount is null, and they are excluded from totals rather than mixed in
unconverted.

## Bonus: GPG Encryption

This tool supports (gpg) encrypted json files.
Decrypted values are never written to disk.

    # you will need a valid gpg key in ~/.gnupg/
    portfolio_rs [COMMAND] data.json.gpg

Pro Tip: Use a plugin like [vim-gnupg](https://github.com/jamessan/vim-gnupg)
for editing your data file.
