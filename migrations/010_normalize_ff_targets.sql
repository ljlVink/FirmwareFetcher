DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));

    UPDATE firmware_submissions
    SET target_bl_version_number = replace(target_bl_version_number, ')(', '')
    WHERE target_bl_version_number ~ '\(SP[0-9]+\)\([A-Z]*C[0-9]+E[0-9]+R[0-9]+P[0-9]+\)$';
END
$migration$;
