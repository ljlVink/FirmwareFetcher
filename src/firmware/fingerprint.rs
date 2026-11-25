use std::collections::BTreeMap;

use quick_xml::{events::Event, Reader};
use sha2::{Digest, Sha256};
use wasm_bindgen::{JsCast, JsValue};
use worker::{
    js_sys::{Reflect, Uint8Array},
    wasm_bindgen_futures::JsFuture,
    web_sys::{ReadableStream, ReadableStreamDefaultReader},
    Fetch, Headers, Method, Request, RequestInit, RequestRedirect, Response, ResponseBody, Url,
};

use crate::domain::FirmwarePackage;

const MAX_XML_SIZE: usize = 1024 * 1024;
const MAX_PACKAGES: usize = 32;
const MAX_NODES: usize = 16_384;

/// Huawei: only upstream XML file identities participate, never version labels or URLs.
/// Honor: the CDN rejects Workers egress, so identity is the manifest URL path
/// (scheme-insensitive). Package boundaries and repeated packages/entries are preserved.
pub(crate) async fn content_key(
    brand: &str,
    packages: &[FirmwarePackage],
) -> Result<String, String> {
    let sources = sources(brand, packages)?;
    if brand == "honor" {
        return url_key(&sources);
    }
    let mut identities = Vec::with_capacity(sources.len());
    for (url, selection) in sources {
        let xml = fetch_xml(&url).await?;
        identities.push(package_identity(&xml, selection.as_deref())?);
    }
    combine(identities, b"FirmwareFetcher XML content v1\0", "xml")
}

fn url_key(sources: &[(String, Option<String>)]) -> Result<String, String> {
    let identities = sources.iter().map(|(url, _)| {
        let url = Url::parse(url).map_err(|_| "Invalid firmware XML URL")?;
        Ok(Sha256::digest(url.path().as_bytes()).into())
    }).collect::<Result<Vec<[u8; 32]>, String>>()?;
    combine(identities, b"FirmwareFetcher URL identity v1\0", "url")
}

fn sources(
    brand: &str,
    packages: &[FirmwarePackage],
) -> Result<Vec<(String, Option<String>)>, String> {
    if !matches!(brand, "huawei" | "honor") {
        return Err("Unknown firmware brand".into());
    }
    if packages.len() > MAX_PACKAGES {
        return Err("Firmware package count exceeds 32".into());
    }
    packages
        .iter()
        .filter(|package| {
            let version = package.version_number.trim();
            version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit())
        })
        .map(|package| manifest_url(brand, &package.url))
        .collect()
}

fn manifest_url(brand: &str, raw: &str) -> Result<(String, Option<String>), String> {
    let mut url = Url::parse(raw).map_err(|_| "Invalid firmware XML URL")?;
    // Url normalizes explicit default ports away; inspect the original authority too.
    let authority = raw.split_once("://").map(|(_, rest)| {
        rest.split(['/', '?', '#']).next().unwrap_or("")
    }).ok_or("Firmware URL is missing a scheme")?;
    let host = url.host_str().ok_or("Firmware URL is missing a host")?;
    let trusted = match brand {
        "huawei" => host.ends_with(".dbankcdn.com") || host.ends_with(".hicloud.com"),
        "honor" => host == "update.hihonorcdn.com",
        _ => false,
    };
    if !trusted || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty() || url.password().is_some()
        || authority.contains(['@', ':', '\\'])
    {
        return Err("Firmware XML URL is not on a trusted upstream host".into());
    }
    let path = url.path();
    let selection = if path.ends_with(".zip") {
        let (directory, filename) = path.rsplit_once('/').ok_or("ZIP URL is missing a file name")?;
        if filename.is_empty() || filename.contains('%') {
            return Err("Invalid ZIP file name".into());
        }
        let filename = filename.to_owned();
        url.set_path(&format!("{directory}/filelist.xml"));
        Some(filename)
    } else {
        if !path.ends_with("/filelist.xml") {
            return Err("Firmware URL is neither an XML manifest nor a ZIP package".into());
        }
        None
    };
    url.set_fragment(None);
    Ok((url.into(), selection))
}

