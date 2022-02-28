use anyhow::{anyhow, ensure, Context, Result};
use futures_util::{pin_mut, stream::FuturesUnordered, StreamExt};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tokio::{fs, task};

use crate::{
    backends::{BuildQueue, Database, FileMetadata, RemoteSource},
    models::{self, prelude::*},
};

pub struct AcmeService {
    database: Database,
}

impl AcmeService {
    pub async fn run(mut self) -> Result<()> {
        Ok(())
    }
}
