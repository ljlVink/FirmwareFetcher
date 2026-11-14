# FirmwareFetcher

NeonDB + Cloudflare Workers + Telegram Bot

This project does not contain any secrets required to call the official server.

## Requirements

- A [Neon](https://neon.tech) Postgres database. The Worker talks to Neon over its serverless HTTP SQL endpoint (`https://api.<region>.aws.neon.tech/sql`).
- The database connection string in the `NEON_CONN_STRING` secret, copied from the Neon console, e.g. `postgresql://<user>:<password>@ep-xxx.<region>.aws.neon.tech/<db>?sslmode=require`. Tables, indexes and seed data are created by the idempotent migrations in `migrations/` on first database access.
- Device credentials in the secrets `HUAWEI_DEVICE_ID`, `HUAWEI_DEV_CERT`, `HUAWEI_KEY_ATT`, `HONOR_DEVICE_ID`, `HONOR_DEV_CERT`, `HONOR_KEY_ATT`.
- Put the secrets above in `.dev.vars`
