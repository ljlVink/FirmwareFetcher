DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:xml-baseline'));
    LOCK TABLE firmware_submissions, firmware_packages IN SHARE ROW EXCLUSIVE MODE;

    DELETE FROM firmware_packages WHERE btrim(version_number) ~ '^[0-9]{9}$';

    DELETE FROM firmware_submissions s
    WHERE NOT EXISTS (SELECT 1 FROM firmware_packages p WHERE p.submission_id = s.id);

    WITH link_sets AS (
        SELECT s.id, s.brand, s.content_key, array_agg(DISTINCT p.url ORDER BY p.url) AS urls
        FROM firmware_submissions s
        JOIN firmware_packages p ON p.submission_id = s.id
        GROUP BY s.id
    ), duplicates AS (
        SELECT id, row_number() OVER (
            PARTITION BY brand, urls ORDER BY content_key IS NULL, id
        ) AS position
        FROM link_sets
    )
    DELETE FROM firmware_submissions s USING duplicates d
    WHERE s.id = d.id AND d.position > 1;
END
$migration$;
