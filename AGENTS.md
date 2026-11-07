# Rules for agents working in this repo

## Setting up

If the backend isn't set up on this computer yet, follow `README.md` for the
user's operating system, one step at a time, and explain what each step does
in plain words. The user may not be technical. Confirm before installing
anything. Finish by running `cgg-harness dev` and posting a count with
`cgg-harness say main 1`.

## No code comments

No `//`, `///`, `//!` or `/* */`, including in tests. Code has to explain
itself through names, small functions and types. If something is so unusual
that it seems to need a comment, tell the user in chat instead of writing
one.

## Layout

One Cargo workspace (`crates/*`), one `Cargo.lock`.

| Crate | What it is |
|---|---|
| `core` (`cgg-core`) | env loading, MySQL config and pool, logging |
| `counting` | post parsing and validation; the only post parser |
| `socket` (`cgg-socket`) | the live socket server |
| `api` (`cgg-api`) | the HTTP API server |
| `bot` (`cgg-bot`) | the Discord bot: logs, role sync, chat mirror and bingo chat relays |
| `web` (`cgg-web`) | code shared by `socket` and `api`: sessions, internal TCP messages, ids, reactions, levels |
| `stats` | stats library: engine, DB, Redis, models, shared interfaces |
| `stats-server` | the stats HTTP server and background loops (binary `stats`) |
| `progression` (`cgg-progression`) | sample achievements: definitions, live awards and the periodic check |
| `harness` (`cgg-harness`) | end-to-end correctness suite and benchmarks (CI runs them on every PR) |

A dependency used by more than one crate goes in `[workspace.dependencies]`
in the root `Cargo.toml`; crates add only their extra features. Edition,
toolchain and lint levels are set once for the workspace.

Shared code goes in a library crate inside the workspace. No path
dependencies outside the repo, and no nested git repos.

## Building

- `cargo fmt --all` after every change (`rustfmt.toml`: width 120); CI fails unformatted code
- `cargo clippy --workspace --all-targets -- -D warnings` (CI fails on any warning)
- `cargo deny check` for new dependencies: licence and advisory policy is in `deny.toml`
- `cargo test --release --locked --workspace` (what CI runs)

## Harness

`cgg-harness` starts the real socket binary against a scratch MySQL database
(`cgg_harness*`, local only) and a non-zero Redis database index, seeds
synthetic accounts and threads, and drives it over socket.io.

- `cgg-harness test`: correctness suite
- `cgg-harness bench --kind socket|api|bot`: benchmark one build. `api`
  serves read endpoints to one client then `--clients` at once; `bot` times
  commands to a fake Discord API, a `--burst` of them, and the chat mirror
- `cgg-harness ab --kind socket|api|bot --base-bin <old> --head-bin <new>`:
  interleaved benchmark; fails if `crates/harness/src/compare.rs` thresholds
  are exceeded. CI runs all three on every PR
- `cgg-harness dev [--keep]`: long-running local backend with demo threads
  and accounts on port 8805; `cgg-harness say [--as N] <thread> <text>` posts
  as a demo account

MySQL and Redis come from `.env` (see `.env.example`). If either refuses
connections, the database isn't running: `docker compose up -d mysql redis`.
New behaviour in the socket gets a case in `crates/harness/src/suite.rs`.

## Safety

Never read, print or copy secrets from `.env` files. Only ever run the
harness against local databases; it refuses anything else.

## Git

`*.md` is ignored except this file, `CLAUDE.md`, `README.md` and
`CONTRIBUTING.md`. Commit only when asked; never push unasked.
