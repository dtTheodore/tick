# games/

One folder per mini-game. Each game owns its boundary — UI, optional backend,
optional Move contracts, optional library — and depends on the platform via
`@dopamint/sdk`, `@dopamint/types`, `@dopamint/sui`, and `@dopamint/core`.

## Per-game layout

```
games/<game>/
├── ui/         # frontend (framework chosen by the game's dev)
├── backend/    # optional Rust crate (axum), wired into the root Cargo workspace
├── contracts/  # optional Sui Move package
└── lib/        # optional packages this game exposes back to the platform
```

## Adding a new UI

The frontend framework is a per-game decision — Phaser for arcade games,
React/Vite for content games, Svelte for tiny ones, all fine.

When scaffolding the UI:

1. Use `bun create vite` (or the framework's idiomatic init) inside `games/<game>/ui`.
2. The folder is already covered by the root `package.json` `workspaces` glob
   (`games/*/ui`) — `bun install` from the repo root will pick it up.
3. Read ports from env (`process.env.<GAME>_UI_PORT`) populated by
   `scripts/worktree-env.sh`. **Never hardcode ports.**
4. Import the platform SDK with `import { DopamintSdk } from '@dopamint/sdk'`.

## Adding a backend

1. `mkdir games/<game>/backend/src && touch games/<game>/backend/Cargo.toml`
   (use `games/trivia-show/backend/Cargo.toml` as the template).
2. Add the new crate path to `members` in the root `Cargo.toml`.
3. Read its port from `<GAME>_BACKEND_PORT`.

## Adding contracts

```bash
cd games/<game>
sui move new contracts
```

Then edit `contracts/Move.toml` to use a `dopamint_<game>` package name (the
platform convention) and rename the generated `contracts.move` to a meaningful
file name.
