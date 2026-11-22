# FirmwareFetcher

Huawei / Honor firmware fetching site on Cloudflare Workers: Rust backend, Neon HTTP database, plain static frontend.

## Working Conventions

- Never stage, commit, push, merge, rebase, rewrite commits or deploy without explicit user authorization for that operation; "done" or "fix it" is not authorization, and do not ask whether to commit. Authorized commit messages end with a blank line then `Co-Authored-By: Claude <noreply@anthropic.com>`.
- No new explanatory comments, doc comments, docstrings, banners or placeholder comments unless the user asks, and then only in the specified scope. Keep existing valid comments, licenses, generated markers and machine metadata; fix only inaccuracies touched by the task.
- Replies follow [`../agent-workstyle/references/output.md`](../agent-workstyle/references/output.md): Simplified Chinese, no emoji, conclusion first, actual verification and its limits; no optional narration, commit suggestions or continuation offers.
- Confirm root, target files and existing changes first; preserve user changes, reuse existing implementations, stay in scope. Rules rest on user decisions, configuration and code facts, never guesses.
- This file holds stable facts and constraints only: update in place, no chat logs, temporary tasks, error logs or work summaries.

## Layout

- `Cargo.toml`: Rust 2021, `workers-rs` 0.8; do not infer a minimum Rust version. `package.json`: Wrangler 4, dev/deploy only; installs reuse the lockfile.
- `wrangler.toml`: source of truth for routes, compatibility date, build, assets, Cron and queues. `worker-build --release` → `build/index.js`; `public/` served by Workers Assets.
- `src/lib.rs`: Worker HTTP/Cron/queue exports only. `src/api.rs`: routing, JSON/form validation, responses. `src/domain.rs`: shared data, `UpdateAction`, pagination constants.
- `src/firmware/`: `mod.rs` per-brand query entry and `DeviceCredentials`; `huawei.rs` / `honor.rs` separate brand requests; `manual.rs` / `remote_zip.rs` three-package flow and ZIP Range reads; `fingerprint.rs` XML fingerprints.
- `src/storage/`: `Repository` is the only DB entry and owns all SQL, transaction locks and result parsing. `mod.rs` connection/migrations, `firmware.rs` list/writes/historical fingerprints/refresh seeds, `device_models.rs` mapping sync transaction, `neon.rs` HTTP transport.
- `src/service.rs`: save flow shared by version, manual and refresh submissions: verify new XML, initialize historical fingerprints, dedupe, write. No DB access before verification passes.
- `src/tasks/`: `mod.rs` Cron dispatch, `device_models.rs` CSV download/validation, `firmware.rs` refresh seeds, queue and Huawei queries. `src/notifications.rs`: Telegram.
- `migrations/`: idempotent SQL. `public/index.html|app.js|style.css`; `public/i18n.js` zh/en/ru dictionary, loaded before `app.js`. Pure page changes need no Rust.
- Boundaries: domain is independent of HTTP/DB; firmware adapters never touch the DB; storage never fetches XML, builds responses or notifies; API and refresh write through the shared save flow and notify only after success. When moving code, update callers and delete old paths; no forwarding.
- Secrets `HUAWEI_DEVICE_ID/HUAWEI_DEV_CERT/HUAWEI_KEY_ATT` and `HONOR_DEVICE_ID/HONOR_DEV_CERT/HONOR_KEY_ATT` (Honor values from `../honor_mock/app.py`), never in source. Missing/empty: submission returns 500 with no OTA request or save; refresh tasks fail and the queue retries.
- `../check_huawei_ota.py`: read-only OTA probe, reads no XML, prints 「返回固件（未验证XML）」, never claims a version is recorded.

## API and Storage

