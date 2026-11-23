use std::borrow::Cow;

use miniz_oxide::inflate::{
    core::{decompress, inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF, DecompressorOxide},
    TINFLStatus,
};
use wasm_bindgen::{JsCast, JsValue};
use worker::{
    js_sys::{Reflect, Uint8Array},
    wasm_bindgen_futures::JsFuture,
    web_sys::{ReadableStream, ReadableStreamDefaultReader},
    CacheMode, Fetch, Headers, Method, Request, RequestInit, RequestRedirect, Response, ResponseBody,
};

const MAX_ENTRY_SIZE: u64 = 1024 * 1024;
const MAX_DIRECTORY_SIZE: u64 = 4 * 1024 * 1024;
const MAX_ENTRIES: u64 = 16_384;
const TAIL_SIZE: u64 = 22 + 65_535 + 20 + 56;
const MAX_ZIP64_END_SIZE: u64 = 65_536;
const MAX_REQUESTS: usize = 6;
const MAX_DOWNLOAD_SIZE: u64 = 6 * 1024 * 1024;
const MAX_BODY_CHUNKS: usize = 65_536;

pub(crate) async fn fetch_entry(url: &str, entry_name: &str) -> Result<String, String> {
    if entry_name.is_empty()
        || !entry_name.is_ascii()
        || entry_name.bytes().any(|b| matches!(b, b'/' | b'\\' | 0))
    {
        return Err("ZIP target name must be an ASCII file name without a path".into());
    }
    let mut remote = RemoteZip {
        url,
        size: None,
        requests: 0,
        downloaded: 0,
    };
    remote.request(0, 1).await?;
    let size = remote.size.ok_or("ZIP file size has not been read yet")?;
    let length = size.min(TAIL_SIZE);
    let tail = remote.request(size - length, length).await?;
    let directory = read_directory_info(&mut remote, &tail).await?;
    let central = remote
        .read_cached(&tail, directory.offset, directory.size)
        .await?;
    let entry = find_entry(&central, &directory, entry_name.as_bytes())?;
    drop(central);
    let content = read_entry(&mut remote, &tail, &entry).await?;
    let mut text = String::from_utf8(content)
        .map_err(|error| format!("{entry_name} is not valid UTF-8 text: {error}"))?;
    let end = text.trim_end_matches('\0');
    let end = end
        .strip_suffix("\r\n")
        .or_else(|| end.strip_suffix('\n'))
        .unwrap_or(end);
    text.truncate(end.len());
    let prefix_len = text.len() - text.trim_start_matches('\u{feff}').len();
    if prefix_len != 0 {
        drop(text.drain(..prefix_len));
    }
    Ok(text)
}

struct RemoteZip<'a> {
    url: &'a str,
    size: Option<u64>,
    requests: usize,
    downloaded: u64,
}

struct RangeBytes {
    start: u64,
    bytes: Vec<u8>,
}

