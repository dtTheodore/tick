# CLAUDE.md

## Git

- **Always rebase over merge; squash on integration**: Keep git history linear
- **No AI trailers in commit message**: Never add any AI attribution to the commit. Commits must appear human-authored.
- **Conventional Commits**: format every commit as `<type>(<scope>): <subject>`. Types: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `build`, `ci`, `chore`, `revert`. Use `!` or a `BREAKING CHANGE:` footer for breaking changes.
- **Short commit messages**:
  - Subject line ≤ 50 chars, imperative mood, lowercase after the type, no trailing period.
  - Skip the body unless a single line genuinely can't carry the meaning. If a body is needed, at most ~5 lines covering *why* and any non-obvious decision, not *what*.
  - Push verbose context — rationale, alternatives considered, screenshots, test plan, migration notes, links to issues — into the **PR description**, not the commit.
  - One logical change per commit. If you're tempted to write a long body, that's usually a sign to split the commit.
  - Reference issues in the PR description, not in every commit subject.

## Code Convention

**Respect the language and framework you're in.** Every language and
framework has its own idioms, naming conventions, project layout,
error-handling style, and community-accepted best practices — follow
them. Don't import patterns from another ecosystem (Java-style
abstractions in Go, Rails-style magic in a Bun service, OOP heavy
hierarchies in idiomatic TypeScript) just because they're familiar.
When the official docs, the standard library, or the framework's own
examples show a way of doing something, that's the way — deviate only
with a clear reason. The rules below are project-wide; they sit *on
top of* idiomatic usage of whatever stack the file lives in, not in
place of it.

### Naming

Names (types, functions, variables, modules, files) must describe the
target's purpose, functionality, and meaning — not just its category. A
reader should be able to infer what the thing is and why it exists from
the name alone, without opening the file.

- **Specific over generic**: a name should pin down *which* thing it
  refers to. Replace category-only names (`Result`, `Data`, `Config`,
  `Item`) with names that say what kind of result, what shape of data,
  what the config governs. If a word is already overloaded in this
  codebase, qualify the new use rather than adding to the pile.
- **Consistent prefixes within a subsystem**: types, functions, and
  files that belong to the same feature should share a prefix or
  namespace so cross-file references stay legible and grep-able. Pick
  the prefix once per subsystem and stick to it.
- **One concept per name**: if a single type or function answers two
  questions (e.g. "what is this" vs. "where does it live"), split it.
  Conflated names hide invariants and make refactors painful.
- **A few extra tokens for clarity is the right trade**: avoid one- or
  two-letter identifiers outside tight local scopes, and avoid vague
  suffixes like `Handler`, `Manager`, `Helper`, `Util`, `Info`, `Data`.
  Don't introduce acronyms that aren't already established here.
- **Don't ramble**: a name carries the *meaning*; a doc comment
  carries the *mechanism*. If you're tempted to encode implementation
  details in the name (`UserCacheBackedByRedisWithTTL`), shorten the
  name and put the details in a comment or the type's docs.
- **Match the language's conventions**: follow the casing, file-naming,
  and pluralization rules already in use in this repo. Consistency
  inside the project beats theoretical purity.
- **Renames are atomic**: when renaming, update every call site,
  import, doc reference, and string literal in the same change.
  Half-renamed symbols are worse than the old bad name.
- **SQL migration files must have meaningful, human-readable names**:
  the descriptive part of the filename should say *what the migration
  does* so anyone can skim the migrations directory and understand
  the history without opening files. `add_user_email_verified_column`
  beats `migration_0042` or `update_schema`. Leave the prefix
  (timestamp, sequence number, hash) to whatever the migration tool
  in use generates — don't fight the framework's convention.

### Doc comments

A good name says *what* something is. A doc comment exists to capture
what the name can't: *why* it exists, *how* it works when that isn't
obvious, and the constraints a caller must respect. If a comment only
restates the name or signature, delete it.

- **Document the non-obvious, not the obvious**: rationale, hidden
  constraints, invariants, gotchas, edge cases, and the reason a
  surprising choice was made. Skip comments that just narrate what the
  code already says (`// increment counter`).