async fn fetch_xml(url: &str) -> Result<Vec<u8>, String> {
    let headers = Headers::new();
    headers.set("User-Agent", "curl/8.13.0").map_err(|e| e.to_string())?;
    headers.set("Accept-Encoding", "identity").map_err(|e| e.to_string())?;
    let request = Request::new_with_init(url, &RequestInit {
        method: Method::Get,
        headers,
        redirect: RequestRedirect::Manual,
        ..Default::default()
    }).map_err(|e| format!("Failed to build XML request: {e}"))?;
    let response = Fetch::Request(request).send().await
        .map_err(|e| format!("XML request failed: {e}"))?;
    let mut body = XmlBody::new(&response);
    if response.status_code() != 200 {
        return Err(format!("XML request failed: HTTP {}", response.status_code()));
    }
    if let Some(length) = response.headers().get("Content-Length").map_err(|e| e.to_string())? {
        let length: u64 = length.parse().map_err(|_| "Invalid XML Content-Length")?;
        if length > MAX_XML_SIZE as u64 {
            return Err("XML manifest exceeds 1 MiB".into());
        }
    }
    body.read().await
}

// Inspect JS chunk sizes before copying them into WASM. Drop also cancels failed
// status/oversized responses, including streams whose reader was never acquired.
struct XmlBody<'a> {
    stream: Option<&'a ReadableStream>,
    reader: Option<ReadableStreamDefaultReader>,
}

impl<'a> XmlBody<'a> {
    fn new(response: &'a Response) -> Self {
        Self {
            stream: match response.body() {
                ResponseBody::Stream(stream) => Some(stream),
                _ => None,
            },
            reader: None,
        }
    }

    async fn read(&mut self) -> Result<Vec<u8>, String> {
        let stream = self.stream.ok_or("XML response has no streaming body")?;
        self.reader = Some(ReadableStreamDefaultReader::new(stream)
            .map_err(|_| "Unable to open XML response stream")?);
        let reader = self.reader.as_ref().ok_or("Unable to open XML response stream")?;
        let done_key = JsValue::from_str("done");
        let value_key = JsValue::from_str("value");
        let mut bytes = Vec::new();
        for _ in 0..65_536 {
            let result = JsFuture::from(reader.read()).await.map_err(|_| "Failed to read XML stream")?;
            let done = Reflect::get(&result, &done_key).map_err(|_| "Invalid XML stream format")?
                .as_bool().ok_or("XML stream is missing the done flag")?;
            if done {
                return Ok(bytes);
            }
            let chunk = Reflect::get(&result, &value_key).map_err(|_| "Invalid XML stream format")?
                .dyn_into::<Uint8Array>().map_err(|_| "XML stream is not byte data")?;
            let length = chunk.length() as usize;
            if length > MAX_XML_SIZE - bytes.len() {
                return Err("XML manifest exceeds 1 MiB; download aborted".into());
            }
            let start = bytes.len();
            bytes.resize(start + length, 0);
            chunk.copy_to(&mut bytes[start..]);
        }
        Err("XML response chunk count exceeds the safety limit".into())
    }
}

impl Drop for XmlBody<'_> {
    fn drop(&mut self) {
        if let Some(reader) = &self.reader {
            let _ = reader.cancel();
            reader.release_lock();
        } else if let Some(stream) = self.stream {
            let _ = stream.cancel();
        }
    }
}

#[derive(Default)]
struct Node {
    name: String,
    attrs: BTreeMap<String, String>,
    text: String,
    children: Vec<Node>,
}

fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|_| "XML must be UTF-8".into())
}

