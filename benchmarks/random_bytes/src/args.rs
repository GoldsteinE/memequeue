use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Recv {
        count: usize,
    },
    Send {
        count: usize,
        min_size: usize,
        max_size: usize,
    },
}

#[derive(Debug, Clone, Parser)]
pub struct Args {
    pub file_name: PathBuf,
    #[clap(subcommand)]
    pub command: Command,
}

pub fn parse() -> Args {
    Args::parse()
}
