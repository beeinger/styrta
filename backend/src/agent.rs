//! One innovation, one agent run.
//!
//! The model chooses which archive files to read. Rust extracts the text and
//! refuses a digest that skips the files or invents URLs. Links are attached
//! by the caller, not by the model.

use crate::extract::{self, Entry, Extracted, Kind};
use crate::llm::Llm;
use crate::scrape::{Detail, Link};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub const AGENT_VERSION: i32 = 2;
const READ_CHARS: usize = 7000;
const MAX_ROUNDS: usize = 12;
const MAX_READS: usize = 8;

#[derive(Debug)]
pub struct Digest {
    pub title: String,
    pub knowledge_text: String,
    pub files_read: Vec<String>,
    pub caveats: String,
    pub raw: Value,
}

pub struct ArchiveSession {
    zip_path: std::path::PathBuf,
    scratch: std::path::PathBuf,
    entries: Vec<Entry>,
    cache: HashMap<String, Extracted>,
    reads: usize,
}

impl ArchiveSession {
    pub fn open(zip_path: &Path, scratch: &Path) -> Result<Self> {
        let entries = extract::index_zip(zip_path)?;
        Ok(Self {
            zip_path: zip_path.to_path_buf(),
            scratch: scratch.to_path_buf(),
            entries,
            cache: HashMap::new(),
            reads: 0,
        })
    }

    pub fn documents(&self) -> Vec<Entry> {
        extract::readable_sorted(&self.entries)
    }

    pub fn skipped_binary(&self) -> usize {
        self.entries.iter().filter(|e| !e.kind.readable()).count()
    }

    fn find(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    pub async fn read(&mut self, path: &str, offset: usize) -> Result<String> {
        if self.reads >= MAX_READS {
            return Ok("Read budget is spent. Call submit_digest with what you have.".into());
        }
        let Some(entry) = self.find(path).cloned() else {
            return Ok(format!("unknown path: {path}"));
        };
        if !entry.kind.readable() {
            return Ok("this file is not a pdf, docx, odt, rtf, or text file".into());
        }
        self.reads += 1;
        let key = entry.path.clone();
        if !self.cache.contains_key(&key) {
            let zip_path = self.zip_path.clone();
            let scratch = self.scratch.clone();
            let entry_for_extract = entry.clone();
            let extracted = tokio::task::spawn_blocking(move || {
                extract::extract_entry(&zip_path, &entry_for_extract, &scratch)
            })
            .await
            .context("extract task")??;
            self.cache.insert(key, extracted);
        }
        let extracted = self.cache.get(&entry.path).expect("cached");
        Ok(window(&entry.path, entry.kind, extracted, offset))
    }

    pub fn cached(&self) -> &HashMap<String, Extracted> {
        &self.cache
    }
}

pub async fn run(
    llm: &Llm,
    detail: &Detail,
    page_url: &str,
    categories: &[String],
    mut archive: Option<&mut ArchiveSession>,
) -> Result<Digest> {
    let docs = archive.as_ref().map(|a| a.documents()).unwrap_or_default();
    let skipped = archive.as_ref().map(|a| a.skipped_binary()).unwrap_or(0);
    let mut messages = vec![
        json!({"role": "system", "content": SYSTEM}),
        json!({"role": "user", "content": user_prompt(detail, page_url, categories, &docs, skipped)}),
    ];
    let tools = tools_schema();
    let mut had_text = false;

    for _ in 0..MAX_ROUNDS {
        let reply = llm.chat(&messages, &tools).await?;
        if reply.tool_calls.is_empty() {
            messages.push(json!({
                "role": "assistant",
                "content": reply.content.unwrap_or_default(),
            }));
            messages.push(json!({
                "role": "user",
                "content": "Call a tool. Read a file, or call submit_digest.",
            }));
            continue;
        }

        let reads_in_turn = reply.tool_calls.iter().any(|c| c.name == "read_file");
        let assistant_calls: Vec<Value> = reply
            .tool_calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": {"name": c.name, "arguments": c.arguments},
                })
            })
            .collect();
        messages.push(json!({
            "role": "assistant",
            "content": reply.content,
            "tool_calls": assistant_calls,
        }));

        for call in &reply.tool_calls {
            let result = match call.name.as_str() {
                "list_files" => Ok(list_files(&docs, skipped)),
                "read_file" => match archive.as_deref_mut() {
                    Some(session) => {
                        let args: ReadArgs =
                            serde_json::from_str(&call.arguments).unwrap_or(ReadArgs {
                                path: String::new(),
                                offset: 0,
                            });
                        match session.read(&args.path, args.offset).await {
                            Ok(text) => {
                                if text_has_body(&text) {
                                    had_text = true;
                                }
                                Ok(text)
                            }
                            Err(err) => Ok(format!("error reading file: {err:#}")),
                        }
                    }
                    None => Ok("this innovation has no archive".into()),
                },
                "submit_digest" => {
                    if reads_in_turn {
                        Err("do not call submit_digest in the same turn as read_file; wait for the tool result".into())
                    } else {
                        match parse_digest(&call.arguments, &docs, had_text) {
                            Ok(digest) => return Ok(digest),
                            Err(err) => Err(err),
                        }
                    }
                }
                other => Err(format!("unknown tool {other}")),
            };
            let content = match result {
                Ok(text) => text,
                Err(err) => format!("error: {err}"),
            };
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": content,
            }));
        }
    }
    bail!("agent did not call submit_digest");
}

