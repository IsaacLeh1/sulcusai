// SPDX-License-Identifier: AGPL-3.0-only
//! Where the app keeps its data on disk.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub models: PathBuf,
    pub engines: PathBuf,
    pub logs: PathBuf,
    pub db: PathBuf,
}

impl Paths {
    pub fn new(data: PathBuf) -> std::io::Result<Paths> {
        let paths = Paths {
            models: crate::storage::models_dir(&data),
            engines: data.join("engines"),
            logs: data.join("logs"),
            db: data.join("sulcusai.db"),
            data,
        };
        for dir in [&paths.data, &paths.models, &paths.engines, &paths.logs] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(paths)
    }

    pub fn engine_log(&self) -> PathBuf {
        self.logs.join("engine.log")
    }
}
