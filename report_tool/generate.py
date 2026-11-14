#!/usr/bin/env python3

import argparse
from collections import Counter
from datetime import date, datetime
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parent
BEIJING = ZoneInfo("Asia/Shanghai")
SYSTEMS = {"207": "鸿蒙 7", "206": "鸿蒙 6", "104": "鸿蒙 4"}

def parse_date(value):
    try:
        return date.fromisoformat(value)
    except ValueError:
        raise argparse.ArgumentTypeError("Date format should be YYYY-MM-DD") from None

sqlcmd = '''
WITH day_records AS (
    SELECT s.* FROM firmware_submissions s
    WHERE s.created_at >= ($1::date::timestamp AT TIME ZONE 'Asia/Shanghai')
      AND s.created_at < (($1::date + 1)::timestamp AT TIME ZONE 'Asia/Shanghai')
      AND s.upgrade_or_erec IN ('recovery', 'upgrade')
), details AS (
    SELECT s.id, s.brand, s.dev, dm.model_name, s.vendor, s.upgrade_or_erec,
        s.target_bl_version_number,
        to_char(s.created_at AT TIME ZONE 'Asia/Shanghai', 'YYYY-MM-DD HH24:MI:SS') AS created_at_beijing
    FROM day_records s LEFT JOIN LATERAL (
        SELECT coalesce(
            (SELECT string_agg(DISTINCT cn.model_name, ' / ' ORDER BY cn.model_name)
               FROM device_models cn
               WHERE cn.model = s.dev AND cn.source_file = s.brand || '_cn'),
            (SELECT string_agg(DISTINCT a.model_name, ' / ' ORDER BY a.model_name)
               FROM device_models a
               WHERE a.model = s.dev AND starts_with(a.source_file, s.brand || '_')),
            (SELECT string_agg(DISTINCT c2.model_name, ' / ' ORDER BY c2.model_name)
               FROM device_models c2
               WHERE s.dev LIKE c2.model || '_' AND c2.source_file = s.brand || '_cn'),
            (SELECT string_agg(DISTINCT a2.model_name, ' / ' ORDER BY a2.model_name)
               FROM device_models a2
               WHERE s.dev LIKE a2.model || '_' AND starts_with(a2.source_file, s.brand || '_')),
            (SELECT string_agg(DISTINCT c3.model_name, ' / ' ORDER BY c3.model_name)
               FROM device_models c3
               WHERE s.dev = c3.model || 'CN' AND c3.source_file = s.brand || '_cn'),
            (SELECT string_agg(DISTINCT a3.model_name, ' / ' ORDER BY a3.model_name)
               FROM device_models a3
               WHERE s.dev = a3.model || 'CN' AND starts_with(a3.source_file, s.brand || '_'))
        ) AS model_name
    ) dm ON true
)
SELECT json_build_object(
    'report_date', $1::text, 'timezone', 'Asia/Shanghai',
    'queried_at_beijing', to_char(now() AT TIME ZONE 'Asia/Shanghai', 'YYYY-MM-DD HH24:MI:SS'),
    'records', (SELECT coalesce(json_agg(row_to_json(d) ORDER BY d.id DESC), '[]'::json) FROM details d)
)::text;
'''

def load_env(path):
    if not path.is_file():
        return
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
            value = value[1:-1]
        os.environ.setdefault(key.strip(), value)


def require_env(name):
    value = os.environ.get(name)
    if not value:
        raise RuntimeError(f"Missing {name}, please set it in .env")
    return value


def fetch_records(report_date):
    request = urllib.request.Request(
        require_env("NEON_CONNECTION_ENDPOINT"),
        data=json.dumps({"queries": [
            {"query": "SELECT current_setting('transaction_read_only')", "params": []},
            {"query": sqlcmd, "params": [report_date.isoformat()]},
        ]}).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Neon-Connection-String": require_env("NEON_CONNECTION_STRING"),
            "Neon-Raw-Text-Output": "true",
            "Neon-Array-Mode": "true",
            "Neon-Batch-Read-Only": "true",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=90) as response:
            result = json.load(response)
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"Database check failed, HTTP {error.code}") from None
    except urllib.error.URLError:
        raise RuntimeError("Cannot connect to database, please check network and Neon configuration") from None
    try:
        if result["results"][0]["rows"][0][0] != "on":
            raise RuntimeError("Database is not in read-only mode, generation stopped")
        return json.loads(result["results"][1]["rows"][0][0])
    except (KeyError, IndexError, TypeError, json.JSONDecodeError):
        raise RuntimeError("Invalid data format returned by database") from None