pub fn compose(detail: &Detail, page_url: &str, categories: &[String], digest: &Digest) -> String {
    let mut out = String::new();
    out.push_str(&digest.title);
    out.push_str("\n\nŹródło: Biblioteka Innowacji Społecznych, ROPS Kraków\nStrona: ");
    out.push_str(page_url);
    if !categories.is_empty() {
        out.push_str("\nKategorie: ");
        out.push_str(&categories.join(", "));
    }
    if let Some(licence) = &detail.licence {
        out.push_str("\nLicencja: ");
        out.push_str(licence);
    }
    out.push_str("\n\n");
    out.push_str(digest.knowledge_text.trim());
    out.push_str("\n\nOdnośniki:\n");
    for link in &detail.links {
        let label = link.label.as_deref().unwrap_or(link.kind);
        out.push_str("- ");
        out.push_str(label);
        out.push_str(": ");
        out.push_str(&link.url);
        out.push('\n');
    }
    out
}

#[derive(Deserialize)]
struct ReadArgs {
    #[serde(default)]
    path: String,
    #[serde(default)]
    offset: usize,
}

#[derive(Deserialize)]
struct RawDigest {
    title: String,
    summary: String,
    knowledge_text: String,
    problem: String,
    how_it_works: String,
    audience: String,
    who_can_use: String,
    evidence: String,
    authors: String,
    #[serde(default)]
    files_read: Vec<String>,
    #[serde(default)]
    caveats: String,
}

fn parse_digest(
    arguments: &str,
    docs: &[Entry],
    had_text: bool,
) -> std::result::Result<Digest, String> {
    let raw: Value = serde_json::from_str(arguments)
        .map_err(|err| format!("submit_digest arguments are not JSON: {err}"))?;
    let parsed: RawDigest = serde_json::from_value(raw.clone())
        .map_err(|err| format!("submit_digest is missing fields: {err}"))?;
    let knowledge = parsed.knowledge_text.trim();
    let min = if had_text { 700 } else { 250 };
    if knowledge.chars().count() < min {
        return Err(format!(
            "knowledge_text is too short ({min} characters minimum). Write a standalone brief from the materials."
        ));
    }
    if knowledge.contains("http://") || knowledge.contains("https://") {
        return Err("remove URLs from knowledge_text. Links are stored separately.".into());
    }
    if had_text && parsed.files_read.is_empty() {
        return Err("files_read is empty. List the paths you actually read.".into());
    }
    for path in &parsed.files_read {
        if !docs.iter().any(|d| d.path == *path) {
            return Err(format!("files_read has an unknown path: {path}"));
        }
    }
    for (name, value) in [
        ("summary", parsed.summary.trim()),
        ("problem", parsed.problem.trim()),
        ("how_it_works", parsed.how_it_works.trim()),
        ("audience", parsed.audience.trim()),
        ("who_can_use", parsed.who_can_use.trim()),
        ("evidence", parsed.evidence.trim()),
        ("authors", parsed.authors.trim()),
    ] {
        if value.is_empty() {
            return Err(format!(
                "{name} is empty. If the materials do not say, write 'brak w materiałach'."
            ));
        }
    }
    Ok(Digest {
        title: parsed.title.trim().to_string(),
        knowledge_text: knowledge.to_string(),
        files_read: parsed.files_read,
        caveats: parsed.caveats.trim().to_string(),
        raw,
    })
}

