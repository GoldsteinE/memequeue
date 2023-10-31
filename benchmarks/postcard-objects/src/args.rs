use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Recv,
    Send,
}

#[derive(Debug, Clone, Parser)]
pub struct Args {
    pub file_name: PathBuf,
    pub count: usize,
    #[clap(short, long)]
    pub batch_size: Option<usize>,
    #[clap(subcommand)]
    pub command: Command,
}

pub fn parse() -> Args {
    Args::parse()
}