impl RemoteZip<'_> {
    async fn read_cached<'a>(
        &mut self,
        tail: &'a RangeBytes,
        start: u64,
        length: u64,
    ) -> Result<Cow<'a, [u8]>, String> {
        let end = add(start, length)?;
        let size = self.size.ok_or("ZIP file size has not been read yet")?;
        if end > size {
            return Err("ZIP metadata points outside the file".into());
        }
        if length == 0 {
            return Ok(Cow::Borrowed(&[]));
        }
        if start >= tail.start && end <= add(tail.start, tail.bytes.len() as u64)? {
            let offset = (start - tail.start) as usize;
            return Ok(Cow::Borrowed(&tail.bytes[offset..offset + length as usize]));
        }
        Ok(Cow::Owned(
            self.request(start, length)
                .await?
                .bytes,
        ))
    }

    async fn request(&mut self, start: u64, limit: u64) -> Result<RangeBytes, String> {
        let end = add(start, limit)?;
        if limit == 0 || self.size.is_some_and(|size| end > size)
            || (self.size.is_none() && (start != 0 || limit != 1))
        {
            return Err("ZIP Range request is out of bounds, zero-length or has an unknown file size".into());
        }
        let range_header = format!("bytes={start}-{}", end - 1);
        if limit > MAX_DIRECTORY_SIZE
            || self.requests >= MAX_REQUESTS
            || add(self.downloaded, limit)? > MAX_DOWNLOAD_SIZE
        {
            return Err("Reading ZIP exceeded the request count or download byte limit".into());
        }
        self.requests += 1;
        let headers = Headers::new();
        headers
            .set("Range", &range_header)
            .and_then(|_| headers.set("Accept-Encoding", "identity"))
            .and_then(|_| headers.set("Cache-Control", "no-transform"))
            .map_err(|error| format!("Unable to set ZIP request headers: {error}"))?;
        let init = RequestInit {
            method: Method::Get,
            headers,
            redirect: RequestRedirect::Manual,
            cache: Some(CacheMode::NoStore),
            ..Default::default()
        };
        let request = Request::new_with_init(self.url, &init)
            .map_err(|error| format!("Unable to create ZIP request: {error}"))?;
        let response = Fetch::Request(request)
            .send()
            .await
            .map_err(|error| format!("ZIP Range request failed ({range_header}): {error}"))?;
        // Install cancellation before inspecting headers, including error/redirect responses.
        let mut body = LimitedBody::new(&response);
        let status = response.status_code();
        if status != 206 {
            return Err(if (300..400).contains(&status) {
                format!("Download server returned a redirect (HTTP {status}); following is forbidden")
            } else {
                format!("Download server must support exact Range requests ({range_header}: expected HTTP 206, got {status})")
            });
        }
        if let Some(encoding) = header(&response, "Content-Encoding")? {
            if !encoding.eq_ignore_ascii_case("identity") {
                return Err(format!("Download server used a forbidden transfer encoding: {encoding}"));
            }
        }
        let content_range = header(&response, "Content-Range")?
            .ok_or("Range response is missing Content-Range")?;
        let (response_start, response_end, total) = parse_content_range(&content_range)?;
        if let Some(size) = self.size {
            if size != total {
                return Err("ZIP total size differs across Range responses".into());
            }
        }
        if (response_start, response_end) != (start, end - 1) {
            return Err(format!(
                "Range response mismatch: requested {range_header}, received {content_range}"
            ));
        }
        let length = response_end - response_start + 1;
        if let Some(content_length) = header(&response, "Content-Length")? {
            if decimal(&content_length)? != length {
                return Err("Range response Content-Length does not match Content-Range".into());
            }
        }
        self.size = Some(total);
        self.downloaded = add(self.downloaded, length)?;
        let bytes = body.read(length as usize).await?;
        Ok(RangeBytes { start, bytes })
    }
}

fn header(response: &Response, name: &str) -> Result<Option<String>, String> {
    response
        .headers()
        .get(name)
        .map_err(|error| format!("Unable to read {name} response header: {error}"))
}

fn decimal(value: &str) -> Result<u64, String> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Range response header contains an invalid decimal byte count".into());
    }
    value
        .parse()
        .map_err(|_| "Range response header byte count overflow".into())
}

fn parse_content_range(value: &str) -> Result<(u64, u64, u64), String> {
    let invalid = || "Download server returned an invalid Content-Range".to_owned();
    let value = value.strip_prefix("bytes ").ok_or_else(invalid)?;
    let (span, total) = value.split_once('/').ok_or_else(invalid)?;
    let (start, end) = span.split_once('-').ok_or_else(invalid)?;
    let (start, end, total) = (decimal(start)?, decimal(end)?, decimal(total)?);
    if start > end || end >= total {
        return Err("Content-Range start, end or total size is out of bounds".into());
    }
    Ok((start, end, total))
}

struct LimitedBody<'a> {
    stream: Option<&'a ReadableStream>,
    reader: Option<ReadableStreamDefaultReader>,
}

impl<'a> LimitedBody<'a> {
    fn new(response: &'a Response) -> Self {
        Self {
            stream: match response.body() {
                ResponseBody::Stream(stream) => Some(stream),
                _ => None,
            },
            reader: None,
        }
    }

