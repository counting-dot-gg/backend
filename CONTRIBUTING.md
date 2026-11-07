# Contributing

Thanks for helping. Start with the [README](README.md) to get a backend
running.

## Making a change

1. Fork the repo and make a branch.
2. Make your change. Keep it to one thing.
3. Run the checks:

   ```
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --release --workspace
   cargo build --release -p cgg-socket -p cgg-harness
   target/release/cgg-harness test
   ```

4. Open a pull request saying what changed and why.

## What CI checks on every pull request

- **Formatting and lints:** `cargo fmt` and `cargo clippy` find nothing.
- **Dependencies:** `cargo deny` finds no known security advisories, and
  every dependency's licence is compatible with Apache-2.0.
- **Tests:** all unit tests pass.
- **Behaviour:** the harness starts the real socket, counts with demo
  accounts, and checks the results: counts in order, wrong counts and double
  posts rejected, rapid posts kept in order, recovery after a crash.
- **Speed:** CI benchmarks your version and the current one side by side.
  If yours is slower by more than the allowed margin, the check fails and shows which number got
  worse. The table is on the pull request's checks page.

If you change how the socket behaves, add a case to
`crates/harness/src/suite.rs`.

## Code style

- No comments, doc comments (`///`) included. Names, small functions and
  types should make the code clear. If something truly needs explaining, say
  so in the pull request instead. This is deliberate: the workspace is an
  application, not a published library, so there are no API docs to
  generate.
- Shared code goes in a library crate inside the workspace.
- Formatting: VS Code formats on save. Otherwise run `cargo fmt --all`
  before you commit (AGENTS.md tells AI assistants to). Pull requests fail if anything
  isn't formatted.

## Conduct and security

Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).
Security problems go through [SECURITY.md](SECURITY.md), not public issues.

## Where things are

| Folder | Contents |
|---|---|
| `crates/socket` | the live socket server |
| `crates/counting` | reading and checking counts |
| `crates/stats`, `crates/stats-server` | the stats service |
| `crates/harness` | the dev backend, test suite and benchmark |
| `crates/core` | settings, database connection, logging |
