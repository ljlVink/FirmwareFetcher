DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:xml-baseline'));
    LOCK TABLE firmware_submissions, firmware_packages IN SHARE ROW EXCLUSIVE MODE;

    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS url_set_key TEXT;

    WITH computed AS (
        SELECT s.id, s.brand, s.content_key,
               'urlset:v1:' || encode(
                   sha256(
                       convert_to(
                           'FirmwareFetcher URL set v1' || E'\n' ||
                           string_agg(octet_length(u.url)::text || ':' || u.url, E'\n' ORDER BY u.url COLLATE "C"),
                           'UTF8'
                       )
                   ),
                   'hex'
               ) AS url_set_key
        FROM firmware_submissions s
        JOIN (
            SELECT DISTINCT submission_id, url
            FROM firmware_packages
            WHERE btrim(version_number) !~ '^[0-9]{9}$'
        ) u ON u.submission_id = s.id
        GROUP BY s.id, s.brand, s.content_key
    ), duplicates AS (
        SELECT id, row_number() OVER (
            PARTITION BY brand, url_set_key ORDER BY content_key IS NULL, id
        ) AS position
        FROM computed
    )
    DELETE FROM firmware_submissions s USING duplicates d
    WHERE s.id = d.id AND d.position > 1;

    WITH computed AS (
        SELECT s.id,
               'urlset:v1:' || encode(
                   sha256(
                       convert_to(
                           'FirmwareFetcher URL set v1' || E'\n' ||
                           string_agg(octet_length(u.url)::text || ':' || u.url, E'\n' ORDER BY u.url COLLATE "C"),
                           'UTF8'
                       )
                   ),
                   'hex'
               ) AS url_set_key
        FROM firmware_submissions s
        JOIN (
            SELECT DISTINCT submission_id, url
            FROM firmware_packages
            WHERE btrim(version_number) !~ '^[0-9]{9}$'
        ) u ON u.submission_id = s.id
        WHERE s.url_set_key IS NULL
        GROUP BY s.id
    )
    UPDATE firmware_submissions s
    SET url_set_key = c.url_set_key
    FROM computed c
    WHERE s.id = c.id AND s.url_set_key IS NULL;

    CREATE UNIQUE INDEX IF NOT EXISTS firmware_submissions_url_set_key
      ON firmware_submissions (brand, url_set_key);
    CREATE INDEX IF NOT EXISTS firmware_submissions_huawei_recovery_seeds
      ON firmware_submissions (id)
      WHERE brand = 'huawei' AND upgrade_or_erec = 'recovery' AND lgrp_num IS NOT NULL;
    CREATE INDEX IF NOT EXISTS firmware_submissions_huawei_dev_action
      ON firmware_submissions (dev, upgrade_or_erec)
      WHERE brand = 'huawei' AND upgrade_or_erec IN ('upgrade', 'recovery');
END
$migration$;