    async fn read(&mut self, length: usize) -> Result<Vec<u8>, String> {
        let stream = self.stream.ok_or("Range response has no streaming body")?;
        self.reader = Some(
            ReadableStreamDefaultReader::new(stream)
                .map_err(|_| "Unable to open Range response stream")?,
        );
        let reader = self.reader.as_ref().ok_or("Unable to open Range response stream")?;
        let done_key = JsValue::from_str("done");
        let value_key = JsValue::from_str("value");
        let mut bytes = Vec::with_capacity(length);
        for _ in 0..MAX_BODY_CHUNKS {
            let result = JsFuture::from(reader.read())
                .await
                .map_err(|_| "Failed to read Range response body")?;
            let done = Reflect::get(&result, &done_key)
                .map_err(|_| "Invalid Range response stream format")?
                .as_bool()
                .ok_or("Range response stream is missing the done flag")?;
            if done {
                if bytes.len() != length {
                    return Err("Range response body is truncated and does not match the declared length".into());
                }
                return Ok(bytes);
            }
            let chunk = Reflect::get(&result, &value_key)
                .map_err(|_| "Invalid Range response stream format")?
                .dyn_into::<Uint8Array>()
                .map_err(|_| "Range response stream is not byte data")?;
            let chunk_length = chunk.length() as usize;
            if chunk_length > length - bytes.len() {
                return Err("Range response body exceeds the declared range; download aborted".into());
            }
            let previous = bytes.len();
            bytes.resize(previous + chunk_length, 0);
            chunk.copy_to(&mut bytes[previous..]);
        }
        Err("Range response chunk count exceeds the safety limit".into())
    }
}

impl Drop for LimitedBody<'_> {
    fn drop(&mut self) {
        if let Some(reader) = &self.reader {
            let _ = reader.cancel();
            reader.release_lock();
        } else if let Some(stream) = self.stream {
            let _ = stream.cancel();
        }
    }
}

struct Directory {
    offset: u64,
    size: u64,
    entries: u64,
}

async fn read_directory_info(
    remote: &mut RemoteZip<'_>,
    tail: &RangeBytes,
) -> Result<Directory, String> {
    let bytes = &tail.bytes;
    if bytes.len() < 22 {
        return Err("ZIP file is too short and has no end of central directory record".into());
    }
    let mut found = None;
    for offset in (0..=bytes.len() - 22).rev() {
        if &bytes[offset..offset + 4] == b"PK\x05\x06"
            && offset + 22 + u16_at(bytes, offset + 20)? as usize == bytes.len()
        {
            if found.replace(offset).is_some() {
                return Err("ZIP has multiple interpretable end of central directory records; ambiguous file rejected".into());
            }
        }
    }
    let offset = found.ok_or("ZIP end of central directory record is missing, truncated or followed by extra data")?;
    let end = &bytes[offset..];
    if u16_at(end, 4)? != 0 || u16_at(end, 6)? != 0 {
        return Err("Multi-disk ZIP is not supported".into());
    }
    let disk_entries = u16_at(end, 8)?;
    let entries = u16_at(end, 10)?;
    let size = u32_at(end, 12)?;
    let directory_offset = u32_at(end, 16)?;
    let needs_zip64 = disk_entries == u16::MAX
        || entries == u16::MAX
        || size == u32::MAX
        || directory_offset == u32::MAX;
    let has_locator = offset >= 20 && &bytes[offset - 20..offset - 16] == b"PK\x06\x07";
    let (directory, metadata_start) = if has_locator {
        let locator = &bytes[offset - 20..offset];
        if u32_at(locator, 4)? != 0 || u32_at(locator, 16)? != 1 {
            return Err("ZIP64 locator points to a multi-disk archive".into());
        }
        let zip64_offset = u64_at(locator, 8)?;
        let locator_offset = add(tail.start, (offset - 20) as u64)?;
        if add(zip64_offset, 56)? > locator_offset {
            return Err("ZIP64 end record is out of bounds or overlaps the locator".into());
        }
        let record = remote.read_cached(tail, zip64_offset, 56).await?;
        if &record[..4] != b"PK\x06\x06" {
            return Err("Invalid ZIP64 end record signature".into());
        }
        let record_size = add(u64_at(&record, 4)?, 12)?;
        if !(56..=MAX_ZIP64_END_SIZE).contains(&record_size)
            || add(zip64_offset, record_size)? != locator_offset
        {
            return Err("ZIP64 end record has an abnormal length or is not adjacent to the locator".into());
        }
        if u16_at(&record, 14)? != 45 {
            return Err("Unsupported ZIP64 version or encrypted directory format".into());
        }
        if u32_at(&record, 16)? != 0 || u32_at(&record, 20)? != 0 {
            return Err("Multi-disk ZIP64 is not supported".into());
        }
        let count = u64_at(&record, 32)?;
        let full_size = u64_at(&record, 40)?;
        let full_offset = u64_at(&record, 48)?;
        if u64_at(&record, 24)? != count
            || (disk_entries != u16::MAX && u64::from(disk_entries) != count)
            || (entries != u16::MAX && u64::from(entries) != count)
            || (size != u32::MAX && u64::from(size) != full_size)
            || (directory_offset != u32::MAX && u64::from(directory_offset) != full_offset)
        {
            return Err("ZIP64 and standard ZIP central directory information disagree".into());
        }
        (
            Directory {
                offset: full_offset,
                size: full_size,
                entries: count,
            },
            zip64_offset,
        )
    } else {
        if needs_zip64 {
            return Err("ZIP uses ZIP64 size markers but has no ZIP64 locator".into());
        }
        if disk_entries != entries {
            return Err("ZIP central directory file counts disagree".into());
        }
        (
            Directory {
                offset: u64::from(directory_offset),
                size: u64::from(size),
                entries: u64::from(entries),
            },
            add(tail.start, offset as u64)?,
        )
    };
    if directory.entries == 0 {
        return Err("ZIP central directory has no files".into());
    }
    if directory.entries > MAX_ENTRIES || directory.size > MAX_DIRECTORY_SIZE {
        return Err("ZIP central directory exceeds the 4 MiB or 16384-entry safety limit".into());
    }
    if directory.size < directory.entries * 46
        || add(directory.offset, directory.size)? != metadata_start
    {
        return Err("Invalid ZIP central directory size, offset or file count".into());
    }
    Ok(directory)
}

