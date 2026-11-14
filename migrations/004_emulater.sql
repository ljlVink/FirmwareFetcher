DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));

    ALTER TABLE firmware_submissions
        DROP CONSTRAINT IF EXISTS firmware_submissions_upgrade_or_erec_check;

    -- updater 的 PATCH 标记只描述该组件，旧版误标的整套三包仍是全量包。
    UPDATE firmware_submissions
        SET upgrade_or_erec = 'recovery'
        WHERE upgrade_or_erec = 'patch';

    ALTER TABLE firmware_submissions
        ADD CONSTRAINT firmware_submissions_upgrade_or_erec_check
        CHECK (upgrade_or_erec IN ('upgrade', 'recovery', 'emulater'));

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conrelid = 'firmware_submissions'::regclass
          AND conname = 'firmware_submissions_emulater_vendor_check'
    ) THEN
        ALTER TABLE firmware_submissions
            ADD CONSTRAINT firmware_submissions_emulater_vendor_check
            CHECK (upgrade_or_erec <> 'emulater' OR vendor IN ('x86', 'arm64'));
    END IF;
END
$migration$;