def summarize(data):
    records = data["records"]
    for record in records:
        record["display_model_name"] = (
            record["model_name"]
            or "未收录机型"
        ).replace("HUAWEI ", "")
        match = re.search(r"\s(\d+)\.", record["target_bl_version_number"])
        record["system_prefix"] = match.group(1) if match else "unknown"
    counts = Counter(record["upgrade_or_erec"] for record in records)
    data["counts"] = {
        "firmware_records": len(records),
        "full": counts["recovery"],
        "delta": counts["upgrade"],
        "device_codes": len({(r["brand"], r["dev"]) for r in records}),
    }
    data["system_summary"] = []
    prefixes = list(SYSTEMS) + sorted({r["system_prefix"] for r in records} - SYSTEMS.keys())
    for prefix in prefixes:
        group = [r for r in records if r["system_prefix"] == prefix]
        actions = Counter(r["upgrade_or_erec"] for r in group)
        data["system_summary"].append({
            "prefix": prefix,
            "label": SYSTEMS.get(prefix, "其他" if prefix == "unknown" else f"其他（{prefix}）"),
            "full": actions["recovery"],
            "delta": actions["upgrade"],
            "models": len({(r["brand"], r["dev"]) for r in group}),
        })
    return data


def typst_command(typst, args):
    result = subprocess.run([typst, *args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("Typst execution failed:\n" + result.stderr.strip())
    return result.stdout


def generate(data, output, typst):
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".firmware-report-", dir=output.parent) as temporary:
        staging = Path(temporary)
        (staging / "firmware-data.json").write_text(
            json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8",
        )
        source = staging / "firmware-daily.typ"
        shutil.copyfile(ROOT / "report.typ", source)
        available_fonts = typst_command(typst, ["fonts"])
        if "Yozai" not in available_fonts.splitlines():
            raise RuntimeError("Missing system font Yozai, please install the Yozai font first")
        layout = json.loads(typst_command(
            typst, ["eval", "query(<report-layout>).map(it => it.value)", "--in", str(source)],
        ))
        expected_pages = max(1, (layout[0]["rows"] + 29) // 30) if len(layout) == 1 else 0
        if len(layout) != 1 or layout[0]["pages"] != expected_pages:
            raise RuntimeError("Page layout is abnormal, please adjust report.typ to ensure no more than 30 entries per page")
        image_name = "firmware-daily.png" if expected_pages == 1 else "firmware-daily-{p}.png"
        typst_command(typst, [
            "compile", "--format", "png", "--ppi", "350", str(source),
            str(staging / image_name),
        ])
        images = ["firmware-daily.png"] if expected_pages == 1 else [
            f"firmware-daily-{page}.png" for page in range(1, expected_pages + 1)
        ]
        output.mkdir(parents=True, exist_ok=True)
        for filename in ("firmware-data.json", "firmware-daily.typ", *images):
            os.replace(staging / filename, output / filename)
        for previous in output.glob("firmware-daily*.png"):
            if re.fullmatch(r"firmware-daily(?:-\d+)?\.png", previous.name) and previous.name not in images:
                previous.unlink()
    return layout[0]["rows"]


def main():
    parser = argparse.ArgumentParser(description="Generate daily firmware report")
    parser.add_argument("--date", type=parse_date, default=datetime.now(BEIJING).date(), help="Beijing time date")
    parser.add_argument("--output", type=Path, help="Output directory, default is reports/YYYY-MM-DD")
    args = parser.parse_args()
    try:
        load_env(ROOT / ".env")
        typst = shutil.which("typst")
        if not typst:
            raise RuntimeError("Cannot find typst, please install the Typst command-line tool first")
        data = summarize(fetch_records(args.date))
        output = (args.output or ROOT / args.date.isoformat()).resolve()
        rows = generate(data, output, typst)
    except (OSError, ValueError, RuntimeError) as error:
        print(f"Failed to generate report: {error}", file=sys.stderr)
        return 1
    counts = data["counts"]
    print(f"{args.date}: {counts['firmware_records']} sets, full {counts['full']} sets, delta {counts['delta']} sets, {counts['device_codes']} device models; details merged into {rows} rows.")
    print(f"Query time: {data['queried_at_beijing']}")
    pages = max(1, (rows + 29) // 30)
    if pages == 1:
        print(f"Image: {output / 'firmware-daily.png'}")
    else:
        for page in range(1, pages + 1):
            print(f"Page {page}: {output / f'firmware-daily-{page}.png'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
