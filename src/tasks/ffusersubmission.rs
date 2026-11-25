use std::time::Duration;

use aes::cipher::{BlockDecrypt, KeyInit};
use futures_util::StreamExt;
use serde::{de::IgnoredAny, Deserialize, Serialize};
use worker::{console_log, Delay, Env, Error, Fetch, Headers, Method, Request, RequestInit, RequestRedirect, Result};

use crate::domain::{FirmwareInput, FirmwarePackage, SubmissionSource, UpdateAction};
use crate::service::{self, SaveError};
use crate::{firmware, notifications};

pub(super) const CRON: &str = "30 16 * * *";
const DATABASE: &str = "https://ffusersubmission.byethost17.com/firmwards.hash";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const MAX_DOWNLOAD: usize = 4 * 1024 * 1024;
const MAX_DECODED: usize = 32 * 1024 * 1024;
const FIRST_CODE: usize = 256;
const LAST_CODE: usize = 1 << 18;

#[derive(Deserialize, Serialize)]
pub(crate) struct Job {
    model: String,
    vendor: String,
    target: String,
    requirements: [String; 3],
    packages: [FirmwarePackage; 3],
}

#[derive(Deserialize)]
struct Record(String, String, [[String; 2]; 3], Vec<[String; 3]>, IgnoredAny, IgnoredAny);

fn hex(input: &str) -> std::result::Result<Vec<u8>, String> {
    if input.len() % 2 != 0 { return Err("Invalid challenge hex length".into()); }
    input.as_bytes().chunks_exact(2).map(|pair| {
        let high = (pair[0] as char).to_digit(16).ok_or("Invalid challenge hex")?;
        let low = (pair[1] as char).to_digit(16).ok_or("Invalid challenge hex")?;
        Ok(((high << 4) | low) as u8)
    }).collect()
}