struct Entry {
    name: Vec<u8>,
    needed_version: u16,
    flags: u16,
    method: u16,
    crc: u32,
    compressed_size: u64,
    size: u64,
    local_offset: u64,
    boundary: u64,
    zip64_sizes: bool,
}

fn find_entry(bytes: &[u8], directory: &Directory, target: &[u8]) -> Result<Entry, String> {
    let mut cursor = 0;
    let mut found = None;
    let mut spans = Vec::with_capacity(directory.entries as usize);
    for _ in 0..directory.entries {
        let fixed = slice(bytes, cursor, 46)?;
        if &fixed[..4] != b"PK\x01\x02" {
            return Err("Invalid ZIP central directory entry signature".into());
        }
        let flags = u16_at(fixed, 8)?;
        let name_length = u16_at(fixed, 28)? as usize;
        let extra_length = u16_at(fixed, 30)? as usize;
        let comment_length = u16_at(fixed, 32)? as usize;
        let name = slice(bytes, cursor + 46, name_length)?;
        check_name(name, flags)?;
        let extra = parse_extras(slice(bytes, cursor + 46 + name_length, extra_length)?, name)?;
        let is_target = basename_matches(name, target);
        if let Some(unicode) = extra.unicode_name {
            if unicode != name && (is_target || basename_matches(unicode, target)) {
                return Err("ZIP target entry Unicode file name differs from the raw file name; ambiguous file rejected".into());
            }
        }
        let size32 = u32_at(fixed, 24)?;
        let compressed32 = u32_at(fixed, 20)?;
        let offset32 = u32_at(fixed, 42)?;
        let disk16 = u16_at(fixed, 34)?;
        let values = resolve_zip64(extra.zip64, size32, compressed32, Some(offset32), Some(disk16))?;
        if values.disk != 0 {
            return Err("ZIP entry points to another disk".into());
        }
        let minimum_end = add(add(values.offset, 30 + name_length as u64)?, values.compressed)?;
        if minimum_end > directory.offset {
            return Err("ZIP entry local header or compressed data exceeds the data area".into());
        }
        spans.push((values.offset, minimum_end));
        if is_target {
            if found.is_some() {
                return Err("ZIP contains multiple target files with the same name (including nested paths or case variants); ambiguous file rejected".into());
            }
            let method = u16_at(fixed, 10)?;
            if flags & !0x080e != 0 || (method == 0 && flags & 6 != 0) {
                return Err("ZIP target file uses encryption or unsupported flags".into());
            }
            if !matches!(method, 0 | 8) {
                return Err(format!("ZIP target file uses unsupported compression method {method} (only stored/deflate are supported)"));
            }
            if values.size > MAX_ENTRY_SIZE || values.compressed > MAX_ENTRY_SIZE {
                return Err("ZIP target file compressed or uncompressed size exceeds the 1 MiB limit".into());
            }
            if method == 0 && values.size != values.compressed {
                return Err("ZIP stored entry compressed and uncompressed sizes differ".into());
            }
            let needed_version = u16_at(fixed, 6)?;
            let zip64_sizes = size32 == u32::MAX || compressed32 == u32::MAX;
            if needed_version > 45
                || ((zip64_sizes || offset32 == u32::MAX) && needed_version < 45)
            {
                return Err("ZIP target entry format version is invalid or unsupported".into());
            }
            found = Some(Entry {
                name: name.to_vec(),
                needed_version,
                flags,
                method,
                crc: u32_at(fixed, 16)?,
                compressed_size: values.compressed,
                size: values.size,
                local_offset: values.offset,
                boundary: directory.offset,
                zip64_sizes,
            });
        }
        cursor += 46 + name_length + extra_length + comment_length;
        if cursor > bytes.len() {
            return Err("ZIP central directory file comment is truncated".into());
        }
    }
    if cursor != bytes.len() {
        return Err("ZIP central directory contains undeclared entries or extra data".into());
    }
    spans.sort_unstable_by_key(|span| span.0);
    for adjacent in spans.windows(2) {
        if adjacent[0].1 > adjacent[1].0 {
            return Err("ZIP entries have duplicate offsets or overlapping compressed data".into());
        }
    }
    let mut entry = found.ok_or_else(|| {
        format!("{} not found in ZIP", String::from_utf8_lossy(target))
    })?;
    if let Some(next) = spans.iter().find(|span| span.0 > entry.local_offset) {
        entry.boundary = next.0;
    }
    Ok(entry)
}

