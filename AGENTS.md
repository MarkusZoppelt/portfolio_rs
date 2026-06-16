# AGENTS.md

Rust CLI/TUI for portfolio management. Single crate, bin + lib: `src/main.rs` just calls
`cli::run()` in `src/cli.rs` (clap builder + dispatch). Subcommand handlers live in
`src/commands/`, domain logic in `src/*.rs` (portfolio, position, policy, review,
workspace, ...), quote fetching in `src/services/` (Yahoo Finance). Static workspace/skill
templates (`INVESTMENT_POLICY.md`, `AGENTS.md`, `CLAUDE.md`, `SKILL.md`) live as plain
Markdown under `templates/` and are compiled in via `include_str!`.

## Verify (matches CI, .github/workflows/rust.yml)

```sh
cargo build
cargo test                 # unit tests + tests/{e2e_offline,api_offline}.rs
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check          # rustfmt.toml: max_width = 100
```

- Network tests are all `#[ignore]`: `cargo test --test e2e_network -- --ignored`
  (requires internet / Yahoo Finance). Don't un-ignore them.
- Single test: `cargo test test_name` or `cargo test --test e2e_offline test_name`.
- CI also runs `nix flake check` and `nix fmt -- --check .` (alejandra). If you touch
  `flake.nix`, format it with `nix fmt` or CI fails.

## Gotchas

- The full command surface (context, review, simulate, policy, decision, report,
  doctor, validate, init-workspace, agent, mcp, api) is defined in `src/cli.rs`;
  README.md documents it — keep both in sync when adding commands.
- CLI handlers in `src/commands/` return `eyre::Result` and propagate to `main`
  for non-zero exit codes. `validate` uses exit code 2 for validation failures.
- `state::AppState` + its DTOs are the public facade consumed by the HTTP API and
  the external Tauri GUI (separate repo, depends on this crate by path). Changing
  DTO shapes (all camelCase serde) is a breaking change for the GUI.
- `positions.json` and `database/` at repo root are the user's real data (gitignored).
  Never commit them; use `example_data.json` in tests/examples.
- Running the app opens a sled DB at `./database` relative to CWD
  (`src/services/persistence.rs`) — a side effect of running `balances`/TUI locally.
- `.gpg` portfolio files are decrypted by shelling out to `gpg`; decrypted content must
  never be written to disk.
- User config is loaded via confy (YAML) from the user config dir; `create_live_portfolio`
  degrades gracefully offline (returns `NetworkStatus`), so most code paths must not
  assume live quotes.
- Dev shell: nix flake + direnv (`.envrc` = `use flake`). Repo is a colocated jujutsu
  (`.jj/`) + git repo.
- The Nix build's source filter (`flake.nix`, `craneLib.filterCargoSources`) only admits
  `.rs`/`.toml`/`Cargo.lock` by default; `.json` and `.md` are explicitly allow-listed
  there for `example_data.json` and the `templates/` `include_str!`s. Any new
  `include_str!`/`include_bytes!` target needs its extension added to that filter or
  `nix build`/`nix flake check` fails even though `cargo build` succeeds locally.
- Release: pushing a `v*` tag triggers the GitHub release workflow; version lives in
  `Cargo.toml`.
