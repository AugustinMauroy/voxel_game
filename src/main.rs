mod app;
mod debug;
mod gameplay;
mod meshing;
mod persistence;
mod renderer;
mod streaming;
mod ui;
mod world;

fn main() -> anyhow::Result<()> {
    renderer::run()
}
