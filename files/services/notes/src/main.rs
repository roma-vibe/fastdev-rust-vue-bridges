//! `app-notes` as a separate process (`BRIDGES=off`, `SEPARATE_SERVICES`): see `lib.rs`.

fn main() -> anyhow::Result<()> {
    app_core::service::main("notes", app_notes::build)
}
