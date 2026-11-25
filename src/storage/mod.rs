use std::cell::Cell;

use worker::{Env, Result};

mod device_models;
mod firmware;
mod neon;

use neon::NeonDb;

const MIGRATIONS: [&str; 8] = [
    include_str!("../../migrations/001_verified_firmware.sql"),
    include_str!("../../migrations/002_content_dedup.sql"),
    include_str!("../../migrations/003_device_models.sql"),
    include_str!("../../migrations/004_emulater.sql"),
    include_str!("../../migrations/005_lgrp_num.sql"),
    include_str!("../../migrations/006_xml_fingerprints.sql"),
    include_str!("../../migrations/007_numeric_version_cleanup.sql"),
    include_str!("../../migrations/008_submission_source.sql"),
];

thread_local! {
    static SCHEMA_READY: Cell<bool> = const { Cell::new(false) };
}

pub(crate) struct Repository {
    db: NeonDb,
}

impl Repository {
    pub(crate) async fn connect(env: &Env) -> Result<Self> {
        let db = NeonDb::from_env(env)?;
        if !SCHEMA_READY.get() {
            for migration in MIGRATIONS {
                db.query(migration, &[]).await?;
            }
            SCHEMA_READY.set(true);
        }
        Ok(Self { db })
    }
}