- `POST /requests` (JSON/form): requires `dev/base/cust/preload/upgrade_or_erec`; `brand` `huawei` (default) / `honor`; `vendor` default `all_cn`; empty/omitted `patch` = no patch. Saves only with available firmware and verified XML: 201, 409 duplicate, 422 no firmware, 502 XML failure; nothing saved on no firmware or network error.
- `UpdateAction::Upgrade/Recovery/Rollback` (`FromStr` / `as_str()`) = `upgrade` (incremental) / `recovery` (full) / `rollback` (beta) in HTTP, OTA and DB. Scheduled refresh uses only `upgrade/recovery`; `emulater` is a list filter / admin import value outside the enum.
- Huawei: `https://query.hicloud.com/BL/v1/VersionCheck`; `erecovery: true` (Huawei only) uses `https://query.hicloud.com/BL/v1/CheckAndAuthorize`. Both share the request builder (`ver: "2"`), verification and saving. Only `VersionCheck` sends `commonRules.checkType: "1"`; on `CheckAndAuthorize` it can select a different target.
- Honor: `update.platform.hihonorcloud.com/blversion/v1/version/check`, own certificate and request shape; parse numeric `status` and `result.checkResult/versionList/blVersionCheckResults`; exclude numeric-only changelog entries; never fabricate a missing PRELOAD link.
- `POST /requests/manual`: JSON only, `{"urls":[...]}`, ≤16 KiB, extra fields rejected. 201, 400 invalid, 415 non-JSON, 413 too large, 422 package failure, 502 XML failure, 409 duplicate; success shape matches version submission.
- `GET /requests`: both brands, `{ requests, total, page, perPage }`; records `id/brand/dev/vendor/targetBlVersionNumber/deviceName/created_at/upgrade_or_erec/packages[]`, `id` desc. `page` from 1, 50 per page. `q` fuzzy-matches device, channel, target version, package version number and Chinese model name. Valid `action=upgrade/recovery/rollback/emulater` filters type first; otherwise emulators are excluded unless `include_emulators=1`. Filters apply before pagination and count.
- `firmware_submissions`: brand, device, params, target version, time. `firmware_packages`: only `versionNumber` + URL, never raw upstream responses. OTA URLs completed to `<dir>/full/filelist.xml`; manual and emulator records keep direct ZIP links. Missing brand in history/imports = Huawei.
- `lgrp_num`: stored generated column from `base`, multi-digit (`GEM-LGRP3-CHN 207.0.0.100(SP10)` → `LGRP3`), `NULL` without a complete number. Writes omit it and never override it; tracks `base` changes automatically.
- Emulators: `upgrade_or_erec=emulater` (keep spelling), `vendor` `arm64/x86`, admin DB imports only. Public API returns 400 for JSON/form emulator submissions with no OTA request or save.

## XML Fingerprints

- Version, manual and refresh saves share content dedup. Huawei: fetch real `filelist.xml`, normalize, SHA-256 → `xml:v1:<64 hex>`. Honor CDN returns 403 to Workers egress (HTTP and HTTPS), so the backend never fetches Honor XML: hash each package's manifest URL path (scheme ignored, host fixed `update.hihonorcdn.com`) → `url:v1:<64 hex>`. Unique `(brand, content_key)` index blocks concurrent duplicates and isolates brands.
- Fields: `files/file` `spath/dpath/operation/sha256/size`; cloud packages `shainfo/size/subpath/package`; extension packages `filename/sha256/size`. Ignore formatting, order, checksum case and numeric leading zeros; keep package grouping and duplicate counts.
- Huawei version names, URLs, signatures and other metadata are excluded: changed files under the same version save; changed URL/name alone → 409 "An identical package already exists". Numeric-only version entries are changelogs: excluded, pseudo-XML links never requested. Honor: same package set at a new path saves again; in-place replacement under the same path goes undetected.
- History: the migration only clears old version-name signatures and keeps firmware. A submission locates only same-brand, same-target-version baseline-less records (name used only to locate) and compares their old XML; baselined records dedupe across names within the brand; no DB-wide scan of other versions. Missing/invalid historical XML: keep record, log ID, no name blocking, so re-save is possible; in-place upstream replacement is unrecoverable without a snapshot.
- New Huawei XML non-200, redirected, malformed or with bad fingerprint fields → 502, no save/notify, no fallback to name/URL dedup. Brand's trusted CDN hosts only; no credentials, ports or redirects; UA `curl/8.13.0`; streamed, 1 MiB per XML; ≤32 OTA entries per request.
- Fingerprints trust XML-declared checksums; full firmware is never downloaded; no claim of verifying full ZIP SHA-256 or upstream signatures.