fn parse(xml: &[u8]) -> Result<Node, String> {
    if xml.len() > MAX_XML_SIZE || xml.contains(&b'&') {
        return Err("XML exceeds limits or contains forbidden entity references".into());
    }
    utf8(xml)?;
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut count = 0;
    let mut declaration = false;
    loop {
        let event = reader.read_event().map_err(|e| format!("Malformed XML: {e}"))?;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                count += 1;
                if count > MAX_NODES || stack.len() >= 64 || (stack.is_empty() && root.is_some()) {
                    return Err("XML structure exceeds limits or has multiple root elements".into());
                }
                let mut node = Node { name: utf8(start.name().as_ref())?.to_owned(), ..Node::default() };
                for attr in start.attributes() {
                    let attr = attr.map_err(|_| "Invalid or duplicate XML attribute")?;
                    let name = utf8(attr.key.as_ref())?.to_owned();
                    let value = utf8(attr.value.as_ref())?.trim().to_owned();
                    if node.attrs.insert(name, value).is_some() {
                        return Err("Duplicate XML attribute".into());
                    }
                }
                if empty {
                    attach(node, &mut stack, &mut root)?;
                } else {
                    stack.push(node);
                }
            }
            Event::End(end) => {
                let node = stack.pop().ok_or("XML closing tag has no opening tag")?;
                if node.name.as_bytes() != end.name().as_ref() {
                    return Err("Mismatched XML tags".into());
                }
                attach(node, &mut stack, &mut root)?;
            }
            Event::Text(text) => {
                let text = utf8(text.as_ref())?;
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(text);
                } else if !text.trim().is_empty() {
                    return Err("XML has text outside the root element".into());
                }
            }
            Event::CData(text) => {
                stack.last_mut().ok_or("XML has CDATA outside the root element")?.text.push_str(utf8(text.as_ref())?);
            }
            Event::Decl(decl) => {
                if declaration || root.is_some() || !stack.is_empty() {
                    return Err("Invalid XML declaration position".into());
                }
                declaration = true;
                decl.version().map_err(|_| "Invalid XML declaration")?;
                if let Some(encoding) = decl.encoding() {
                    let encoding = encoding.map_err(|_| "Invalid XML encoding declaration")?;
                    if !encoding.eq_ignore_ascii_case(b"utf-8") {
                        return Err("XML encoding must be UTF-8".into());
                    }
                }
            }
            Event::DocType(_) => return Err("XML DTD is not allowed".into()),
            Event::Eof => break,
            Event::Comment(_) | Event::PI(_) => {}
        }
    }
    if !stack.is_empty() {
        return Err("XML manifest is truncated".into());
    }
    let root = root.ok_or("XML manifest is empty")?;
    if root.name != "root" {
        return Err("XML root element must be root".into());
    }
    Ok(root)
}

fn attach(node: Node, stack: &mut [Node], root: &mut Option<Node>) -> Result<(), String> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else if root.replace(node).is_some() {
        return Err("XML has multiple root elements".into());
    }
    Ok(())
}

fn normalize(name: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    match name {
        "sha256" | "shainfo" => {
            if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("XML {name} must be a full SHA-256 hex value"));
            }
            Ok(value.to_ascii_lowercase())
        }
        "size" => {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err("Invalid XML file size".into());
            }
            Ok(value.parse::<u64>().map_err(|_| "XML file size overflow")?.to_string())
        }
        "subpath" => Ok(value.to_owned()),
        _ if value.is_empty() => Err(format!("XML {name} must not be empty")),
        _ => Ok(value.to_owned()),
    }
}

fn field(node: &Node, name: &str, alias: Option<&str>, optional: bool) -> Result<String, String> {
    let mut value = None;
    let mut accept = |raw: &str| -> Result<(), String> {
        let normalized = normalize(name, raw)?;
        if value.as_ref().is_some_and(|previous| previous != &normalized) {
            return Err(format!("XML {name} has conflicting duplicate fields"));
        }
        value = Some(normalized);
        Ok(())
    };
    for key in std::iter::once(name).chain(alias) {
        if let Some(raw) = node.attrs.get(key) {
            accept(raw)?;
        }
        for child in node.children.iter().filter(|child| child.name == key) {
            if !child.children.is_empty() || !child.attrs.is_empty() {
                return Err(format!("XML {name} is not a plain text field"));
            }
            accept(&child.text)?;
        }
    }
    value.or_else(|| optional.then(String::new)).ok_or_else(|| format!("XML is missing {name}"))
}

