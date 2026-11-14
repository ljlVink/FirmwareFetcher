use worker::{Error, Result};

use crate::domain::ModelSyncSummary;

use super::neon::{SqlQuery, SqlResult};
use super::Repository;

const LOCK_SQL: &str = "SELECT pg_advisory_xact_lock(hashtext('cfworker-rs:device-models'))";
const SYNC_SQL: &str = r#"
WITH incoming AS (
    SELECT model, model_name, ver_name, source_file
    FROM jsonb_to_recordset($1::jsonb)
        AS row(model text, model_name text, ver_name text, source_file text)
), removed AS (
    DELETE FROM device_models existing
    WHERE (starts_with(existing.source_file, 'huawei_')
        OR starts_with(existing.source_file, 'honor_'))
      AND NOT EXISTS (
          SELECT 1 FROM incoming
          WHERE (incoming.model, incoming.model_name, incoming.ver_name, incoming.source_file)
              = (existing.model, existing.model_name, existing.ver_name, existing.source_file)
      )
    RETURNING 1
), added AS (
    INSERT INTO device_models (model, model_name, ver_name, source_file)
    SELECT model, model_name, ver_name, source_file FROM incoming
    ON CONFLICT (model, model_name, ver_name, source_file) DO NOTHING
    RETURNING 1
)
SELECT (SELECT count(*) FROM added), (SELECT count(*) FROM removed)
"#;
const COUNTS_SQL: &str = r#"
SELECT source_file, count(*)
FROM device_models
WHERE starts_with(source_file, 'huawei_') OR starts_with(source_file, 'honor_')
GROUP BY source_file ORDER BY source_file
"#;

fn count(value: &Option<String>) -> Result<u64> {
    value.as_deref().and_then(|value| value.parse().ok())
        .ok_or_else(|| Error::RustError("Neon returned an invalid sync count".into()))
}

fn sync_counts(mut results: Vec<SqlResult>) -> Result<ModelSyncSummary> {
    let invalid = || Error::RustError("Neon returned an invalid sync result".into());
    if results.len() != 3 || results[1].rows.len() != 1 || results[1].rows[0].len() != 2 {
        return Err(invalid());
    }
    let added = count(&results[1].rows[0][0])?;
    let removed = count(&results[1].rows[0][1])?;
    let rows = results.pop().ok_or_else(invalid)?.rows;
    let mut sources = Vec::with_capacity(rows.len());
    let (mut huawei, mut honor) = (false, false);
    for mut row in rows {
        if row.len() != 2 { return Err(invalid()); }
        let source = row[0].take().filter(|source| !source.is_empty()).ok_or_else(invalid)?;
        let amount = count(&row[1])?;
        if amount == 0 { return Err(invalid()); }
        if source.starts_with("huawei_") { huawei = true; }
        else if source.starts_with("honor_") { honor = true; }
        else { return Err(invalid()); }
        sources.push((source, amount));
    }
    if !huawei || !honor { return Err(invalid()); }
    Ok(ModelSyncSummary { added, removed, sources })
}

impl Repository {
    pub(crate) async fn sync_models(&self, payload: &str) -> Result<ModelSyncSummary> {
        let queries = [
            SqlQuery { query: LOCK_SQL, params: &[] },
            SqlQuery { query: SYNC_SQL, params: &[payload] },
            SqlQuery { query: COUNTS_SQL, params: &[] },
        ];
        sync_counts(self.db.batch(&queries).await?)
    }
}
