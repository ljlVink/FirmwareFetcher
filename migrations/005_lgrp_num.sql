DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));

    -- Stored generation fills existing rows and keeps inserts/base updates in sync.
    -- Bases without a complete LGRP token (including emulator imports) yield NULL.
    ALTER TABLE firmware_submissions
        ADD COLUMN IF NOT EXISTS lgrp_num TEXT GENERATED ALWAYS AS (
            substring(base FROM '(?:^|-)(LGRP[0-9]+)(?:-|[[:space:]]|$)')
        ) STORED;
END
$migration$;