fn frame(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value);
}

fn record(node: &Node, kind: &str, fields: &[&str]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    frame(&mut out, kind.as_bytes());
    for name in fields {
        frame(&mut out, name.as_bytes());
        let alias = (kind == "cloud" && *name == "size").then_some("sizeinfo");
        frame(&mut out, field(node, name, alias, *name == "subpath")?.as_bytes());
    }
    Ok(out)
}

fn has_field(node: &Node, fields: &[&str]) -> bool {
    fields.iter().any(|name| node.attrs.contains_key(*name) || node.children.iter().any(|child| child.name == *name))
}

fn downloads(node: &Node, kind: &str, fields: &[&str], out: &mut Vec<Vec<u8>>) -> Result<(), String> {
    if has_field(node, fields) || (kind == "cloud" && has_field(node, &["sizeinfo"])) {
        out.push(record(node, kind, fields)?);
    }
    for child in &node.children {
        // Scalar identity children are already consumed by the parent record.
        if !fields.contains(&child.name.as_str()) {
            downloads(child, kind, fields, out)?;
        }
    }
    Ok(())
}

pub(crate) fn package_identity(xml: &[u8], selection: Option<&str>) -> Result<[u8; 32], String> {
    let root = parse(xml)?;
    let mut records = Vec::new();
    let mut matches = 0;
    for node in &root.children {
        match node.name.as_str() {
            "files" => {
                for file in node.children.iter().filter(|file| file.name == "file") {
                    if let Some(filename) = selection {
                        let spath = field(file, "spath", None, false)?;
                        if spath.rsplit('/').next() != Some(filename) {
                            continue;
                        }
                        matches += 1;
                    }
                    records.push(record(file, "file", &["spath", "dpath", "operation", "sha256", "size"])?);
                }
            }
            "cloudRomInfo" | "cloudRom" if selection.is_none() => {
                downloads(node, "cloud", &["shainfo", "size", "subpath", "package"], &mut records)?;
            }
            "installExtPackageInfo" if selection.is_none() => {
                downloads(node, "install", &["filename", "sha256", "size"], &mut records)?;
            }
            _ => {}
        }
    }
    if selection.is_some() && matches != 1 {
        return Err("XML has no file entry uniquely matching the manual ZIP".into());
    }
    if records.is_empty() {
        return Err("XML has no valid file fingerprint".into());
    }
    records.sort_unstable();
    let mut digest = Sha256::new();
    for record in records {
        digest.update((record.len() as u64).to_be_bytes());
        digest.update(record);
    }
    Ok(digest.finalize().into())
}

fn combine(mut packages: Vec<[u8; 32]>, domain: &[u8], prefix: &str) -> Result<String, String> {
    if packages.is_empty() {
        return Err("No firmware packages available for fingerprinting".into());
    }
    packages.sort_unstable();
    let mut digest = Sha256::new();
    digest.update(domain);
    for package in packages {
        digest.update((package.len() as u64).to_be_bytes());
        digest.update(package);
    }
    Ok(format!("{prefix}:v1:{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::url_key;

    #[test]
    fn honor_url_key_ignores_scheme_and_order_but_not_path() {
        let source = |url: &str| (url.to_owned(), None);
        let a = "http://update.hihonorcdn.com/TDS/data/bl/files/v943171/f1/full/filelist.xml";
        let b = "http://update.hihonorcdn.com/TDS/data/bl/files/v926410/f1/full/filelist.xml";
        let key = url_key(&[source(a), source(b)]).unwrap();
        assert!(key.starts_with("url:v1:") && key.len() == 7 + 64);
        assert_eq!(key, url_key(&[source(&b.replacen("http:", "https:", 1)), source(a)]).unwrap());
        assert_ne!(key, url_key(&[source(a), source(&b.replace("v926410", "v926411"))]).unwrap());
        assert_ne!(key, url_key(&[source(a), source(b), source(b)]).unwrap());
    }
}
