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
cargo coverage             # 100% line coverage gate (needs the nix dev shell)
```

A change is done only when all five pass. Run them before you call a task finished,
not just the tests you touched.

## Coverage: 100% lines, enforced

Every line under `src/` runs under `cargo test`. CI fails below 100%. The gate is
the `coverage` alias in `.cargo/config.toml` (cargo-llvm-cov). The dev shell provides
cargo-llvm-cov and the matching LLVM tools. Outside it, install `cargo-llvm-cov` and
the `llvm-tools-preview` rustup component. When it fails, it lists the missing lines
per file.

- New or changed code ships with tests in the same change. Don't lower the threshold,
  and don't add exclusions to get green.
- Tests must assert behavior: outputs, errors, state, exit codes. A test that only
  executes code to turn lines green is not a test. Cover error paths on purpose.
- Tests stay offline and deterministic. Use the `fixture:`/`offline` quote providers
  (`src/services/quotes.rs`), tempdirs, and `example_data.json`. Never use the network,
  the real config dir, `./database`, or `positions.json`.
- If code is hard to reach, refactor it so it can be tested. Split pure logic from
  I/O, inject the provider/clock/writer, delete unreachable branches. Don't write
  contrived tests.
- The only exclusion is files named `*_io.rs`. They hold the thin shell that cannot run
  offline: live Yahoo calls, the real terminal loop. Keep them tiny and logic-free.
  Moving logic into one to dodge coverage is a review failure. A new `*_io.rs` file
  needs a reason in its module doc comment.
- `#[coverage(off)]`, `--ignore-filename-regex` changes, and threshold changes are
  human decisions. Don't make them on your own.

## Testing notes

- Network tests are all `#[ignore]`: `cargo test --test e2e_network -- --ignored`
  (requires internet / Yahoo Finance). Don't un-ignore them.
- Single test: `cargo test test_name` or `cargo test --test e2e_offline test_name`.
- CI also runs `nix flake check` and `nix fmt -- --check .` (alejandra). If you touch
  `flake.nix`, format it with `nix fmt` or CI fails.

## Gotchas

- The full command surface (context, review, simulate, policy, decision, report,
  doctor, validate, init-workspace, agent, mcp, api) is defined in `src/cli.rs`;
  README.md (command table) and `docs/` document it; keep them in sync when adding
  commands. `tests/mcp_offline.rs` checks that `docs/mcp.md` lists every MCP tool and
  resource.
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
- Portfolio writes go through the atomic helper (`src/services/atomic_write.rs`) and keep
  a single `.bak`.
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
  `Cargo.toml`. The same workflow publishes to crates.io via Trusted Publishing (no
  stored token); running it manually (`workflow_dispatch`) only does a dry run.
