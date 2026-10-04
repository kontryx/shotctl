mod app;
mod config;
mod import;
mod media;
mod ui;

#[cfg(test)]
mod tests;

use std::io::{self, IsTerminal};

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut source = None;
    let mut config_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "shotctl — photo import and archive manager\n\nUsage: shotctl [--source PATH] [--config PATH]\n\n  --source PATH   Select a card or photo directory\n  --config PATH   Use a specific configuration file\n  -h, --help      Show help\n  -V, --version   Show version\n\nRun shotctl in a terminal. First launch walks you through archive setup.\nPhotos are copied and verified; source files are always preserved."
                );
                return Ok(());
            }
            "-V" | "--version" => {
                println!("shotctl {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--source" => {
                source = Some(config::expand_path(
                    &args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--source needs a path"))?,
                ))
            }
            "--config" => {
                config_path = Some(config::expand_path(
                    &args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--config needs a path"))?,
                ))
            }
            _ => bail!("Unknown argument {arg}. Use --help for usage."),
        }
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("shotctl needs an interactive terminal. Use --help for usage.");
    }
    let mut app = app::App::new(
        config_path.map(Ok).unwrap_or_else(config::config_path)?,
        source,
    )?;
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}