- **Explain *why*, then *how*; never *what***: the *what* is the code.
  Use the comment for the design decision behind it ("we retry on 429
  but not 5xx because the upstream is non-idempotent") and the
  mechanism only when it's not deducible from reading the body.
- **Document contracts at the boundary**: on public/exported APIs,
  state preconditions, postconditions, error modes, ownership/lifetime
  rules, thread-safety, and side effects. Internal helpers usually
  don't need this.
- **Call out gotchas explicitly**: if something *will* trip up the
  next reader — an off-by-one that's intentional, a workaround for a
  specific bug, an ordering requirement, a non-obvious performance
  cliff — say so plainly. Link the issue or commit when relevant.
- **Keep them short and load-bearing**: every line should earn its
  place. Prefer a tight 1–3 line block over a paragraph; if a comment
  needs to be long, the abstraction is probably wrong.
- **Comments rot — design against it**: don't reference call sites,
  ticket numbers as the *only* context, or "current" behavior that
  will change. Anchor the comment to invariants of *this* code, not to
  the surrounding world.
- **Delete stale comments aggressively**: when behavior changes,
  update or remove the comment in the same change. A wrong comment is
  worse than no comment.
- **Don't use comments to apologize or narrate process**: no `// TODO
  fix later` without an issue link, no `// added for the X feature`,
  no `// removed unused import`. That belongs in the PR or git log.

### Frameworks & Tooling

- **TypeScript / JavaScript — use `bun` fully**: `bun` is the runtime,
  package manager, script runner, test runner, and bundler for all
  TS/JS work in this repo. Use `bun install` (not `npm`/`pnpm`/`yarn`),
  `bun run <script>` (not `npm run`), `bun test` (not `jest`/`vitest`
  unless a script genuinely can't be expressed in `bun test`), and
  `bun <file.ts>` to execute TypeScript directly without a separate
  build step. Don't mix package managers — no `package-lock.json` /
  `pnpm-lock.yaml` / `yarn.lock`; only `bun.lockb` is committed.
  Prefer Bun's built-in APIs (`Bun.file`, `Bun.serve`, `Bun.$`) over
  Node equivalents when both work.

### Linting & Formatting

The rules below are the TS/JS defaults. For any other language in the
repo, use that ecosystem's idiomatic linter and formatter (e.g.
`gofmt`+`golangci-lint`, `ruff`+`black`, `rustfmt`+`clippy`) — same
principles, different tools.

- **Biome for all TS/JS linting and formatting**: a single tool owns
  both jobs — no ESLint, no Prettier, no `eslint-config-prettier`
  shims. Run `bunx biome check --write` (or the project script) before
  committing; CI runs `biome ci` and treats any finding as a failure.
- **Formatter is non-negotiable**: never hand-format around Biome.
  If a rule produces ugly output in one spot, fix the rule config or
  add a scoped `// biome-ignore` with a one-line reason — don't
  disable formatting locally.
- **Lint warnings are errors in CI**: there is no "warning" tier we
  ignore. Either the rule is on (and must be clean) or it's off in
  `biome.json`. Don't merge with new findings.
- **Config lives in `biome.json` at the repo root**: changes to lint
  rules go through review like any other code change. Don't sprinkle
  per-directory overrides unless a subdirectory genuinely has a
  different contract (e.g. generated code).

### Testing

Three tiers, each with a clear job. Pick the lowest tier that can
prove the behavior — unit if it can, integration if it must, e2e
only when the value is the wiring itself.

The taxonomy and discipline below apply across the repo regardless of
language; the *runner* should be whatever's idiomatic for the stack
(`bun test` for TS/JS, `go test` for Go, `pytest` for Python, etc.).
Don't bring a foreign test framework into a stack just to keep tooling
uniform.

- **Unit tests**: pure logic, no IO, no network, no clock, no
  filesystem. Fast (milliseconds), deterministic, run on every save.
  Mock only what the unit under test directly depends on; don't mock
  the standard library or your own pure helpers. Co-locate as
  `*.test.ts` next to the code they cover.
- **Integration tests (Testcontainers)**: exercise real dependencies
  — real Postgres, real Redis, real S3-compatible store — spun up via
  Testcontainers. Never mock the database in integration tests; the
  whole point is to catch mock/prod divergence. Each test owns its
  own schema/namespace and cleans up after itself; no shared mutable
  fixtures across tests. Slower than unit (seconds), still run in CI
  on every PR.
- **End-to-end tests**: full stack from the user-facing entrypoint
  (HTTP, CLI, UI) through to real infrastructure. Cover *golden
  paths* and high-value regressions only — e2e is expensive, so
  don't try to cover edge cases here; push those down into unit or
  integration. Treat flakes as bugs: quarantine, root-cause, fix or
  delete; never retry-loop a flaky test green.
- **Name tests by behavior, not implementation**: `rejects expired
  tokens` beats `test_validateToken_returns_false`. The test name is
  the spec; the body is the proof.
- **Tests are first-class code**: same naming, same review bar, same
  refactor discipline. Duplicated setup belongs in helpers; flaky
  tests don't belong anywhere.

## Local Development

This repo uses a **worktree-safe local dev environment** so multiple
git worktrees can run side-by-side without port collisions, shared DB
state, or fighting over `tmp/` files. All local-dev setup goes
through the `cmk-worktree-dev-env` skill — invoke it when adding
services, changing ports, or troubleshooting overlap between
worktrees. Don't hand-roll an ad-hoc local setup that bypasses it.

- **Use the skill, don't reinvent it**: any change that adds a new
  service, daemon, port binding, or local data directory must go
  through `cmk-worktree-dev-env` so port isolation and coherence
  validation stay correct across worktrees.
- **Per-worktree state is gitignored and lives in two places**:
  - `.local/` — durable per-worktree config and data (env files,
    Postgres data dir, MinIO buckets, cached fixtures, etc.).
  - `tmp/` — ephemeral runtime artifacts (logs, PIDs, sockets, run
    manifests, anything safe to `rm -rf` between sessions).
  Both directories are gitignored; nothing inside them should ever
  be committed or referenced from production code paths.
- **Never hardcode ports**: every port — HTTP, DB, cache, message
  bus, debugger, anything that binds — must come from the
  worktree-calculated values the dev-env skill produces (typically
  via env vars or a generated config). Hardcoded ports break the
  moment a second worktree spins up. If you find one in existing
  code, replace it; don't propagate it.
- **Read config from the env, not from literals**: services should
  consume `process.env.SOMETHING_PORT` (or the framework equivalent)
  with no string-literal fallbacks like `?? 5432`. A missing env var
  is a setup bug — fail loudly rather than silently binding to the
  default and colliding with another worktree.
- **Keep paths worktree-relative**: when writing to `.local/` or
  `tmp/`, resolve paths relative to the repo root (or the worktree
  root) — never to `$HOME`, `/tmp`, or any global location. Cross-
  worktree leakage through global paths is exactly what this setup
  prevents.

## Worktrees & local-dev contract (required for agents)

For any request to start, restart, or interact with local
development, follow this contract — do not improvise around it.

- Use `.claude/worktrees/` for git worktrees (already gitignored).
- For any local-dev start, **always run `./scripts/init-worktree-dev.sh` first**.
  Do not run `bun dev`, `cargo run`, `vite`, or `cargo watch` directly
  before init has set up `.local/.env` and synced service envs.
- Never source env files from another worktree path; only use `.local/`
  files in the current repo root.
- Never bypass `scripts/ensure-worktree-coherence.sh`. If it fails,
  stop and report the mismatch — don't "fix" it by editing `.local/.env`
  by hand. Re-run `./scripts/init-worktree-dev.sh` instead.
- For interactive human sessions, start services with
  `mprocs --config mprocs.yaml`.
- For non-interactive / headless agent runs (no TTY), use
  `./scripts/start-headless.sh` (stop with `--stop`). Tail logs with
  `./scripts/logs.sh [service]`. Logs go to `tmp/*.log`, PIDs to
  `tmp/pids/`.
- When adding a new service that binds a port, edit
  `scripts/worktree-env.sh` (add the port export), `scripts/sync-service-envs.sh`
  (write the per-service env), `scripts/ensure-worktree-coherence.sh`
  (add a `check_port` call), and `mprocs.yaml` / `start-headless.sh`
  (register the process). All four must move together.

## Backend architecture & plan (authoritative)

Backend implementation is anchored to two specs in `docs/`:

- `docs/Sui-Mini-Game-Platform.md` — system architecture (9 chapters,
  target state at 100K DAW / 1M MAU).
- `docs/dopamint_infra_plan.agent.final.md` — phased implementation
  plan (10 sections, 14-week roadmap, 7 workstreams).

Both treat 100K DAW / 1M MAU as capacity assumptions, not PoC
targets. The plan presumes 7 parallel workstreams of ~2 engineers
each; we are executing it as a single solo track in plan-ordered
dependency sequence (A → B → C → F → D → E → G), with stack picks
deferred behind interfaces where they aren't load-bearing yet (local
Postgres / Redis / `sui-test-validator` instead of RDS / ElastiCache /
Shinami at this stage).

When the two specs contradict each other, or a decision needs to be
made that neither doc resolves (e.g. Catalog Service ownership, guest
wallet custody, indexer implementation strategy, Snake.io UDP ingress),
record the resolution under `docs/decisions/` as a short ADR before
writing the code that depends on it. The folder doesn't exist until
the first ADR is written.

New backend Rust crates follow the layout in plan §1.1.2:
`platform/backend/api/<service>/` for axum service crates (library
crate exposing `pub fn router() -> Router`, mounted by the gateway),
`platform/backend/workers/` for the worker fleet binary,
`platform/lib/<name>/` for shared libraries
(`platform-lib-types`, `platform-lib-core`, `platform-lib-sui`,
`platform-lib-codegen`, `platform-lib-sdk`). Add new crates to the
root `Cargo.toml` workspace `members` list.

