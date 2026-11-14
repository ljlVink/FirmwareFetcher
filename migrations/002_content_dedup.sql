DO $migration$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('cfworker-rs:verified-firmware-schema'));

    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS content_key TEXT;
    ALTER TABLE firmware_submissions ADD COLUMN IF NOT EXISTS brand TEXT NOT NULL
      DEFAULT 'huawei' CHECK (brand IN ('huawei', 'honor'));

    -- 仅保留字段初始化；旧版按版本名回填、合并历史记录的步骤已废止。
    -- XML 指纹的清理和唯一索引由 006 负责，冷启动不得重写已有 XML 指纹。
    DROP INDEX IF EXISTS firmware_submissions_dedup_key;
    DROP INDEX IF EXISTS firmware_submissions_brand_id;
END
$migration$;