fn check_name(name: &[u8], flags: u16) -> Result<(), String> {
    if name.is_empty() || name.contains(&0) {
        return Err("ZIP file name is empty or contains a NUL byte".into());
    }
    if flags & 0x0800 != 0 && std::str::from_utf8(name).is_err() {
        return Err("ZIP file name declares UTF-8 but is not valid UTF-8".into());
    }
    Ok(())
}

fn basename_matches(name: &[u8], target: &[u8]) -> bool {
    name.rsplit(|byte| matches!(*byte, b'/' | b'\\'))
        .next()
        .is_some_and(|basename| basename.eq_ignore_ascii_case(target))
}

struct Extras<'a> {
    zip64: Option<&'a [u8]>,
    unicode_name: Option<&'a [u8]>,
}

fn parse_extras<'a>(bytes: &'a [u8], name: &[u8]) -> Result<Extras<'a>, String> {
    let mut extra = Extras {
        zip64: None,
        unicode_name: None,
    };
    let mut cursor = 0;
    while bytes.len() - cursor >= 4 {
        let tag = u16_at(bytes, cursor)?;
        let length = u16_at(bytes, cursor + 2)? as usize;
        let value = slice(bytes, cursor + 4, length)?;
        match tag {
            0x0001 => {
                if extra.zip64.replace(value).is_some() {
                    return Err("ZIP entry contains duplicate ZIP64 extra fields".into());
                }
            }
            0x7075 => {
                if value.len() < 5 || value[0] != 1 || u32_at(value, 1)? != crc32fast::hash(name) {
                    return Err("Invalid ZIP Unicode path extra field version or checksum".into());
                }
                let unicode = &value[5..];
                check_name(unicode, 0x0800)?;
                if extra.unicode_name.replace(unicode).is_some() {
                    return Err("ZIP entry contains duplicate Unicode file names".into());
                }
            }
            _ => {}
        }
        cursor += 4 + length;
    }
    if bytes[cursor..].iter().any(|&byte| byte != 0) {
        return Err("ZIP metadata or extra field is truncated".into());
    }
    Ok(extra)
}

struct Zip64Values {
    size: u64,
    compressed: u64,
    offset: u64,
    disk: u32,
}

