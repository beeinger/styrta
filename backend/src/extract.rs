//! ZIP listing and text extraction.
//!
//! Polish Windows packagers store names in CP852 and do not set the UTF-8 flag.
//! `zip` would show those names as CP437 mojibake, so names are decoded from the raw bytes.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use zip::ZipArchive;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Docx,
    Odt,
    Text,
    Rtf,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Odt => "odt",
            Self::Text => "text",
            Self::Rtf => "rtf",
            Self::Other => "other",
        }
    }

    pub fn readable(self) -> bool {
        !matches!(self, Self::Other)
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub index: usize,
    pub path: String,
    pub byte_len: u64,
    pub kind: Kind,
}

pub struct Extracted {
    pub text: String,
    pub sha256: String,
    pub note: String,
}

const CP852: [char; 128] = [
    '\u{00c7}', '\u{00fc}', '\u{00e9}', '\u{00e2}', '\u{00e4}', '\u{016f}', '\u{0107}', '\u{00e7}',
    '\u{0142}', '\u{00eb}', '\u{0150}', '\u{0151}', '\u{00ee}', '\u{0179}', '\u{00c4}', '\u{0106}',
    '\u{00c9}', '\u{0139}', '\u{013a}', '\u{00f4}', '\u{00f6}', '\u{013d}', '\u{013e}', '\u{015a}',
    '\u{015b}', '\u{00d6}', '\u{00dc}', '\u{0164}', '\u{0165}', '\u{0141}', '\u{00d7}', '\u{010d}',
    '\u{00e1}', '\u{00ed}', '\u{00f3}', '\u{00fa}', '\u{0104}', '\u{0105}', '\u{017d}', '\u{017e}',
    '\u{0118}', '\u{0119}', '\u{00ac}', '\u{017a}', '\u{010c}', '\u{015f}', '\u{00ab}', '\u{00bb}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{00c1}', '\u{00c2}', '\u{011a}',
    '\u{015e}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255d}', '\u{017b}', '\u{017c}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252c}', '\u{251c}', '\u{2500}', '\u{253c}', '\u{0102}', '\u{0103}',
    '\u{255a}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256c}', '\u{00a4}',
    '\u{0111}', '\u{0110}', '\u{010e}', '\u{00cb}', '\u{010f}', '\u{0147}', '\u{00cd}', '\u{00ce}',
    '\u{011b}', '\u{2518}', '\u{250c}', '\u{2588}', '\u{2584}', '\u{0162}', '\u{016e}', '\u{2580}',
    '\u{00d3}', '\u{00df}', '\u{00d4}', '\u{0143}', '\u{0144}', '\u{0148}', '\u{0160}', '\u{0161}',
    '\u{0154}', '\u{00da}', '\u{0155}', '\u{0170}', '\u{00fd}', '\u{00dd}', '\u{0163}', '\u{00b4}',
    '\u{00ad}', '\u{02dd}', '\u{02db}', '\u{02c7}', '\u{02d8}', '\u{00a7}', '\u{00f7}', '\u{00b8}',
    '\u{00b0}', '\u{00a8}', '\u{02d9}', '\u{0171}', '\u{0158}', '\u{0159}', '\u{25a0}', '\u{00a0}',
];

pub fn index_zip(path: &Path) -> Result<Vec<Entry>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut archive = ZipArchive::new(file).context("read zip")?;
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        if !file.is_file() {
            continue;
        }
        let name = decode_zip_name(file.name_raw());
        if !safe_zip_path(&name) {
            continue;
        }
        if name
            .split('/')
            .any(|p| p == "__MACOSX" || p.starts_with('.'))
        {
            continue;
        }
        entries.push(Entry {
            index,
            path: name.clone(),
            byte_len: file.size(),
            kind: kind_of(&name),
        });
    }
    Ok(entries)
}

pub fn extract_entry(zip_path: &Path, entry: &Entry, scratch: &Path) -> Result<Extracted> {
    if !entry.kind.readable() {
        bail!("not a text document");
    }
    if entry.byte_len > 40 * 1024 * 1024 {
        bail!("file is larger than 40MB");
    }
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut zf = archive.by_index(entry.index)?;
    let mut bytes = Vec::with_capacity(entry.byte_len.min(40 * 1024 * 1024) as usize);
    zf.read_to_end(&mut bytes)?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let text = match entry.kind {
        Kind::Pdf => pdf_text(&bytes, scratch, entry.index)?,
        Kind::Docx => docx_text(&bytes)?,
        Kind::Odt => odt_text(&bytes)?,
        Kind::Rtf => rtf_text(&String::from_utf8_lossy(&bytes)),
        Kind::Text => String::from_utf8_lossy(&bytes).into_owned(),
        Kind::Other => unreachable!("kind filtered above"),
    };
    let (text, note) = cap_text(text);
    Ok(Extracted { text, sha256, note })
}

pub fn readable_sorted(entries: &[Entry]) -> Vec<Entry> {
    let mut docs: Vec<Entry> = entries
        .iter()
        .filter(|e| e.kind.readable())
        .cloned()
        .collect();
    docs.sort_by(|a, b| {
        rank(&a.path)
            .cmp(&rank(&b.path))
            .then_with(|| pdf_first(a.kind).cmp(&pdf_first(b.kind)))
            .then_with(|| a.path.cmp(&b.path))
    });
    docs
}

