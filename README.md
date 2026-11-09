# Rust + Vue bridges

Rust services that run as one process by default (in-process bridges) and can be split back into microservices without code changes, with a modular Vue 3 SPA.

This repository is a [fastDev](https://github.com/roma-vibe/fastDev) project skeleton (`rust-vue-bridges`). fastDev lists it
from a registry, downloads it when first used and creates named, ready-to-run projects from it.

- `template.toml` — the manifest: requirements, options, env, setup steps and commands.
- `files/` — the files of a new project (`*.tmpl` files are rendered with the project name and options).
- `CHANGELOG.md` — what changed in every version.

Versions are the `vX.Y.Z` tags of this repository and never change once published.
