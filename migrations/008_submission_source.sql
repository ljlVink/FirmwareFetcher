DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));

    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS source TEXT
        CHECK (source IN ('manual_version', 'erec_three_package', 'auto_refresh', 'ffusersubmission', 'admin_manual'));

    UPDATE firmware_submissions SET source = 'admin_manual'
    WHERE source IS NULL AND upgrade_or_erec = 'emulater';
END
$migration$;
