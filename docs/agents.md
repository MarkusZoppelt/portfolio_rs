# AI and agent integration

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
  workspace: reviewable in six months, greppable forever. Use `--dry-run`
  to preview any file mutation.

- **MCP server**: `portfolio_rs mcp` serves your portfolio read-only to MCP
  clients over stdio (see [MCP server](mcp.md)).

- **HTTP API**: `portfolio_rs api` serves the same data to local scripts and
  GUIs (see [HTTP API](http-api.md)).