fn challenge_cookie(page: &[u8]) -> std::result::Result<Option<String>, String> {
    if !page.windows(7).any(|bytes| bytes == b"__test=") { return Ok(None); }
    let page = std::str::from_utf8(page).map_err(|_| "Invalid challenge page encoding")?;
    let parameter = |name| {
        let (_, rest) = page.split_once(name).ok_or("Missing challenge parameter")?;
        hex(rest.split_once('"').ok_or("Invalid challenge parameter")?.0)
    };
    let key = parameter("a=toNumbers(\"")?;
    let mut previous: [u8; 16] = parameter("b=toNumbers(\"")?.try_into().map_err(|_| "Invalid challenge IV")?;
    let mut plain = parameter("c=toNumbers(\"")?;
    if plain.is_empty() || plain.len() % 16 != 0 { return Err("Invalid challenge ciphertext".into()); }
    macro_rules! decrypt {
        ($cipher:ty) => {{
            let cipher = <$cipher>::new_from_slice(&key).map_err(|_| "Invalid challenge key")?;
            for block in plain.chunks_exact_mut(16) {
                let ciphertext: [u8; 16] = (&*block).try_into().unwrap();
                cipher.decrypt_block((&mut *block).into());
                for (value, iv) in block.iter_mut().zip(previous) { *value ^= iv; }
                previous = ciphertext;
            }
        }};
    }
    match key.len() {
        16 => decrypt!(aes::Aes128),
        24 => decrypt!(aes::Aes192),
        32 => decrypt!(aes::Aes256),
        _ => return Err("Invalid challenge key length".into()),
    }
    if plain.len() > 16 {
        let padding = *plain.last().unwrap() as usize;
        if (1..=16).contains(&padding) && plain[plain.len() - padding..].iter().all(|&byte| byte as usize == padding) {
            plain.truncate(plain.len() - padding);
        }
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut cookie = String::with_capacity(plain.len() * 2);
    for byte in plain {
        cookie.push(HEX[(byte >> 4) as usize] as char);
        cookie.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(Some(cookie))
}

async fn download() -> Result<Vec<u8>> {
    let mut cookie = None;
    for _ in 0..2 {
        let headers = Headers::new();
        headers.set("User-Agent", USER_AGENT)?;
        headers.set("Cache-Control", "no-cache")?;
        if let Some(value) = &cookie { headers.set("Cookie", &format!("__test={value}"))?; }
        let request = Request::new_with_init(DATABASE, &RequestInit {
            method: Method::Get, headers, redirect: RequestRedirect::Manual, ..Default::default()
        })?;
        let mut response = Fetch::Request(request).send().await?;
        if response.status_code() != 200 {
            return Err(Error::RustError(format!("ffusersubmission: download failed HTTP {}", response.status_code())));
        }
        let mut bytes = Vec::new();
        let mut stream = response.stream()?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if chunk.len() > MAX_DOWNLOAD - bytes.len() {
                return Err(Error::RustError("ffusersubmission: download exceeds 4 MiB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        cookie = challenge_cookie(&bytes).map_err(Error::RustError)?;
        if cookie.is_none() { return Ok(bytes); }
    }
    Err(Error::RustError("ffusersubmission: challenge cookie rejected".into()))
}

fn decode(input: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let value = |byte| match byte {
        b'A'..=b'Z' => Ok((byte - b'A') as usize),
        b'a'..=b'z' => Ok((byte - b'a' + 26) as usize),
        b'0'..=b'9' => Ok((byte - b'0' + 52) as usize),
        b'-' => Ok(62), b'_' => Ok(63),
        _ => Err("Invalid database alphabet"),
    };
    if input.len() % 3 != 0 { return Err("Invalid database encoded length".into()); }
    let mut dictionary: Vec<(usize, usize)> = Vec::with_capacity((input.len() / 3).min(LAST_CODE - FIRST_CODE));
    let mut output = Vec::with_capacity(input.len());
    let mut previous = (0, 0);
    for chunk in input.chunks_exact(3) {
        let code = value(chunk[0])? | (value(chunk[1])? << 6) | (value(chunk[2])? << 12);
        let start = output.len();
        if code < FIRST_CODE {
            let character = char::from_u32(code.checked_sub(24).ok_or("Invalid literal code")? as u32)
                .ok_or("Invalid literal character")?;
            if character.len_utf8() > MAX_DECODED - output.len() { return Err("Decoded database exceeds 32 MiB".into()); }
            output.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
        } else {
            let &(offset, length) = dictionary.get(code - FIRST_CODE).or_else(|| {
                (code == FIRST_CODE + dictionary.len() && previous.1 != 0).then_some(&previous)
            }).ok_or("Invalid database dictionary code")?;
            let extra = if code == FIRST_CODE + dictionary.len() {
                (output[offset] as char).len_utf8()
            } else { 0 };
            if length + extra > MAX_DECODED - output.len() { return Err("Decoded database exceeds 32 MiB".into()); }
            output.extend_from_within(offset..offset + length);
            if extra != 0 { output.extend_from_within(offset..offset + extra); }
        }
        let length = output.len() - start;
        if previous.1 != 0 {
            let first = if output[start] < 128 { 1 } else { 2 };
            dictionary.push((previous.0, previous.1 + first));
            if dictionary.len() == LAST_CODE - FIRST_CODE { dictionary.clear(); }
        }
        previous = (start, length);
    }
    Ok(output)
}

fn package_value(name: &str) -> Option<u64> {
    let Some((_, version)) = name.split_once(' ') else { return Some(0); };
    let mut digits = version.split('(').next()?.trim_start().bytes().filter(|&byte| byte != b'.').peekable();
    digits.peek().filter(|byte| byte.is_ascii_digit())?;
    let mut value = 0u64;
    for byte in digits.take_while(u8::is_ascii_digit) {
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u64)?;
    }
    Some(value)
}

fn firmware_name(model: &str, packages: &[[String; 2]; 3]) -> String {
    let build = || {
        let base = &packages[0][0];
        let (_, version) = base.get(5..)?.split_once(' ')?;
        let (_, cust) = packages[1][0].rsplit_once('.')?;
        let (e, _) = cust.split_once('(')?;
        let (_, region) = packages[1][0].rsplit_once('(')?;
        let (region, _) = region.split_once(')')?;
        let (_, preload) = packages[2][0].rsplit_once('.')?;
        let (p, _) = preload.split_once('(')?;
        let (_, revision) = packages[2][0].rsplit_once('R')?;
        let (r, _) = revision.split_once(')')?;
        Some(format!("{model} {version}({region}E{e}R{r}P{p})"))
    };
    build().unwrap_or_else(|| packages[0][0].clone())
}

fn parse(input: &[u8]) -> std::result::Result<Vec<Job>, String> {
    let decoded = decode(input)?;
    let records: Vec<Record> = serde_json::from_slice(&decoded).map_err(|error| format!("Invalid database records: {error}"))?;
    drop(decoded);
    records.into_iter().map(|Record(model, region_vendor, packages, requirements, _, _)| {
        let (_, vendor) = region_vendor.split_once(" - ").ok_or("Missing firmware vendor")?;
        if model.is_empty() || vendor.is_empty() { return Err("Empty firmware model or vendor".into()); }
        let mut requirements = requirements.into_iter();
        let mut highest = requirements.next().ok_or("Missing firmware requirements")?;
        for entry in requirements {
            for (previous, candidate) in highest.iter_mut().zip(entry) {
                if let (Some(current), Some(next)) = (package_value(previous), package_value(&candidate)) {
                    if next > current { *previous = candidate; }
                }
            }
        }
        let target = firmware_name(&model, &packages);
        let [base, cust, preload] = packages.map(|[version_number, url]| {
            firmware::filelist_url(&url).map(|url| FirmwarePackage { version_number, url })
                .map_err(|error| error.to_string())
        });
        let mut vendor = vendor.replace('-', "_");
        vendor.make_ascii_lowercase();
        Ok(Job { model, vendor, target, requirements: highest, packages: [base?, cust?, preload?] })
    }).collect()
}

pub(super) async fn enqueue(env: &Env) -> Result<()> {
    let jobs = parse(&download().await?).map_err(Error::RustError)?;
    let queue = env.queue("OTA_REFRESH")?;
    for chunk in jobs.chunks(100) {
        queue.send_batch::<&Job, _>(chunk.iter()).await?;
    }
    console_log!("ffusersubmission: queued {} firmware records", jobs.len());
    Ok(())
}

pub(super) async fn process(job: &Job, env: &Env) -> Result<()> {
    let [base, cust, preload] = &job.requirements;
    let input = FirmwareInput {
        brand: "huawei", dev: job.model.clone(), vendor: job.vendor.clone(),
        base: base.clone(), cust: cust.clone(), preload: preload.clone(),
        patch: None, upgrade_or_erec: UpdateAction::Recovery,
    };
    let saved = service::save(env, &input, &job.target, &job.packages, SubmissionSource::Ffusersubmission)
        .await.map_err(|error| match error {
            SaveError::Fingerprint(message) => Error::RustError(format!("ffusersubmission: XML fingerprint failed: {message}")),
            SaveError::Storage(error) => error,
        })?;
    if saved.is_some() {
        console_log!("ffusersubmission: inserted firmware");
        if env.secret("TG_BOT_KEY").is_ok() && env.secret("TG_GROUP").is_ok() {
            if notifications::send(env, &input, &job.target).await.is_err() {
                console_log!("ffusersubmission: Telegram failed");
            }
            Delay::from(Duration::from_millis(3100)).await;
        }
    } else {
        console_log!("ffusersubmission: duplicate firmware");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(codes: impl IntoIterator<Item = usize>) -> Vec<u8> {
        const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        codes.into_iter().flat_map(|code| [ALPHABET[code & 63], ALPHABET[(code >> 6) & 63], ALPHABET[(code >> 12) & 63]]).collect()
    }

    #[test]
    fn lzw_handles_new_codes_and_rejects_missing_entries() {
        assert_eq!(decode(&encoded([89, 256, 257])).unwrap(), b"AAAAAA");
        assert_eq!(decode(&encoded([255, 256])).unwrap(), "ççç".as_bytes());
        assert!(decode(&encoded([256])).is_err());
        assert!(decode(&encoded([89, 257])).is_err());
        assert!(decode(b"AAA!").is_err());
        assert!(decode(b"AA!").is_err());
    }

    #[test]
    fn lzw_resets_dictionary_at_eighteen_bits() {
        let count = LAST_CODE - FIRST_CODE + 1;
        let codes = std::iter::repeat(89).take(count).chain([90, 256]);
        let decoded = decode(&encoded(codes)).unwrap();
        let mut expected = vec![b'A'; count];
        expected.extend_from_slice(b"BAB");
        assert_eq!(decoded, expected);
    }

    #[test]
    fn challenge_preserves_single_block_and_unpads_longer_messages() {
        let page = |ciphertext| format!("a=toNumbers(\"00000000000000000000000000000000\"),b=toNumbers(\"00000000000000000000000000000000\"),c=toNumbers(\"{ciphertext}\");__test=");
        assert_eq!(challenge_cookie(page("58e2fccefa7e3061367f1d57a4e7455a").as_bytes()).unwrap().unwrap(), "00000000000000000000000000000001");
        assert_eq!(challenge_cookie(page("66e94bd4ef8a2c3b884cfa59ca342b2e9434dec2d00fdac765f00c0c11628cd1").as_bytes()).unwrap().unwrap(), "00000000000000000000000000000000");
        assert!(challenge_cookie(b"__test=broken").is_err());
        assert_eq!(challenge_cookie(b"database").unwrap(), None);
    }

    #[test]
    fn records_rebuild_target_and_select_each_highest_requirement() {
        let packages = [
            ["BLA-LGRP2-OVS 102.0.0.225", "http://update.dbankcdn.com/base/"],
            ["BLA-L29-CUST 102.0.0.3(C185)", "http://update.dbankcdn.com/cust/"],
            ["BLA-L29-PRELOAD 102.0.0.1(C185R1)", "http://update.dbankcdn.com/preload/"],
        ];
        let requirements = [
            ["BASE 10.0.0.9", "CUST 10.0.0.11", "PRELOAD 10.0.0.2"],
            ["BASE 10.0.0.10", "CUST 10.0.0.2", "PRELOAD 10.0.0.3"],
        ];
        let json = serde_json::to_vec(&serde_json::json!([["BLA-L29", "C185 - HW-meafnaf", packages, requirements, 2, 1]])).unwrap();
        let mut jobs = parse(&encoded(json.into_iter().map(|byte| byte as usize + 24))).unwrap();
        let job = jobs.pop().unwrap();
        assert_eq!(job.target, "BLA-L29 102.0.0.225(C185E3R1P1)");
        assert_eq!(job.vendor, "hw_meafnaf");
        assert_eq!(job.requirements, ["BASE 10.0.0.10", "CUST 10.0.0.11", "PRELOAD 10.0.0.3"]);
        assert_eq!(job.packages[0].version_number, packages[0][0]);
        assert_eq!(job.packages[0].url, "http://update.dbankcdn.com/base/full/filelist.xml");
    }
}
