# Contributing to Destruct-O-Vision

Thanks for helping. Destruct-O-Vision is early, so expect things to move. Open an issue before starting anything large, so we can agree on the shape first.

## Build and test

You need Rust through [rustup](https://rustup.rs); `rust-toolchain.toml` pins the version. Development happens on macOS (Apple silicon). Linux and Windows builds haven't been tried yet, and reports or fixes for them are welcome.

```sh
cargo run --release -p dov-app -- --demo lab
cargo test
cargo fmt
cargo clippy --all-targets -- -D warnings
```

CI runs the last three on every pull request, and all of them must pass.

The tests don't need a GPU. Every module's shader is compiled and validated with naga, so a broken kernel fails `cargo test` before anyone has to run the app.

## Adding a module

Built-in modules are defined in `crates/dov-modules/src/library.rs`. Each one has:
- a `ModuleDesc`: its ports, its params (with optional CV jacks) and its kernel
- a small WGSL `process` function

The engine generates everything else: bindings, CV handling and the panel. The crate docs in `crates/dov-modules/src/lib.rs` describe what a kernel can use. Copy a similar module and start from there.

Before you open a pull request:
- Patch the module in the rack and look at it. For visual changes, attach a snapshot (the Snapshot button, or `--snapshot-after SECS`).
- Port and param ids are stored in patch files. Don't rename or remove them, or existing patches break.

## Style

- **Formatting:** `cargo fmt`, with a line width of 110.
- **Comments:** explain why something is done, or what the signal means, not what the next line does. Match the density of the code around you.
- **Commits:** keep each one focused, with a message that says what changed and why.

## Licensing

Destruct-O-Vision is GPL-3.0-or-later. The module format's pieces (the standard WGSL library and the built-in kernels) are MIT; see the README. By contributing, you agree that your contribution is licensed under the same terms as the files it changes.