fn window(path: &str, kind: Kind, extracted: &Extracted, offset: usize) -> String {
    let chars: Vec<char> = extracted.text.chars().collect();
    let total = chars.len();
    if total == 0 {
        let note = if extracted.note.is_empty() {
            String::new()
        } else {
            format!(" {}", extracted.note)
        };
        return format!(
            "{path} ({}) has no extractable text.{note} It may be a scan. {}",
            kind.as_str(),
            twin_hint(path)
        );
    }
    let start = offset.min(total);
    let end = (start + READ_CHARS).min(total);
    let slice: String = chars[start..end].iter().collect();
    let next = if end < total {
        format!("\n\n[truncated, next_offset={end}, total_chars={total}]")
    } else {
        format!("\n\n[end, total_chars={total}]")
    };
    format!(
        "{path} ({}, chars {start}..{end} of {total})\n{slice}{next}",
        kind.as_str()
    )
}

fn twin_hint(path: &str) -> String {
    let lower = path.to_lowercase();
    if lower.ends_with(".pdf") {
        "If a docx with the same name exists, read that.".into()
    } else {
        String::new()
    }
}

fn text_has_body(windowed: &str) -> bool {
    windowed.contains("chars ") && !windowed.contains("no extractable text")
}

fn list_files(docs: &[Entry], skipped: usize) -> String {
    let mut lines = Vec::new();
    for entry in docs {
        lines.push(format!(
            "{}  {}  {} bytes",
            entry.kind.as_str(),
            entry.path,
            entry.byte_len
        ));
    }
    if lines.is_empty() {
        return format!("no pdf, docx, odt, or text files. skipped binary files: {skipped}");
    }
    format!(
        "{}\nskipped binary files (fonts, illustrator, indesign): {skipped}",
        lines.join("\n")
    )
}

fn user_prompt(
    detail: &Detail,
    page_url: &str,
    categories: &[String],
    docs: &[Entry],
    skipped: usize,
) -> String {
    let mut sections = String::new();
    if !detail.intro.is_empty() {
        sections.push_str("Wstęp:\n");
        sections.push_str(&detail.intro);
        sections.push_str("\n\n");
    }
    for section in &detail.sections {
        sections.push_str(&section.heading);
        sections.push_str("\n");
        sections.push_str(&section.body);
        sections.push_str("\n\n");
    }
    if sections.trim().is_empty() {
        sections.push_str("(brak pól na stronie)\n");
    }
    let links = detail
        .links
        .iter()
        .map(link_line)
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Innowacja: {title}\nURL: {page_url}\nKategorie: {categories}\nLicencja: {licence}\n\nPola ze strony:\n{sections}\nOdnośniki (nie wklejaj ich do knowledge_text, są zapisywane osobno):\n{links}\n\nPliki w archiwum, w kolejności od najważniejszych:\n{files}\n\nPrzeczytaj najwyżej cztery pliki. Najpierw model lub opis, potem instrukcję albo scenariusze, potem raport z testowania. Gdy jest para pdf i docx o tej samej nazwie, czytaj pdf, a docx tylko gdy pdf nie ma tekstu. Potem wywołaj submit_digest.",
        title = if detail.title.is_empty() { "(brak tytułu)" } else { detail.title.as_str() },
        categories = if categories.is_empty() { "(brak)".into() } else { categories.join(", ") },
        licence = detail.licence.as_deref().unwrap_or("niepodana"),
        files = list_files(docs, skipped),
    )
}

