DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));

    CREATE TABLE IF NOT EXISTS firmware_submissions (
        id BIGSERIAL PRIMARY KEY,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        dev TEXT NOT NULL,
        base TEXT NOT NULL,
        cust TEXT NOT NULL,
        preload TEXT NOT NULL,
        patch TEXT NOT NULL,
        target_bl_version_number TEXT NOT NULL CHECK (btrim(target_bl_version_number) <> ''),
        vendor TEXT NOT NULL,
        upgrade_or_erec TEXT NOT NULL CHECK (upgrade_or_erec IN ('upgrade', 'recovery'))
    );

    CREATE TABLE IF NOT EXISTS firmware_packages (
        submission_id BIGINT NOT NULL REFERENCES firmware_submissions(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        version_number TEXT NOT NULL CHECK (btrim(version_number) <> ''),
        url TEXT NOT NULL CHECK (url ~ '^https?://'),
        PRIMARY KEY (submission_id, position)
    );

    -- Idempotent for databases created by earlier versions of this migration.
    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS base TEXT;
    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS cust TEXT;
    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS preload TEXT;
    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS patch TEXT;
    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS target_bl_version_number TEXT;

    IF to_regclass('public.ota_requests') IS NOT NULL THEN
        INSERT INTO firmware_submissions (id, created_at, dev, base, cust, preload, patch,
                                          target_bl_version_number, vendor, upgrade_or_erec)
        SELECT r.id, r.created_at, r.dev, r.base, r.cust, r.preload, r.patch,
               r.last_result#>>'{blVersionCheckResults,0,blVersionInfo,targetBlVersionNumber}',
               r.vendor, r.upgrade_or_erec
        FROM ota_requests r
        WHERE r.last_result->>'status' = '0'
          AND nullif(btrim(r.last_result#>>'{blVersionCheckResults,0,blVersionInfo,targetBlVersionNumber}'), '') IS NOT NULL
          AND EXISTS (
              SELECT 1
              FROM jsonb_array_elements(
                  CASE WHEN jsonb_typeof(r.last_result->'versionList') = 'array'
                       THEN r.last_result->'versionList' ELSE '[]'::jsonb END
              ) AS entry(package)
              WHERE jsonb_typeof(entry.package->'versionNumber') = 'string'
                AND btrim(entry.package->>'versionNumber') <> ''
                AND jsonb_typeof(entry.package->'url') = 'string'
                AND entry.package->>'url' ~ '^https?://'
          )
        ON CONFLICT (id) DO NOTHING;

        INSERT INTO firmware_packages (submission_id, position, version_number, url)
        SELECT r.id, entry.position::integer, btrim(entry.package->>'versionNumber'),
            CASE WHEN split_part(split_part(entry.package->>'url', '?', 1), '#', 1) LIKE '%/full/filelist.xml'
                 THEN entry.package->>'url'
                 ELSE rtrim(split_part(split_part(entry.package->>'url', '?', 1), '#', 1), '/')
                      || '/full/filelist.xml'
                      || coalesce(substring(entry.package->>'url' FROM '([?#].*)$'), '')
            END
        FROM ota_requests r
        JOIN firmware_submissions s ON s.id = r.id
        CROSS JOIN LATERAL jsonb_array_elements(
            CASE WHEN jsonb_typeof(r.last_result->'versionList') = 'array'
                 THEN r.last_result->'versionList' ELSE '[]'::jsonb END
        ) WITH ORDINALITY AS entry(package, position)
        WHERE r.last_result->>'status' = '0'
          AND jsonb_typeof(entry.package->'versionNumber') = 'string'
          AND btrim(entry.package->>'versionNumber') <> ''
          AND jsonb_typeof(entry.package->'url') = 'string'
          AND entry.package->>'url' ~ '^https?://'
        ON CONFLICT (submission_id, position) DO NOTHING;

        PERFORM setval(
            pg_get_serial_sequence('firmware_submissions', 'id'),
            coalesce((SELECT max(id) FROM firmware_submissions), 1),
            EXISTS (SELECT 1 FROM firmware_submissions)
        );
        DROP TABLE ota_requests;
    END IF;


    ALTER TABLE firmware_submissions ALTER COLUMN base SET NOT NULL;
    ALTER TABLE firmware_submissions ALTER COLUMN cust SET NOT NULL;
    ALTER TABLE firmware_submissions ALTER COLUMN preload SET NOT NULL;
    ALTER TABLE firmware_submissions ALTER COLUMN patch SET NOT NULL;
    ALTER TABLE firmware_submissions ALTER COLUMN target_bl_version_number SET NOT NULL;

END
$migration$;
