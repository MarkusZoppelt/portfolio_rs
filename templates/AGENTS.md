# Agent Instructions

This is a private, local-first finance workspace managed by `portfolio_rs`.

## Local Files

- `positions.json` — portfolio data (or `positions.json.gpg`)
- `INVESTMENT_POLICY.md` — your financial constitution
- `portfolio/policy.toml` — machine-readable policy
- `portfolio/diary/` — observations and notes
- `portfolio/decisions/` — structured decision records
- `portfolio/reports/` — generated reports

## Safety

- Do not place trades.
- Do not interact with brokers or exchanges.
- Do not provide regulated financial advice.
- Use `--dry-run` before any file mutation.
- Never write decrypted `.gpg` contents to disk in plaintext.
- Do not commit private finance files.
- Do not scrape portfolio data for external services.

## Agent Skill

If your agent harness supports skills, install the `portfolio-rs` skill:

```bash
portfolio_rs agent skill export <SKILLS_DIR>
```

Then load `<SKILLS_DIR>/portfolio-rs/SKILL.md` into your agent.
