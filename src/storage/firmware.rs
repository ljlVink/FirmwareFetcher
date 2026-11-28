use std::collections::HashSet;

use worker::{Error, Result};

use crate::domain::{FirmwareInput, FirmwarePackage, InsertAttempt, RefreshSeeds, SubmissionSource};

use super::neon::{SqlQuery, SqlResult};
use super::Repository;

const LIST_SQL: &str = r#"
WITH devices AS (
    SELECT brand, dev, code, split_part(code, '-', 1) AS family,
           regexp_replace(split_part(code, '-', 1), '([0-9])[A-Z]+$', '\1') AS family_root,
           substring(code FROM '^[A-Z0-9]{3,}-([A-Z]+)[0-9]') AS letters,
           substring(code FROM '^[A-Z0-9]{3,}-[A-Z]+([0-9]{1,6})')::int AS digits
    FROM (SELECT DISTINCT brand, dev, upper(dev) AS code FROM firmware_submissions) d
), models AS (
    SELECT model_name, brand, source_file = brand || '_cn' AS cn, code, split_part(code, '-', 1) AS family,
           regexp_replace(split_part(code, '-', 1), '([0-9])[A-Z]+$', '\1') AS family_root,
           substring(code FROM '^[A-Z0-9]{3,}-([A-Z]+)[0-9]') AS letters,
           substring(code FROM '^[A-Z0-9]{3,}-[A-Z]+([0-9]{1,6})')::int AS digits
    FROM (SELECT model_name, source_file, split_part(source_file, '_', 1) AS brand,
                 upper(regexp_replace(model, '^.*\s', '')) AS code
          FROM device_models
          WHERE starts_with(source_file, 'huawei_') OR starts_with(source_file, 'honor_')) m
),
-- 机型名候选取最优一档：精确（可跨品牌，同品牌优先）> 提交型号多一位 > 机型表多一位 > CN 尾缀 > 同前缀系列 > 数字系列去字母尾缀；
-- 非精确档只用同品牌；系列内比网络字母（全同 > 首字母同 > 其他）和数字距离，最后中国来源优先，并列名称用 " / " 连接。
candidates AS (
    SELECT d.brand, d.dev, m.model_name,
           rank() OVER (PARTITION BY d.brand, d.dev ORDER BY
               t.tier, m.brand <> d.brand,
               CASE WHEN t.tier < 4 OR m.letters = d.letters THEN 0
                    WHEN left(m.letters, 1) = left(d.letters, 1) THEN 1 ELSE 2 END,
               CASE WHEN t.tier < 4 THEN 0 ELSE abs(m.digits - d.digits) END,
               NOT m.cn) AS position
    FROM devices d
    JOIN models m ON m.family_root = d.family_root
    CROSS JOIN LATERAL (SELECT CASE
        WHEN m.family <> d.family THEN 5
        WHEN m.code = d.code THEN 0
        WHEN length(d.code) = length(m.code) + 1 AND starts_with(d.code, m.code) THEN 1
        WHEN length(m.code) = length(d.code) + 1 AND starts_with(m.code, d.code) THEN 2
        WHEN d.code = m.code || 'CN' THEN 3
        ELSE 4 END AS tier) t
    WHERE t.tier = 0 OR (m.brand = d.brand AND (t.tier < 4 OR (d.letters IS NOT NULL AND m.letters IS NOT NULL)))
), names AS (
    SELECT brand, dev, string_agg(DISTINCT model_name, ' / ' ORDER BY model_name) AS model_name
    FROM candidates WHERE position = 1
    GROUP BY brand, dev
), filtered AS (
    SELECT s.id
    FROM firmware_submissions s
    WHERE ($1 = '' OR s.dev ILIKE $1 OR s.vendor ILIKE $1 OR s.target_bl_version_number ILIKE $1
        OR EXISTS (SELECT 1 FROM firmware_packages p
                   WHERE p.submission_id = s.id AND p.version_number ILIKE $1)
        OR EXISTS (SELECT 1 FROM names n
                   WHERE n.brand = s.brand AND n.dev = s.dev AND n.model_name ILIKE $1))
      AND CASE WHEN $2 = '' THEN ($5 = '1' OR s.upgrade_or_erec <> 'emulater')
               ELSE s.upgrade_or_erec = $2 END
), page AS (
    SELECT id FROM filtered ORDER BY id DESC LIMIT $3::int OFFSET $4::int
)
SELECT jsonb_build_object(
    'total', (SELECT count(*) FROM filtered),
    'page', $4::int / $3::int + 1,
    'perPage', $3::int,
    'requests', coalesce(jsonb_agg(jsonb_build_object(
        'id', s.id,
        'created_at', to_char(s.created_at, 'YYYY-MM-DD HH24:MI:SS'),
        'source', s.source,
        'dev', s.dev, 'brand', s.brand, 'vendor', s.vendor, 'upgrade_or_erec', s.upgrade_or_erec,
        'targetBlVersionNumber', s.target_bl_version_number,
        'deviceName', s.model_name,
        'packages', s.packages
    ) ORDER BY s.id DESC), '[]'::jsonb)
)::text
FROM (
    SELECT s.id, s.created_at, s.source, s.dev, s.brand, s.vendor, s.upgrade_or_erec, s.target_bl_version_number,
           n.model_name,
           jsonb_agg(jsonb_build_object('versionNumber', p.version_number, 'url', p.url)
                     ORDER BY p.position) AS packages
    FROM firmware_submissions s
    JOIN firmware_packages p ON p.submission_id = s.id
    LEFT JOIN names n ON n.brand = s.brand AND n.dev = s.dev
    WHERE s.id IN (SELECT id FROM page)
    GROUP BY s.id, n.model_name
    ORDER BY s.id DESC
) s
"#;
const INSERT_SQL: &str = r#"
WITH packages AS (
    SELECT entry.position::integer AS position,
           entry.package->>'versionNumber' AS version_number, entry.package->>'url' AS url
    FROM jsonb_array_elements($10::jsonb) WITH ORDINALITY AS entry(package, position)
    WHERE btrim(entry.package->>'versionNumber') !~ '^[0-9]{9}$'
), key_calc AS (
    SELECT 'urlset:v1:' || encode(
        sha256(
            convert_to(
                'FirmwareFetcher URL set v1' || E'\n' ||
                string_agg(octet_length(u.url)::text || ':' || u.url, E'\n' ORDER BY u.url COLLATE "C"),
                'UTF8'
            )
        ),
        'hex'
    ) AS url_set_key
    FROM (SELECT DISTINCT url FROM packages) u
    HAVING count(*) > 0
), already_exists AS (
    SELECT 1
    FROM key_calc k
    WHERE EXISTS (
        SELECT 1 FROM firmware_submissions s
        WHERE s.brand = $11 AND s.content_key = $9
    ) OR EXISTS (
        SELECT 1 FROM firmware_submissions s
        WHERE s.brand = $11 AND s.url_set_key = k.url_set_key
    )
), legacy AS (
    SELECT s.id AS sort_id, s.id::text AS id,
           jsonb_agg(jsonb_build_object('versionNumber', p.version_number, 'url', p.url)
                     ORDER BY p.position) AS packages
    FROM firmware_submissions s
    JOIN firmware_packages p ON p.submission_id = s.id
    WHERE $13 = '1'
      AND s.brand = $11 AND s.target_bl_version_number = $8 AND s.content_key IS NULL
      AND s.upgrade_or_erec <> 'emulater'
      AND EXISTS (SELECT 1 FROM key_calc)
      AND NOT EXISTS (SELECT 1 FROM already_exists)
    GROUP BY s.id
), saved_submission AS (
    INSERT INTO firmware_submissions
        (dev, base, cust, preload, patch, vendor, upgrade_or_erec, target_bl_version_number, content_key, brand, source, url_set_key)
    SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $11, $12, k.url_set_key
    FROM key_calc k
    WHERE NOT EXISTS (SELECT 1 FROM already_exists)
      AND NOT EXISTS (SELECT 1 FROM legacy)
    ON CONFLICT DO NOTHING
    RETURNING id, created_at, source, dev, brand, vendor, upgrade_or_erec, target_bl_version_number
), saved_packages AS (
    INSERT INTO firmware_packages (submission_id, position, version_number, url)
    SELECT s.id, p.position, p.version_number, p.url
    FROM saved_submission s CROSS JOIN packages p
    RETURNING position, version_number, url
)
SELECT jsonb_build_object(
    'legacy', coalesce((SELECT jsonb_agg(jsonb_build_object('id', id, 'packages', packages) ORDER BY sort_id) FROM legacy), '[]'::jsonb),
    'saved', (
        SELECT jsonb_build_object(
            'id', s.id, 'created_at', to_char(s.created_at, 'YYYY-MM-DD HH24:MI:SS'),
            'source', s.source,
            'dev', s.dev, 'brand', s.brand, 'vendor', s.vendor, 'upgrade_or_erec', s.upgrade_or_erec,
            'targetBlVersionNumber', s.target_bl_version_number,
            'packages', (SELECT jsonb_agg(jsonb_build_object('versionNumber', version_number, 'url', url)
                                           ORDER BY position) FROM saved_packages)
        )
        FROM saved_submission s
    )
)::text
"#;
const BASELINE_SQL: &str = r#"
UPDATE firmware_submissions SET content_key = $2
WHERE id = $1::bigint AND content_key IS NULL
  AND NOT EXISTS (
      SELECT 1 FROM firmware_submissions WHERE brand = $3 AND content_key = $2
  )