fn resolve_zip64(
    extra: Option<&[u8]>,
    size: u32,
    compressed: u32,
    offset: Option<u32>,
    disk: Option<u16>,
) -> Result<Zip64Values, String> {
    let mut values = Zip64Values {
        size: u64::from(size),
        compressed: u64::from(compressed),
        offset: u64::from(offset.unwrap_or(0)),
        disk: u32::from(disk.unwrap_or(0)),
    };
    let needs_extra = size == u32::MAX
        || compressed == u32::MAX
        || offset == Some(u32::MAX)
        || disk == Some(u16::MAX);
    if needs_extra && extra.is_none() {
        return Err("ZIP entry uses ZIP64 markers but has no extra field".into());
    }
    let data = extra.unwrap_or(&[]);
    let mut cursor = 0;
    if size == u32::MAX {
        values.size = u64_at(data, cursor)?;
        cursor += 8;
    }
    if compressed == u32::MAX {
        values.compressed = u64_at(data, cursor)?;
        cursor += 8;
    }
    if offset == Some(u32::MAX) {
        values.offset = u64_at(data, cursor)?;
        cursor += 8;
    }
    if disk == Some(u16::MAX) {
        values.disk = u32_at(data, cursor)?;
        cursor += 4;
    }
    if cursor != data.len() {
        return Err("ZIP64 extra field length does not match the size/offset markers".into());
    }
    Ok(values)
}

async fn read_entry(
    remote: &mut RemoteZip<'_>,
    tail: &RangeBytes,
    entry: &Entry,
) -> Result<Vec<u8>, String> {
    let fixed = remote.read_cached(tail, entry.local_offset, 30).await?;
    if &fixed[..4] != b"PK\x03\x04" {
        return Err("Invalid ZIP target file local header signature".into());
    }
    if u16_at(&fixed, 4)? != entry.needed_version
        || u16_at(&fixed, 6)? != entry.flags
        || u16_at(&fixed, 8)? != entry.method
    {
        return Err("ZIP local header and central directory disagree on version, flags or compression method".into());
    }
    let name_length = u16_at(&fixed, 26)? as usize;
    let extra_length = u16_at(&fixed, 28)? as usize;
    if name_length != entry.name.len() {
        return Err("ZIP local header and central directory disagree on file name length".into());
    }
    let variable_offset = add(entry.local_offset, 30)?;
    let data_offset = add(variable_offset, (name_length + extra_length) as u64)?;
    let data_end = add(data_offset, entry.compressed_size)?;
    if data_end > entry.boundary {
        return Err("ZIP target file data exceeds the entry boundary or overlaps other entries".into());
    }
    let has_descriptor = entry.flags & 8 != 0;
    let descriptor_length = if has_descriptor {
        (entry.boundary - data_end).min(24)
    } else {
        0
    };
    if has_descriptor && descriptor_length < 12 {
        return Err("ZIP target file data descriptor is truncated".into());
    }
    let variable = remote
        .read_cached(
            tail,
            variable_offset,
            add((name_length + extra_length) as u64, add(entry.compressed_size, descriptor_length)?)?,
        )
        .await?;
    let local_name = &variable[..name_length];
    if local_name != entry.name {
        return Err("ZIP local header and central directory disagree on file name".into());
    }
    let extra = parse_extras(&variable[name_length..name_length + extra_length], local_name)?;
    if extra.unicode_name.is_some_and(|name| name != local_name) {
        return Err("ZIP local header contains an inconsistent Unicode target file name".into());
    }
    let size32 = u32_at(&fixed, 22)?;
    let compressed32 = u32_at(&fixed, 18)?;
    let local = resolve_zip64(extra.zip64, size32, compressed32, None, None)?;
    if (size32 == u32::MAX || compressed32 == u32::MAX) && entry.needed_version < 45 {
        return Err("ZIP local header ZIP64 markers do not match the format version".into());
    }
    let local_crc = u32_at(&fixed, 14)?;
    if has_descriptor {
        if (local_crc != 0 && local_crc != entry.crc)
            || (local.size != 0 && local.size != entry.size)
            || (local.compressed != 0 && local.compressed != entry.compressed_size)
        {
            return Err("ZIP local header checksum or sizes disagree with the central directory".into());
        }
    } else if local_crc != entry.crc
        || local.size != entry.size
        || local.compressed != entry.compressed_size
    {
        return Err("ZIP local header checksum or sizes disagree with the central directory".into());
    }
    let compressed_start = name_length + extra_length;
    let compressed_end = compressed_start + entry.compressed_size as usize;
    if has_descriptor {
        check_descriptor(
            &variable[compressed_end..],
            entry,
            entry.zip64_sizes || size32 == u32::MAX || compressed32 == u32::MAX,
        )?;
    }
    if entry.method == 0 {
        let compressed = match variable {
            Cow::Owned(mut bytes) => {
                bytes.copy_within(compressed_start..compressed_end, 0);
                bytes.truncate(entry.compressed_size as usize);
                Cow::Owned(bytes)
            }
            Cow::Borrowed(bytes) => Cow::Borrowed(&bytes[compressed_start..compressed_end]),
        };
        decode_content(compressed, entry)
    } else {
        decode_content(Cow::Borrowed(&variable[compressed_start..compressed_end]), entry)
    }
}