fn pdf_first(kind: Kind) -> u8 {
    match kind {
        Kind::Pdf => 0,
        Kind::Docx => 1,
        Kind::Odt => 2,
        _ => 3,
    }
}

fn rank(path: &str) -> u8 {
    let p = path.to_lowercase();
    if p.contains("font") || p.ends_with(".ttf") {
        return 9;
    }
    if p.contains("model") && !p.contains("wzorniczy") {
        0
    } else if p.contains("instruk") || p.contains("scenari") {
        1
    } else if p.contains("test") {
        2
    } else if p.contains("raport") {
        3
    } else if p.contains("folder") || p.contains("opis") {
        4
    } else if p.contains("wzorniczy") {
        8
    } else {
        5
    }
}

fn kind_of(path: &str) -> Kind {
    let lower = path.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "pdf" => Kind::Pdf,
        "docx" => Kind::Docx,
        "odt" => Kind::Odt,
        "txt" | "md" | "html" | "htm" => Kind::Text,
        "rtf" => Kind::Rtf,
        _ => Kind::Other,
    }
}

pub fn decode_zip_name(raw: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(raw) {
        return text.replace('\\', "/");
    }
    decode_cp852(raw).replace('\\', "/")
}

fn decode_cp852(raw: &[u8]) -> String {
    raw.iter()
        .map(|b| {
            if *b < 128 {
                *b as char
            } else {
                CP852[(*b - 128) as usize]
            }
        })
        .collect()
}

fn safe_zip_path(path: &str) -> bool {
    !path.starts_with('/') && !path.split('/').any(|p| p == "..")
}

fn cap_text(text: String) -> (String, String) {
    const MAX: usize = 180_000;
    let count = text.chars().count();
    if count <= MAX {
        return (text, String::new());
    }
    let clipped: String = text.chars().take(MAX).collect();
    (clipped, format!("truncated to {MAX} chars from {count}"))
}

fn pdf_text(bytes: &[u8], scratch: &Path, index: usize) -> Result<String> {
    std::fs::create_dir_all(scratch)?;
    let pdf_path: PathBuf = scratch.join(format!("entry-{index}.pdf"));
    std::fs::write(&pdf_path, bytes)?;
    let output = Command::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8", "-q"])
        .arg(&pdf_path)
        .arg("-")
        .output()
        .context("pdftotext is not installed")?;
    let _ = std::fs::remove_file(&pdf_path);
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("pdftotext failed: {err}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn docx_text(bytes: &[u8]) -> Result<String> {
    let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).context("docx zip")?;
    let mut xml = String::new();
    for name in [
        "word/document.xml",
        "word/header1.xml",
        "word/header2.xml",
        "word/footer1.xml",
    ] {
        if let Ok(mut part) = archive.by_name(name) {
            let mut buf = String::new();
            part.read_to_string(&mut buf)?;
            xml.push_str(&buf);
            xml.push('\n');
        }
    }
    if xml.is_empty() {
        bail!("docx has no word/document.xml");
    }
    Ok(office_xml_text(&xml))
}

fn odt_text(bytes: &[u8]) -> Result<String> {
    let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).context("odt zip")?;
    let mut part = archive.by_name("content.xml").context("odt content.xml")?;
    let mut xml = String::new();
    part.read_to_string(&mut xml)?;
    Ok(office_xml_text(&xml))
}

fn office_xml_text(xml: &str) -> String {
    let marked = xml
        .replace("</w:p>", "\n")
        .replace("<w:br/>", "\n")
        .replace("<w:br />", "\n")
        .replace("<w:tab/>", "\t")
        .replace("</text:p>", "\n")
        .replace("<text:line-break/>", "\n")
        .replace("<text:tab/>", "\t");
    let stripped = strip_tags(&marked);
    decode_xml_entities(&stripped)
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rtf_text(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if chars.peek() == Some(&'\n') || chars.peek() == Some(&'\r') {
                    chars.next();
                    continue;
                }
                let mut word = String::new();
                while let Some(n) = chars.peek() {
                    if n.is_ascii_alphabetic() {
                        word.push(*n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars
                    .peek()
                    .is_some_and(|n| n.is_ascii_digit() || *n == '-')
                {
                    while chars
                        .peek()
                        .is_some_and(|n| n.is_ascii_digit() || *n == '-')
                    {
                        chars.next();
                    }
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                if word == "par" || word == "line" {
                    out.push('\n');
                }
            }
            '{' | '}' => {}
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_tags(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut in_tag = false;
    for c in xml.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_xml_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_polish_cp852() {
        // "Załączniki" as CP852, checked against Python's cp852 codec.
        let raw = [b'Z', b'a', 0x88, 0xA5, b'c', b'z', b'n', b'i', b'k', b'i'];
        assert_eq!(decode_zip_name(&raw), "Załączniki");
    }

    #[test]
    fn utf8_names_pass_through() {
        assert_eq!(
            decode_zip_name("model/opis.pdf".as_bytes()),
            "model/opis.pdf"
        );
    }

    #[test]
    fn docx_paragraphs() {
        let xml = r#"<?xml version="1.0"?><w:document><w:p><w:r><w:t>Seniorzy</w:t></w:r></w:p><w:p><w:r><w:t>i &amp; opieka</w:t></w:r></w:p></w:document>"#;
        let text = office_xml_text(xml);
        assert!(text.contains("Seniorzy"));
        assert!(text.contains("i & opieka"));
    }
}
