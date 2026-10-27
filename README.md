<p align="center">
    <img src="https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/logo.png" alt="portfolio_rs logo"><br>
    <img src="https://github.com/markuszoppelt/portfolio_rs/actions/workflows/rust.yml/badge.svg" alt="build status badge">
</p>

Local-first portfolio management for humans and AI agents, written in Rust.

`portfolio_rs` is an interactive terminal UI for tracking investment
portfolios: your total value at a glance, allocation by asset class, and a
balances table you can edit in place. The same data is available as CLI
commands, and AI agents get a safe, local view of your finances:
machine-readable investment policies, policy-aware reviews, rebalancing
simulations, durable records (diary, decisions, reports), a portable agent
skill, a read-only MCP server and a local HTTP API. GUIs can embed it as a
Rust library.

> _This project is the modern successor of [finance](https://github.com/MarkusZoppelt/finance)._

## Installation

- [nixpkgs](https://search.nixos.org/packages?query=portfolio_rs): `nix-shell -p portfolio_rs` or `nix run nixpkgs#portfolio_rs`
- [pkgx](https://pkgx.sh): `pkgx portfolio_rs`
- cargo: `cargo install portfolio_rs`

## Quick start

**Simple mode.** Create a JSON file with your positions (see
[example data](example_data.json) for the schema), then launch the TUI:

    portfolio_rs my_positions.json

A position's `Amount` is its quantity when it has a `Ticker` (and for cash, its
balance). A position without a ticker that is not cash is a *manual* holding:
its `Amount` is a value, and its price shows as `-` in tables. With `Purchases`
the quantity is the sum of the purchase quantities. Without purchases a manual
holding counts as one item valued at `Amount` (quantity 1 in `get_positions` and
`get_context`); valuation totals always use `Amount` for manual holdings.

**Workspace mode.** For policies, diary, decisions, and reports, create a
workspace:

    portfolio_rs init-workspace my-portfolio
    portfolio_rs policy init --strategy balanced-growth my-portfolio
    portfolio_rs context my-portfolio/positions.json

This creates an `INVESTMENT_POLICY.md` (your financial constitution), a
machine-readable `portfolio/policy.toml`, folders for diary entries, decision
records, theses, and reports, agent instructions (`AGENTS.md`, `CLAUDE.md`),
and a `.gitignore` that keeps your private data out of git.

## The TUI

Run `portfolio_rs` with a positions file (or none, to use the one from your
config) and you land in the TUI. It has two tabs:

- **Overview & Allocation**: total portfolio value, an allocation bar chart
  and the percentage per asset class.
- **Balances**: every position with amount and current value. Select one with
  `j`/`k` and press `e` to edit its amount; the change is saved to your file.

Switch tabs with `h`/`l`, `Tab` or `1`/`2`, quit with `q`. Start on a tab with
`--tab balances`, and hide parts of the screen with `--disable` (see
[docs/tui.md](docs/tui.md)).

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
| `snapshot` | Record today's valuation for `get_performance` |
| `config` | Show config file location |
| `components` | List TUI components for `--disable` |

Without a file argument, commands use the portfolio file (or workspace) from
your config. Run `portfolio_rs <COMMAND> --help` for details.

## Documentation

- [TUI](docs/tui.md): tabs, navigation, editing, hiding components
- [AI and agent integration](docs/agents.md): JSON output, policies, agent skill
- [MCP server](docs/mcp.md): tools, resources, price cache, snapshots, client setup
- [HTTP API](docs/http-api.md): endpoints, auth, local-only guarantees
- [Library usage](docs/library.md): embedding `AppState` in a GUI
- [Quotes, currencies and returns](docs/quotes.md): quote sources, FX, `performance`
- [Configuration](docs/configuration.md): config file, GPG-encrypted portfolios