"#;
const XML_BASELINE_LOCK_SQL: &str = "SELECT pg_advisory_xact_lock(hashtext('cfworker-rs:xml-baseline'))";
const EXISTING_URL_KEYS_SQL: &str = r#"
SELECT s.url_set_key
FROM jsonb_array_elements_text($2::jsonb) AS k(key)
JOIN firmware_submissions s
  ON s.brand = $1 AND s.url_set_key = k.key
"#;
const SEEDS_SQL: &str = r#"
WITH recovery_pkgs AS (
    SELECT s.id, s.dev, s.vendor, s.lgrp_num,
           jsonb_agg(p.version_number ORDER BY p.position) AS packages
    FROM firmware_submissions s
    JOIN firmware_packages p ON p.submission_id = s.id
    WHERE s.brand = 'huawei'
      AND s.upgrade_or_erec = 'recovery'
      AND s.lgrp_num IS NOT NULL
    GROUP BY s.id
)
SELECT jsonb_build_object(
    'upgradeOnly', (
        SELECT count(DISTINCT u.dev)
        FROM firmware_submissions u
        WHERE u.brand = 'huawei' AND u.upgrade_or_erec = 'upgrade'
          AND NOT EXISTS (
              SELECT 1 FROM firmware_submissions r
              WHERE r.brand = 'huawei' AND r.upgrade_or_erec = 'recovery' AND r.dev = u.dev
          )
    ),
    'invalidRecovery', (
        SELECT count(*)
        FROM firmware_submissions s
        WHERE s.brand = 'huawei'
          AND s.upgrade_or_erec = 'recovery'
          AND s.lgrp_num IS NULL
    ),
    'seeds', coalesce((
        SELECT jsonb_agg(jsonb_build_object(
            'id', r.id, 'dev', r.dev, 'vendor', r.vendor,
            'lgrp', r.lgrp_num, 'packages', r.packages
        ) ORDER BY r.id)
        FROM recovery_pkgs r
    ), '[]'::jsonb)
)::text
"#;

