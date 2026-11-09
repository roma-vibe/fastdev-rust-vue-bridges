# Rust + Vue bridges skeleton — instructions for AI agents

This repository is a fastDev skeleton (`rust-vue-bridges`). It is not a project: fastDev copies `files/` into
new projects and renders `*.tmpl` files. Read the fastDev skeleton authoring guide first
(MCP tool `get_authoring_guide`, or `docs/skeleton-authoring.md` in the fastDev repository).

- Change `template.toml` and `files/` only; `files/AGENTS.md.tmpl` and `files/SPEC.md.tmpl` are the
  instructions of the future projects, not of this repository.
- Do not commit, tag or edit `CHANGELOG.md` by hand: validate, verify and publish through fastDev
  (`validate_skeleton`, `verify_skeleton`, `publish_skeleton`). Publishing commits, creates the
  `vX.Y.Z` tag, pushes and updates the registry.
- Never move or delete a published tag.
- Everything is written in English.
- This skeleton is a fork of `rust-vue-microservices` 1.0.0 (`forked_from` in the manifest, keep it). Its
  difference is `docs/bridges.md`: services are libraries, `app/` links them into one process, and
  `BRIDGES=off` / `SEPARATE_SERVICES` bring microservices back.
- The `auth` feature switches files by `features.auth` lists and `@auth=on|off` alternatives
  (`crates/core/src/lib.rs`, `caller.rs`, `Cargo.lock`, root `Cargo.toml`, `app/Cargo.toml`,
  `app/src/main.rs`, `app/src/tests.rs`). `[verify] variants` cannot turn features off, so before
  publishing also create a project with `auth` off (`create_project`, version `draft`) and run
  `npm run check` and `npm run build` in it.
- Two `Cargo.lock` files: regenerate both (`cargo generate-lockfile` in a project with auth on and
  one with auth off) whenever Rust dependencies change.
- A new service must be linked in `app/` (both `auth` variants where it applies), in
  `[workspace.dependencies]` and in `services.json.tmpl`; `app/src/tests.rs` fails otherwise.
