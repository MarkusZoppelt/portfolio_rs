---
name: portfolio-rs
description: Use when operating in a portfolio_rs finance workspace, reviewing portfolios, generating reports, drafting decisions, or working with INVESTMENT_POLICY.md, positions.json, portfolio/policy.toml, diary, decisions, theses, reports, or watchlist files.
---

# portfolio_rs Finance Skill

Use `portfolio_rs` commands as the source of truth. Do not parse raw portfolio JSON unless a command cannot answer the question.

## Startup

1. Read workspace instructions if present: `AGENTS.md`, `CLAUDE.md`, or equivalent.
2. Read `INVESTMENT_POLICY.md` if present.
3. Run `portfolio_rs doctor .`.
4. Run `portfolio_rs context positions.json --format json`.
5. If `portfolio/policy.toml` exists, run `portfolio_rs review positions.json --policy portfolio/policy.toml --format json`.

If a policy file is missing, suggest:

```bash
portfolio_rs policy init --strategy balanced-growth .
portfolio_rs policy validate portfolio/policy.toml
```

## Safety

- Do not place trades.
- Do not interact with brokers or exchanges.
- Do not provide regulated financial advice.
- Use `--dry-run` before any file mutation.
- Never write decrypted `.gpg` contents to disk in plaintext.
- Do not commit private finance files.
- Do not scrape portfolio data for external services. All analysis stays local.

## Read-Only Commands

```bash
portfolio_rs context positions.json --format markdown
portfolio_rs review positions.json --policy portfolio/policy.toml
portfolio_rs simulate positions.json --policy portfolio/policy.toml
portfolio_rs doctor .
portfolio_rs validate positions.json
```

## Structured Output (for agents/scripts)

```bash
portfolio_rs context positions.json --format json
portfolio_rs review positions.json --policy portfolio/policy.toml --format json
portfolio_rs simulate positions.json --policy portfolio/policy.toml --format json
```

## Durable Memory

```bash
portfolio_rs decision draft --title "Rebalance Stocks" --dry-run
portfolio_rs report weekly --dry-run
```

## Workspace Layout

```
.
  AGENTS.md              <- Local workspace instructions
  CLAUDE.md              <- Pointer for Claude Code
  INVESTMENT_POLICY.md   <- User-owned financial constitution
  portfolio/
    policy.toml          <- Machine-readable policy
    diary/               <- Market observations
    decisions/           <- Structured decision records
    theses/              <- Investment theses
    reports/             <- Generated reports
    watchlist.json       <- Assets being watched
```

## Decision-Making Checklist

Before suggesting any portfolio change:

1. Does this align with the target allocation in `INVESTMENT_POLICY.md`?
2. Does this respect the minimum cash buffer?
3. Does this respect restricted assets/strategies?
4. Has this been documented in the diary?
5. If significant, has a decision record been created?
6. Are tax implications considered?
7. Is the rationale clear enough to review in 6 months?

## Privacy

Portfolio data is local-first. Encrypted files (`.gpg`) are supported. Decrypted values are ephemeral and must never be written to disk in plaintext outside this workspace.
