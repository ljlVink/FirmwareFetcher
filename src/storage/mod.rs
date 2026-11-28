use std::cell::Cell;
use std::collections::HashSet;

use worker::{Env, Error, Result};

mod device_models;
mod firmware;
mod neon;

use neon::{NeonDb, SqlQuery, SqlResult};

const SCHEMA_LOCK_SQL: &str = "SELECT pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'))";
const CREATE_MIGRATIONS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
)
"#;
const APPLIED_MIGRATIONS_SQL: &str = "SELECT version FROM schema_migrations ORDER BY version";
const RECORD_MIGRATION_SQL: &str = "INSERT INTO schema_migrations (version) VALUES ($1::integer) ON CONFLICT (version) DO NOTHING";

const MIGRATIONS: [(u32, &str); 10] = [
    (1, include_str!("../../migrations/001_verified_firmware.sql")),
    (2, include_str!("../../migrations/002_content_dedup.sql")),
    (3, include_str!("../../migrations/003_device_models.sql")),
    (4, include_str!("../../migrations/004_emulater.sql")),
    (5, include_str!("../../migrations/005_lgrp_num.sql")),
    (6, include_str!("../../migrations/006_xml_fingerprints.sql")),
    (7, include_str!("../../migrations/007_numeric_version_cleanup.sql")),
    (8, include_str!("../../migrations/008_submission_source.sql")),
    (9, include_str!("../../migrations/009_url_set_key.sql")),
    (10, include_str!("../../migrations/010_normalize_ff_targets.sql")),
];

thread_local! {
    static SCHEMA_READY: Cell<bool> = const { Cell::new(false) };
}

fn parse_versions(result: SqlResult) -> Result<HashSet<u32>> {
    let mut versions = HashSet::with_capacity(result.rows.len());
    for row in result.rows {
        let version = row
            .first()
            .and_then(Option::as_deref)
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(|| Error::RustError("database returned invalid schema version".into()))?;
        versions.insert(version);
    }
    Ok(versions)
}

fn all_applied(applied: &HashSet<u32>) -> bool {
    MIGRATIONS.iter().all(|(version, _)| applied.contains(version))
}

async fn ensure_schema(db: &NeonDb) -> Result<()> {
    if let Some(result) = db.query_allowing_missing_relation(APPLIED_MIGRATIONS_SQL, &[]).await? {
        if all_applied(&parse_versions(result)?) {
            return Ok(());
        }
    }
    let mut init = db.batch(&[
        SqlQuery { query: SCHEMA_LOCK_SQL, params: &[] },
        SqlQuery { query: CREATE_MIGRATIONS_SQL, params: &[] },
        SqlQuery { query: APPLIED_MIGRATIONS_SQL, params: &[] },
    ]).await?;
    let applied = parse_versions(
        init.pop().ok_or_else(|| Error::RustError("database returned no schema versions".into()))?,
    )?;
    for &(version, migration) in &MIGRATIONS {
        if applied.contains(&version) {
            continue;
        }
        let version_str = version.to_string();
        db.batch(&[
            SqlQuery { query: SCHEMA_LOCK_SQL, params: &[] },
            SqlQuery { query: migration, params: &[] },
            SqlQuery { query: RECORD_MIGRATION_SQL, params: &[&version_str] },
        ]).await?;
    }
    Ok(())
}

pub(crate) struct Repository {
    db: NeonDb,
}

impl Repository {
    pub(crate) async fn connect(env: &Env) -> Result<Self> {
        let db = NeonDb::from_env(env)?;
        if !SCHEMA_READY.get() {
            ensure_schema(&db).await?;
            SCHEMA_READY.set(true);
        }
        Ok(Self { db })
    }
}
