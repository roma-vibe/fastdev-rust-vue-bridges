//! `app-edge` as a separate process (`BRIDGES=off`, `SEPARATE_SERVICES`): see `lib.rs`.

fn main() -> anyhow::Result<()> {
    app_core::service::main("edge", app_edge::build)
}