## Manual Three-Package Boundary

- 3 required packages + optional updater, any order. Check `starts_with("http://update.dbankcdn.com/")` before parsing; ≤2048 bytes; direct ZIP under `full/`. Reject HTTPS, other hosts, credentials, ports, query, fragment, percent-encoding, backslashes, path traversal, duplicate package types.
- Names: `update_full_base.zip`, `update_full_cust_<model>_<channel>.zip`, `update_full_preload_<model>_<channel>_R<version>.zip`, optional `updater.zip`; CUST/PRELOAD model and channel must match.
- `VERSION.mbn` via HTTP Range (ZIP64, stored/deflate): request `bytes=0-0`, strictly validate `Content-Range` for total length, then explicit start/end ranges. Never `bytes=-N`: some CloudFront nodes ignore suffix ranges and return the whole package with 200.
- Package type, model/series, C code and PRELOAD R version must match file names; channel from the file name; E/P from the last segment of CUST/PRELOAD numeric versions (prefix may differ from BASE). Full version rebuilt by reversing the frontend parser, e.g. `BRE-AL80-LGRP9 207.0.0.111(SP10C00E111R1P2)`.
- Always saved as `recovery`, updater or not. Updater `update.tag` must be strictly `PKG_UPDATE_MODE=OTA`, `PKG_TYPE=PATCH`, `PKG_DATA_TYPE=FULL`, rejecting duplicate/unknown fields; labeled `updater.zip`, never a fabricated `patch00` or patch number; the tag does not prove version binding to the three packages.
- Downloads: no redirects; require 206, exact consistent `Content-Range`, no transfer compression; stream and cancel over limit. Validate ZIP directory, local headers, entry bounds, exact uncompressed size and CRC32 of the read entry; reject duplicate same-name targets, encryption, multi-disk, corruption. No claim about unread content or whole-package signatures.
- Per ZIP: ≤6 requests, ≤6 MiB downloaded; directory ≤4 MiB / 16384 entries; target ≤1 MiB compressed and uncompressed. Then fingerprint only the `file` entry in the sibling `filelist.xml` uniquely matching the ZIP name; 3- and 4-package sets stay distinct. DB access only after version and fingerprint checks pass.

## Frontend

