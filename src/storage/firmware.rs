use worker::{Error, Result};

use crate::domain::{FirmwareInput, FirmwarePackage, LegacyFirmware, RefreshSeed};

use super::neon::{SqlQuery, SqlResult};
use super::Repository;

const LIST_SQL: &str = r#"
WITH devices AS (
    SELECT brand, dev, code, split_part(code, '-', 1) AS family,
           substring(code FROM '^[A-Z0-9]{3,}-([A-Z]+)[0-9]') AS letters,
           substring(code FROM '^[A-Z0-9]{3,}-[A-Z]+([0-9]{1,6})')::int AS digits
    FROM (SELECT DISTINCT brand, dev, upper(dev) AS code FROM firmware_submissions) d
), models AS (
    SELECT model_name, brand, source_file = brand || '_cn' AS cn, code, split_part(code, '-', 1) AS family,
           substring(code FROM '^[A-Z0-9]{3,}-([A-Z]+)[0-9]') AS letters,
           substring(code FROM '^[A-Z0-9]{3,}-[A-Z]+([0-9]{1,6})')::int AS digits
    FROM (SELECT model_name, source_file, split_part(source_file, '_', 1) AS brand,
                 upper(regexp_replace(model, '^.*\s', '')) AS code
          FROM device_models
          WHERE starts_with(source_file, 'huawei_') OR starts_with(source_file, 'honor_')) m
),
-- 机型名候选取最优一档：精确（可跨品牌，同品牌优先）> 提交型号多一位 > 机型表多一位 > CN 尾缀 > 同前缀系列；
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
    JOIN models m ON m.family = d.family
    CROSS JOIN LATERAL (SELECT CASE
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
        'dev', s.dev, 'brand', s.brand, 'vendor', s.vendor, 'upgrade_or_erec', s.upgrade_or_erec,
        'targetBlVersionNumber', s.target_bl_version_number,
        'deviceName', s.model_name,
        'packages', s.packages
    ) ORDER BY s.id DESC), '[]'::jsonb)
)::text
FROM (
    SELECT s.id, s.created_at, s.dev, s.brand, s.vendor, s.upgrade_or_erec, s.target_bl_version_number,
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
WITH saved_submission AS (
    INSERT INTO firmware_submissions
        (dev, base, cust, preload, patch, vendor, upgrade_or_erec, target_bl_version_number, content_key, brand)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $11)
    ON CONFLICT (brand, content_key) DO NOTHING
    RETURNING id, created_at, dev, brand, vendor, upgrade_or_erec, target_bl_version_number
), saved_packages AS (
    INSERT INTO firmware_packages (submission_id, position, version_number, url)
    SELECT s.id, entry.position::integer, entry.package->>'versionNumber', entry.package->>'url'
    FROM saved_submission s
    CROSS JOIN LATERAL jsonb_array_elements($10::jsonb) WITH ORDINALITY AS entry(package, position)
    RETURNING position, version_number, url
)
SELECT jsonb_build_object(
    'id', s.id, 'created_at', to_char(s.created_at, 'YYYY-MM-DD HH24:MI:SS'),
    'dev', s.dev, 'brand', s.brand, 'vendor', s.vendor, 'upgrade_or_erec', s.upgrade_or_erec,
    'targetBlVersionNumber', s.target_bl_version_number,
    'packages', (SELECT jsonb_agg(jsonb_build_object('versionNumber', version_number, 'url', url)
                               ORDER BY position) FROM saved_packages)
)::text
FROM saved_submission s
"#;
const LEGACY_SQL: &str = r#"
SELECT coalesce(jsonb_agg(row_to_json(record)), '[]'::jsonb)::text
FROM (
    SELECT s.id::text AS id,
        jsonb_agg(jsonb_build_object('versionNumber', p.version_number, 'url', p.url)
                  ORDER BY p.position) AS packages
    FROM firmware_submissions s
    JOIN firmware_packages p ON p.submission_id = s.id
    WHERE s.brand = $1 AND s.target_bl_version_number = $2 AND s.content_key IS NULL
      AND s.upgrade_or_erec <> 'emulater'
    GROUP BY s.id ORDER BY s.id
) record
"#;
const BASELINE_SQL: &str = r#"
UPDATE firmware_submissions SET content_key = $2
WHERE id = $1::bigint AND content_key IS NULL
  AND NOT EXISTS (
      SELECT 1 FROM firmware_submissions WHERE brand = $3 AND content_key = $2
  )
"#;
const XML_BASELINE_LOCK_SQL: &str = "SELECT pg_advisory_xact_lock(hashtext('cfworker-rs:xml-baseline'))";
const SEEDS_SQL: &str = r#"
SELECT coalesce(jsonb_agg(jsonb_build_object(
    'id', s.id, 'dev', s.dev, 'vendor', s.vendor,
    'action', s.upgrade_or_erec, 'lgrp', s.lgrp_num,
    'packages', (SELECT jsonb_agg(p.version_number ORDER BY p.position)
                 FROM firmware_packages p WHERE p.submission_id = s.id)
) ORDER BY s.id), '[]'::jsonb)::text
FROM firmware_submissions s
WHERE s.brand = 'huawei' AND s.upgrade_or_erec IN ('upgrade', 'recovery')
"#;

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

    pub(crate) async fn legacy_firmware(&self, brand: &str, target: &str) -> Result<Vec<LegacyFirmware>> {
        let result = self.db.query(LEGACY_SQL, &[brand, target]).await?;
        let json = result.rows.first().and_then(|row| row.first()).and_then(Option::as_deref)
            .ok_or_else(|| Error::RustError("database returned no legacy records".into()))?;
        serde_json::from_str(json).map_err(|error| Error::RustError(error.to_string()))
    }

    pub(crate) async fn set_legacy_fingerprint(&self, id: &str, key: &str, brand: &str) -> Result<()> {
        self.db.batch(&[
            SqlQuery { query: XML_BASELINE_LOCK_SQL, params: &[] },
            SqlQuery { query: BASELINE_SQL, params: &[id, key, brand] },
        ]).await?;
        Ok(())
    }

    pub(crate) async fn insert_firmware(
        &self,
        input: &FirmwareInput,
        target: &str,
        packages: &[FirmwarePackage],
        key: &str,
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
                ],
            },
        ]).await?;
        let result = results.pop()
            .ok_or_else(|| Error::RustError("database returned no insert result".into()))?;
        if result.rows.is_empty() {
            Ok(None)
        } else {
            json_body(result).map(Some)
        }
    }

    pub(crate) async fn refresh_seeds(&self) -> Result<Vec<RefreshSeed>> {
        let result = self.db.query(SEEDS_SQL, &[]).await?;
        let json = result.rows.first().and_then(|row| row.first()).and_then(Option::as_deref)
            .ok_or_else(|| Error::RustError("OTA refresh: missing seed query result".into()))?;
        serde_json::from_str(json)
            .map_err(|_| Error::RustError("OTA refresh: invalid seed query result".into()))
    }
}