fn link_line(link: &Link) -> String {
    format!(
        "- {} {}: {}",
        link.kind,
        link.label.as_deref().unwrap_or(""),
        link.url
    )
}

fn tools_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "list_files",
                "description": "List readable files in the innovation archive.",
                "parameters": {"type": "object", "properties": {}, "additionalProperties": false}
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Extract text from one pdf, docx, odt, rtf, or text file. Returns at most 7000 characters from offset.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Path exactly as listed"},
                        "offset": {"type": "integer", "description": "Character offset, default 0"}
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "submit_digest",
                "description": "Submit the final Polish brief. No URLs in knowledge_text.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "summary": {"type": "string", "description": "Two or three sentences"},
                        "knowledge_text": {"type": "string", "description": "Standalone Polish brief, grounded only in the page and the files you read"},
                        "problem": {"type": "string"},
                        "how_it_works": {"type": "string"},
                        "audience": {"type": "string"},
                        "who_can_use": {"type": "string"},
                        "evidence": {"type": "string"},
                        "authors": {"type": "string"},
                        "files_read": {"type": "array", "items": {"type": "string"}},
                        "caveats": {"type": "string", "description": "What you could not read or what the materials do not say"}
                    },
                    "required": ["title", "summary", "knowledge_text", "problem", "how_it_works", "audience", "who_can_use", "evidence", "authors", "files_read", "caveats"]
                }
            }
        }
    ])
}

const SYSTEM: &str = r#"Jesteś redaktorem indeksu Biblioteki Innowacji Społecznych ROPS Kraków. Dostajesz jedną innowację: pola ze strony i listę plików z archiwum ZIP.

Zasady:
- Pisz po polsku.
- Używaj tylko faktów z pól strony albo z plików, które odczytałeś narzędziem read_file. Nie dopowiadaj liczb, instytucji, skutków ani grup, których nie ma w materiale.
- Nie wstawiaj adresów URL. Odnośniki zapisuje program.
- knowledge_text ma być samodzielną notatką do późniejszego wyszukiwania: czym jest innowacja, jaki problem rozwiązuje, dla kogo jest, jak działa w praktyce, co wiadomo z testów, kto jest autorem, czego brakuje. Wspomnij nazwy plików, z których korzystasz.
- Gdy archiwum nie ma plików tekstowych, oprzyj notatkę na polach strony i napisz to w caveats.
- Nie scalaj dwóch wariantów w jedną regułę. Jeśli instrukcja podaje inny czas sesji albo inny przebieg dla scenariusza prostego i złożonego, zapisz oba.
- Gdy wymieniasz elementy zestawu, liczba pozycji musi być taka jak w źródle. Jeśli elementy są parami, opisz pary. Nie spłaszczaj ich do jednej listy, która gubi pozycję.
- Nie dodawaj „ok.”, „około” ani „prawie” do liczby, którą źródło podaje wprost.
- Gdy strona twierdzi, że test potwierdził efekt, a raport testu mówi, że efektu nie zmierzono, trzymaj się raportu. Napisz, że strona twierdzi więcej, niż raport potwierdza.
- Skończ wywołaniem submit_digest."#;

pub fn ensure_pdftotext() -> Result<()> {
    which_pdftotext().context("pdftotext (poppler-utils) is required to read innovation PDFs")
}

fn which_pdftotext() -> Result<()> {
    let status = std::process::Command::new("pdftotext")
        .arg("-v")
        .output()
        .context("spawn pdftotext")?;
    if status.status.success() || !status.stderr.is_empty() || !status.stdout.is_empty() {
        return Ok(());
    }
    bail!("pdftotext failed")
}
