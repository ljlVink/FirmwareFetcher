DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));

    -- 历史库没有 XML 快照，版本名签名不能伪装成内容指纹。保留全部记录。
    ALTER TABLE firmware_submissions ALTER COLUMN content_key DROP NOT NULL;
    UPDATE firmware_submissions SET content_key = NULL
      WHERE content_key IS NOT NULL AND content_key !~ '^xml:v1:[0-9a-f]{64}$';

    CREATE UNIQUE INDEX IF NOT EXISTS firmware_submissions_content_key
      ON firmware_submissions (brand, content_key);
    CREATE INDEX IF NOT EXISTS firmware_submissions_xml_baseline
      ON firmware_submissions (brand, target_bl_version_number)
      WHERE content_key IS NULL AND upgrade_or_erec <> 'emulater';
END
$migration$;
