#![feature(async_stream)]
#![feature(unix_socket_ancillary_data)]
#![feature(const_mut_refs)]
#![feature(ready_macro)]
// #![deny(missing_docs)]

use std::env;
use std::process::{self};

use anyhow::{Context, Result};
use clap::Parser;
use serve::CallInfo;

use crate::cli::{Cli, Command};
use crate::command::{call, inject, install, prepare, run};
use crate::oci::runtime::{OciCliRuntime, Runtime};
use crate::runtime::generator::{RunGenerator, RuntimeBundleGenerator};
use crate::serve::Serve;

mod cli;
mod command;
mod config;
mod dirs;
mod image;
mod logger;
mod metadata;
mod oci;
mod progress_bar;
mod runtime;
mod script;
mod serve;

#[tokio::main()]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    logger::init(cli.verbose.log_level()).context("could not initialize logger")?;
    log::trace!("current pid is `{}`", process::id());

    match cli.command {
        Command::Run { script, args } => run(script, args).await,
        Command::Call { script, args } => {
            let container_name = script::read_container(script)?;
            let socket_path = env::var("TOIP_SOCK")
                .context("environment variable `TOIP_SOCK` does not exists")?;
            call(socket_path, &container_name, args)
                .with_context(|| format!("could not call container `{}`", container_name))
        }
        Command::Prepare {
            container,
            ignore_missing,
        } => prepare(ignore_missing, container).await,
        Command::Install { ignore_missing } => install(ignore_missing),
        Command::Inject { shell } => inject(shell),
        _ => todo!(),
    }
}