const URL_KEY_QUERY_CHUNK: usize = 1000;
const URL_KEY_BATCH_CHUNKS: usize = 8;

fn json_body(mut result: SqlResult) -> Result<String> {
    result.rows.pop().and_then(|mut row| row.pop()).flatten()
        .ok_or_else(|| Error::RustError("database returned no response".into()))
}

impl Repository {
    pub(crate) async fn list(
        &self,
        pattern: &str,
        action: &str,
        per_page: u32,
        offset: u32,
        include_emulators: &str,
    ) -> Result<String> {
        json_body(self.db.query(LIST_SQL, &[
            pattern, action, &per_page.to_string(), &offset.to_string(), include_emulators,
        ]).await?)
    }

    pub(crate) async fn set_legacy_fingerprint(&self, id: &str, key: &str, brand: &str) -> Result<()> {
        self.db.batch(&[
            SqlQuery { query: XML_BASELINE_LOCK_SQL, params: &[] },
            SqlQuery { query: BASELINE_SQL, params: &[id, key, brand] },
        ]).await?;
        Ok(())
    }

    pub(crate) async fn try_insert_firmware(
        &self,
        input: &FirmwareInput,
        target: &str,
        packages: &[FirmwarePackage],
        key: &str,
        source: SubmissionSource,
    ) -> Result<InsertAttempt> {
        let packages_json = serde_json::to_string(packages)
            .map_err(|error| Error::RustError(error.to_string()))?;
        let result = self.db.query(
            INSERT_SQL,
            &[
                &input.dev, &input.base, &input.cust, &input.preload, input.patch.as_deref().unwrap_or(""),
                &input.vendor, input.upgrade_or_erec.as_str(), target, key, &packages_json, input.brand,
                source.as_str(), "1",
            ],
        ).await?;
        let body = json_body(result)?;
        serde_json::from_str(&body).map_err(|error| Error::RustError(error.to_string()))
    }