- Submission brand defaults to Huawei and only switches the submit route. List, pagination, search, filters, expanded details and emulator toggle are shared; switching brand neither refetches nor resets them.
- "Submit a new version" has two native disclosure sections: "Submit by version" expanded initially and on every brand switch, still collapsible; "Submit three packages manually" collapsed by default, only URL input + submit button (no description or divider), hidden for Honor.
- `auto-parse` on both brands shares the full-version splitter (incl. model and LGRP); examples switch per brand. Honor example `WKL-AN80-LGRP1 11.0.0.111(SP7C00E110R101P2)` fills device and BASE/CUST/PRELOAD. Parsing never changes the patch toggle; Honor defaults to no patch; each brand keeps its own toggle state per page session; unchecked omits the field.
- `REGION_RULES` covers the `C00`–`C999` codes of the [Firm Finder submission form](https://ffusersubmission.byethost17.com/) plus demo `C735` / `DEMC735` → `demo_cn`, channels lowercase with underscores. Mapped China channels → BASE `-CHN`, other mapped → `-OVS`. Unmapped → `-CHN`, and the previous channel is restored after an automatic change unless the user edited it.
- Huawei "eRecovery" checkbox left of "Submit", unchecked by default; Submit/Enter uses eRecovery only when checked, package type and patch unchanged. Hidden and ignored for Honor.
- ≤760px: package type selector is one full-width row of three; eRecovery and Submit share the next row, each ≥44px tall; without the checkbox Submit spans the row.
- Header nav: "Haucet", "Pedal(Beta)" (`https://github.com/ljlVink/Pedal`). `.header-actions`: GitHub icon (`https://github.com/ljlvink/FirmwareFetcher`, accessible name "GitHub"), Telegram icon (`https://t.me/firmwarefetcher`, Simple Icons path, name "Telegram"), language selector, theme toggle. All links open new tabs. GitHub/Telegram/theme share the 36px bordered `.icon-button`. Language selector: visible native `<select>` with an inner-left Material `mdiTranslate` icon, matching the brand `<select>` height, radius and option colors. Narrow screens: brand + nav links on row 1, the four actions right-aligned on row 2, link text intact.
- Languages `en/zh/ru`: `localStorage.lang`, else first matching primary subtag of `navigator.languages`, else English; manual switch writes `localStorage.lang` and `<html lang>`. Static text via `data-i18n` / `data-i18n-placeholder|title|aria-label`, dynamic via `t()`; switching re-renders list, pagination and notice without refetch. Backend errors are English, shown untranslated. Quoted UI labels here are the English strings.
- List columns: Device (`deviceName` small under the code, hidden when unmatched), VENDOR, `targetBlVersionNumber`, package type, expand control; details hold package links and submission time. Types "Incremental", "Full", "Rollback (Beta)"; component PATCH markers never create a whole-package patch type.
- Emulator badges "ARM64 Emulator" / "X86 Emulator", styled like Full in both themes. "Show emulator packages" right of "Firmware list", unchecked by default; toggling goes to page 1, keeps search and type filter, sends `include_emulators=1` when checked.
- Honor "View XML": native HTTPS link, `target="_blank" rel="noopener noreferrer"`, to the original `full/filelist.xml`. On 403, users set UA `curl/8.13.0` for `update.hihonorcdn.com` with a UA extension; downloads need the same UA.
- Pages cannot set the UA: the frontend never reads, parses or proxies manifests cross-origin; no manifest API taking client-supplied upstream URLs; no `no-cors` fake success. The CDN sends no `Access-Control-Allow-Origin` (curl with an Origin header proves nothing); direct navigation needs no read permission. The backend reads Huawei manifests only for submission verification and baseline initialization.

## Scheduled Tasks and Model Mapping

- `wrangler.toml` Cron: `0 16 * * *` daily Huawei OTA refresh (00:00 Beijing), `0 0 * * *` model sync (08:00). Dispatch by expression; unknown expressions fail. Cron/code changes need redeploy; upstream data applies on the next sync.
- Model sync downloads [MobileModels-csv/models.csv](https://raw.githubusercontent.com/KHwang9883/MobileModels-csv/main/models.csv), keeping Huawei/Honor China and global sources, multiple names and variants, deduped by the four-column key; reuses `NEON_CONN_STRING`, no external scheduler, secret or migration. This Worker sync is the only one (old Python script and `npm run sync:models` removed).
- DB is touched only after the whole CSV validates (first run may create tables via migrations). HTTP failure, missing/duplicate columns, bad rows, source/brand mismatch, NUL characters or missing brand fail the task with the DB untouched.
- The Neon batch transaction holds 003's transaction lock, adds missing mappings and deletes `huawei_*` / `honor_*` mappings removed or corrected upstream; other sources, firmware and links are kept. Any failure rolls back; re-running a snapshot adds/removes 0. Success log has valid, added/removed and per-source counts; errors surface as Cron failures in Workers Logs and execution history.
- The `names` CTE in `LIST_SQL` (`src/storage/firmware.rs`) drives model display and name search: case-insensitive, `HUAWEI ` prefix stripped, best tier only, ties joined ` / `. Tiers: exact code (cross-brand, same brand first; Huawei `OXF-AN10` → 荣耀 V30 PRO); submitted code one trailing char longer (`MNA-AL00A` → `MNA-AL00`); table code one trailing char longer (`KLE-AL00` → `KLE-AL00U`); `CN` suffix (`NIZ-AL00CN` → `NIZ-AL00`); same prefix family before `-` (`TGR-W20` → `TGR-W00/W10`). Non-exact tiers are same-brand only. Family rank: identical network letters, same first letter, any, then smallest numeric distance; `*_cn` sources win remaining ties. Emulator categories (no `-`) never family-match.

## Telegram

- After a saved Huawei/Honor version or manual submission, `schedule` (`src/notifications.rs`, `Context::wait_until`) sends one message per new firmware; background refresh calls `send` directly under queue rate limiting. English plain text, previews off: brand, model, channel, target version, package type only; no links or bot signature.
- No notification for duplicates (incl. unique-index conflicts), verification/write failures, model sync or admin SQL imports. Best effort: missing config, network error, non-200 or `ok:false` only log a redacted error; never block success, roll back, retry or resend.
- `TG_GROUP` (username with optional `@`, or numeric chat ID) and `TG_BOT_KEY` (secret, backend only, never in frontend, logs or repo); the bot needs posting permission.

## Migrations and Deployment

- `NEON_CONN_STRING` (currently from `../remote_neondb`); SQL only via `src/storage/neon.rs` to `https://api.<region>.aws.neon.tech/sql` with header `Neon-Connection-String`; no Postgres crate.
- Idempotent migrations run in order on first DB access: 001 tables + old data; 002 brand default, constraints, `content_key`, old index removal; 003 device model table, seeding 1586 Huawei rows into an empty table; 004 whole-package `patch` → `recovery` (links kept), current type constraint incl. `rollback`, emulator arch constraint; 005 `lgrp_num` under the firmware migration lock; 006 same lock, allows baseline-less `content_key=NULL`, clears only non-XML signatures, keeps rows, the `(brand, content_key)` unique index and the history query index. 001/002 never delete, merge or backfill signatures by old five-tuple or version name; cold starts never rewrite XML fingerprints.
- Deploy the Worker together with revised 002/006 so an old Worker never writes under switched dedup rules.
- Keep route `firmwarefetcher.ljlvink.top/*` and `keep_vars = true`. `.dev.vars` is not uploaded; reuse same-named online vars/secrets; never create a secret where a same-named text variable exists. Deploys and secret changes each need authorization.

## Development and Verification

- `npm ci`; `npm run dev` → `http://127.0.0.1:18787`. Put an isolated test `NEON_CONN_STRING` in the existing `.dev.vars`, leaving other settings and user secrets intact.
- Rust: `cargo test`, `worker-build --release`, then run the Worker on affected paths. Frontend: verify in a real browser; for brand changes check switching, submission and the Honor XML new-tab link (real Honor XML needs the manual UA).
- Writes go only to an isolated DB, never production. Save/schedule changes cover both brands, three-package parsing, fingerprint dedup, Cron dispatch, queue consumption and notification boundaries; external services may be intercepted in the local Worker. Report test, build, simulated and real results separately with uncovered scope; reuse results when code, inputs and environment are unchanged, without weakening checks.
- Local model sync: `npx wrangler dev --test-scheduled`, then `curl --fail-with-body 'http://127.0.0.1:18787/__scheduled?cron=0+0+*+*+*'` and read Wrangler logs. This really writes to the DB; `/__scheduled` is local only.
- With authorization: `npm run deploy`; secrets via `npx wrangler secret put <name>` (`NEON_CONN_STRING`, `TG_GROUP`, `TG_BOT_KEY`, the 6 device credentials) or `npx wrangler secret bulk` from JSON. Rust and Worker config changes apply only after deploy.

## Files and Cleanup

- Clean only temporary content this task created and confirmed unused, after checking path, ownership and references; keep user, untracked and ignored files; extra cleanup needs authorization. Stop only processes this task started.
- Keep source, valid tests, migrations, config, lockfiles, artifacts and data. `node_modules/`, `build/`, `target/` are regenerable, which does not authorize deleting them. `.dev.vars` (local secrets) and `.wrangler/` (local state) are ignored but never deleted as caches.