fn decode_content(compressed: Cow<'_, [u8]>, entry: &Entry) -> Result<Vec<u8>, String> {
    if entry.size > MAX_ENTRY_SIZE || entry.compressed_size > MAX_ENTRY_SIZE {
        return Err("ZIP target file compressed or uncompressed size exceeds the 1 MiB limit".into());
    }
    if compressed.len() as u64 != entry.compressed_size {
        return Err("ZIP target file actual compressed size does not match the central directory".into());
    }
    let content = if entry.method == 0 {
        if entry.size != entry.compressed_size {
            return Err("ZIP stored entry compressed and uncompressed sizes differ".into());
        }
        compressed.into_owned()
    } else if entry.method == 8 {
        // One extra byte detects forged sizes without ever growing a ZIP bomb's output.
        let mut output = vec![0; entry.size as usize + 1];
        let (status, consumed, produced) = decompress(
            &mut DecompressorOxide::new(),
            &compressed,
            &mut output,
            0,
            TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
        );
        if status != TINFLStatus::Done
            || consumed != compressed.len()
            || produced != entry.size as usize
        {
            return Err("ZIP deflate data is corrupt, has trailing compressed data or its uncompressed size does not match the central directory".into());
        }
        output.truncate(produced);
        output
    } else {
        return Err("ZIP target file uses an unsupported compression method".into());
    };
    if content.len() as u64 != entry.size {
        return Err("ZIP target file actual uncompressed size does not match the central directory".into());
    }
    if crc32fast::hash(&content) != entry.crc {
        return Err("ZIP target file CRC32 check failed; data is corrupt".into());
    }
    Ok(content)
}

fn check_descriptor(bytes: &[u8], entry: &Entry, zip64: bool) -> Result<(), String> {
    let length = if zip64 { 20 } else { 12 };
    let mut matches = 0;
    for prefix in [0, 4] {
        if bytes.len() < prefix + length
            || (prefix == 4 && u32_at(bytes, 0)? != 0x08074b50)
        {
            continue;
        }
        let data = &bytes[prefix..];
        let (compressed, size) = if zip64 {
            (u64_at(data, 4)?, u64_at(data, 12)?)
        } else {
            (u64::from(u32_at(data, 4)?), u64::from(u32_at(data, 8)?))
        };
        if u32_at(data, 0)? == entry.crc
            && compressed == entry.compressed_size
            && size == entry.size
        {
            matches += 1;
        }
    }
    if matches != 1 {
        return Err("ZIP data descriptor CRC32/size mismatch, truncated or ambiguous".into());
    }
    Ok(())
}

fn add(start: u64, length: u64) -> Result<u64, String> {
    start
        .checked_add(length)
        .ok_or_else(|| "ZIP byte offset or length overflow".into())
}

fn slice(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8], String> {
    let end = offset.checked_add(length).ok_or("ZIP metadata length overflow")?;
    bytes
        .get(offset..end)
        .ok_or_else(|| "ZIP metadata or extra field is truncated".into())
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let data = slice(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([data[0], data[1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let data = slice(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let data = slice(bytes, offset, 8)?;
    Ok(u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::parse_extras;

    #[test]
    fn zip_extra_alignment_does_not_hide_truncation() {
        let extra = [1, 0, 8, 0, 0x34, 0x12, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        for end in 12..=15 {
            let parsed = parse_extras(&extra[..end], b"VERSION.mbn").unwrap();
            assert_eq!(parsed.zip64, Some(&extra[4..12]));
        }
        assert!(parse_extras(&[0, 0, 1], b"VERSION.mbn").is_err());
        assert!(parse_extras(&extra[..11], b"VERSION.mbn").is_err());
    }
}