    pub(crate) async fn insert_firmware_after_baseline(
        &self,
        input: &FirmwareInput,
        target: &str,
        packages: &[FirmwarePackage],
        key: &str,
        source: SubmissionSource,
    ) -> Result<Option<String>> {
        let packages_json = serde_json::to_string(packages)
            .map_err(|error| Error::RustError(error.to_string()))?;
        let mut results = self.db.batch(&[
            SqlQuery { query: XML_BASELINE_LOCK_SQL, params: &[] },
            SqlQuery {
                query: INSERT_SQL,
                params: &[
                    &input.dev, &input.base, &input.cust, &input.preload, input.patch.as_deref().unwrap_or(""),
                    &input.vendor, input.upgrade_or_erec.as_str(), target, key, &packages_json, input.brand,
                    source.as_str(), "0",
                ],
            },
        ]).await?;
        let result = results.pop()
            .ok_or_else(|| Error::RustError("database returned no insert result".into()))?;
        let body = json_body(result)?;
        let attempt: InsertAttempt = serde_json::from_str(&body)
            .map_err(|error| Error::RustError(error.to_string()))?;
        Ok(attempt.saved.map(|value| value.to_string()))
    }

    pub(crate) async fn existing_url_set_keys(
        &self,
        brand: &str,
        keys: &[String],
    ) -> Result<HashSet<String>> {
        let mut unique: Vec<&str> = Vec::with_capacity(keys.len());
        let mut seen = HashSet::with_capacity(keys.len());
        for key in keys {
            if seen.insert(key.as_str()) {
                unique.push(key.as_str());
            }
        }
        if unique.is_empty() {
            return Ok(HashSet::new());
        }
        let mut payloads = Vec::with_capacity(unique.len().div_ceil(URL_KEY_QUERY_CHUNK));
        for chunk in unique.chunks(URL_KEY_QUERY_CHUNK) {
            payloads.push(
                serde_json::to_string(chunk)
                    .map_err(|error| Error::RustError(error.to_string()))?,
            );
        }
        let mut existing = HashSet::new();
        for group in payloads.chunks(URL_KEY_BATCH_CHUNKS) {
            let results = if group.len() == 1 {
                vec![self.db.query(EXISTING_URL_KEYS_SQL, &[brand, &group[0]]).await?]
            } else {
                let param_pairs: Vec<[&str; 2]> = group.iter().map(|payload| [brand, payload.as_str()]).collect();
                let queries: Vec<SqlQuery<'_>> = param_pairs
                    .iter()
                    .map(|params| SqlQuery { query: EXISTING_URL_KEYS_SQL, params })
                    .collect();
                self.db.batch(&queries).await?
            };
            for result in results {
                for mut row in result.rows {
                    if let Some(key) = row.pop().flatten() {
                        existing.insert(key);
                    }
                }
            }
        }
        Ok(existing)
    }

    pub(crate) async fn refresh_seeds(&self) -> Result<RefreshSeeds> {
        let result = self.db.query(SEEDS_SQL, &[]).await?;
        let json = result.rows.first().and_then(|row| row.first()).and_then(Option::as_deref)
            .ok_or_else(|| Error::RustError("OTA refresh: missing seed query result".into()))?;
        serde_json::from_str(json)
            .map_err(|_| Error::RustError("OTA refresh: invalid seed query result".into()))
    }
}
