use crate::{
    AiProvider, Attachment, AttachmentKind, TempFileGuard,
    assistant::{AiFile, AiInputPart, AiMessage, AiSnapshot},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::{DeflateDecoder, ZlibDecoder};
use quick_xml::{
    Reader,
    escape::resolve_xml_entity,
    events::Event,
};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};
use zip::ZipArchive;

pub const MAX_AI_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_AI_NATIVE_PDF_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_AI_DOC_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_AI_CHUNKS: usize = 8;
const MAX_XML_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PDF_STREAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_CHUNK_TEXT_BYTES: usize = 160 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("Unsupported file format: {0}")]
    UnsupportedFileFormat(String),
    #[error("当前模型或接口不支持直接读取扫描版 PDF，请换用支持原生 PDF/视觉的模型")]
    ScannedPdfUnsupported,
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub enum AuthorizedAiAttachment {
    Cached(PathBuf),
    Temporary {
        path: PathBuf,
        _temp_guard: TempFileGuard,
    },
}

impl AuthorizedAiAttachment {
    pub fn path(&self) -> &Path {
        match self {
            Self::Cached(path) | Self::Temporary { path, .. } => path,
        }
    }

    pub fn is_cached(&self) -> bool {
        matches!(self, Self::Cached(_))
    }
}

impl Drop for AuthorizedAiAttachment {
    fn drop(&mut self) {
        if let Self::Temporary { path, .. } = self {
            let _ = std::fs::remove_file(&*path);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportedAttachmentKind {
    ImageJpeg,
    ImagePng,
    ImageWebp,
    Pdf,
    Docx,
    Txt,
    Markdown,
    Xlsx,
    Csv,
}

impl SupportedAttachmentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::ImageJpeg => "JPEG",
            Self::ImagePng => "PNG",
            Self::ImageWebp => "WebP",
            Self::Pdf => "PDF",
            Self::Docx => "DOCX",
            Self::Txt => "TXT",
            Self::Markdown => "Markdown",
            Self::Xlsx => "XLSX",
            Self::Csv => "CSV",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::ImageJpeg => "image/jpeg",
            Self::ImagePng => "image/png",
            Self::ImageWebp => "image/webp",
            Self::Pdf => "application/pdf",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Txt => "text/plain",
            Self::Markdown => "text/markdown",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Csv => "text/csv",
        }
    }
}

pub fn classify_attachment(attachment: &Attachment) -> Result<SupportedAttachmentKind, ExtractError> {
    if attachment.kind != AttachmentKind::File {
        return Err(ExtractError::UnsupportedFileFormat(format!(
            "{} (folder)",
            attachment.name
        )));
    }
    let ext = attachment
        .name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => Ok(SupportedAttachmentKind::ImageJpeg),
        "png" => Ok(SupportedAttachmentKind::ImagePng),
        "webp" => Ok(SupportedAttachmentKind::ImageWebp),
        "pdf" => Ok(SupportedAttachmentKind::Pdf),
        "docx" => Ok(SupportedAttachmentKind::Docx),
        "txt" => Ok(SupportedAttachmentKind::Txt),
        "md" | "markdown" => Ok(SupportedAttachmentKind::Markdown),
        "xlsx" => Ok(SupportedAttachmentKind::Xlsx),
        "csv" => Ok(SupportedAttachmentKind::Csv),
        _ => Err(ExtractError::UnsupportedFileFormat(attachment.name.clone())),
    }
}

pub fn attachment_type_label(attachment: &Attachment) -> String {
    if attachment.kind == AttachmentKind::Folder {
        return "Folder".into();
    }
    if let Ok(kind) = classify_attachment(attachment) {
        return kind.label().into();
    }
    attachment
        .name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_uppercase())
        .filter(|ext| !ext.is_empty())
        .unwrap_or_else(|| "File".into())
}

pub fn check_supported_attachment(attachment: &Attachment) -> Result<(), ExtractError> {
    let kind = classify_attachment(attachment)?;
    match kind {
        SupportedAttachmentKind::ImageJpeg
        | SupportedAttachmentKind::ImagePng
        | SupportedAttachmentKind::ImageWebp => {
            if attachment.size > MAX_AI_IMAGE_BYTES {
                return Err(ExtractError::Other(anyhow::anyhow!(
                    "Image '{}' exceeds the 16 MiB limit for AI requests",
                    attachment.name
                )));
            }
        }
        _ => {
            if attachment.size > MAX_AI_DOC_BYTES {
                return Err(ExtractError::Other(anyhow::anyhow!(
                    "File '{}' exceeds the 32 MiB extraction limit",
                    attachment.name
                )));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum ExtractedAttachment {
    Image {
        name: String,
        image_url: String,
    },
    NativePdf {
        name: String,
        file_data: String,
    },
    TextChunks {
        name: String,
        chunks: Vec<String>,
    },
}

pub fn extract_attachment(
    path: &Path,
    attachment: &Attachment,
    provider: AiProvider,
) -> Result<ExtractedAttachment, ExtractError> {
    let kind = classify_attachment(attachment)?;
    check_supported_attachment(attachment)?;

    let file = File::open(path).with_context(|| format!("Could not open '{}'", attachment.name))?;
    let meta = file
        .metadata()
        .with_context(|| format!("Could not inspect '{}'", attachment.name))?;
    if !meta.is_file() {
        return Err(ExtractError::UnsupportedFileFormat(attachment.name.clone()));
    }

    match kind {
        SupportedAttachmentKind::ImageJpeg
        | SupportedAttachmentKind::ImagePng
        | SupportedAttachmentKind::ImageWebp => {
            let bytes = read_bounded(file, MAX_AI_IMAGE_BYTES, &attachment.name)?;
            validate_image_magic(kind, &bytes, &attachment.name)?;
            let mime = kind.mime();
            Ok(ExtractedAttachment::Image {
                name: attachment.name.clone(),
                image_url: format!("data:{mime};base64,{}", STANDARD.encode(&bytes)),
            })
        }
        SupportedAttachmentKind::Txt
        | SupportedAttachmentKind::Markdown
        | SupportedAttachmentKind::Csv => {
            let bytes = read_bounded(file, MAX_AI_DOC_BYTES, &attachment.name)?;
            if bytes.contains(&0) {
                return Err(ExtractError::UnsupportedFileFormat(attachment.name.clone()));
            }
            let text = String::from_utf8(bytes).map_err(|_| {
                ExtractError::Other(anyhow::anyhow!(
                    "File '{}' is not valid UTF-8 text",
                    attachment.name
                ))
            })?;
            let chunks = split_text_chunks(&text, MAX_CHUNK_TEXT_BYTES)?;
            Ok(ExtractedAttachment::TextChunks {
                name: attachment.name.clone(),
                chunks,
            })
        }
        SupportedAttachmentKind::Docx => {
            let text = extract_docx_text(file, &attachment.name)?;
            let chunks = split_text_chunks(&text, MAX_CHUNK_TEXT_BYTES)?;
            Ok(ExtractedAttachment::TextChunks {
                name: attachment.name.clone(),
                chunks,
            })
        }
        SupportedAttachmentKind::Xlsx => {
            let sheets = extract_xlsx_sheets(file, &attachment.name)?;
            let chunks = pack_sections_into_chunks(&sheets, MAX_CHUNK_TEXT_BYTES)?;
            Ok(ExtractedAttachment::TextChunks {
                name: attachment.name.clone(),
                chunks,
            })
        }
        SupportedAttachmentKind::Pdf => {
            let bytes = read_bounded(file, MAX_AI_DOC_BYTES, &attachment.name)?;
            if !bytes.starts_with(b"%PDF-") {
                return Err(ExtractError::UnsupportedFileFormat(attachment.name.clone()));
            }
            if provider == AiProvider::OpenAi && bytes.len() as u64 <= MAX_AI_NATIVE_PDF_BYTES {
                return Ok(ExtractedAttachment::NativePdf {
                    name: attachment.name.clone(),
                    file_data: format!("data:application/pdf;base64,{}", STANDARD.encode(&bytes)),
                });
            }
            let pages = extract_pdf_pages(&bytes)?;
            let chunks = pack_sections_into_chunks(&pages, MAX_CHUNK_TEXT_BYTES)?;
            Ok(ExtractedAttachment::TextChunks {
                name: attachment.name.clone(),
                chunks,
            })
        }
    }
}

pub fn build_chunked_requests(
    prompt: &str,
    messages: &[AiMessage],
    local_files: &[AiFile],
    extracted: &[ExtractedAttachment],
) -> Result<Vec<Vec<AiInputPart>>> {
    let mut text_docs: Vec<(String, Vec<String>)> = Vec::new();
    let mut media_parts: Vec<AiInputPart> = Vec::new();
    for item in extracted {
        match item {
            ExtractedAttachment::Image { image_url, .. } => {
                media_parts.push(AiInputPart::InputImage {
                    image_url: image_url.clone(),
                });
            }
            ExtractedAttachment::NativePdf { name, file_data } => {
                media_parts.push(AiInputPart::InputFile {
                    filename: name.clone(),
                    file_data: file_data.clone(),
                });
            }
            ExtractedAttachment::TextChunks { name, chunks } => {
                text_docs.push((name.clone(), chunks.clone()));
            }
        }
    }

    if text_docs.is_empty() {
        let input = AiSnapshot {
            prompt: prompt.to_owned(),
            messages: messages.to_vec(),
            files: local_files.to_vec(),
        }
        .encode()?;
        let mut parts = vec![AiInputPart::InputText { text: input }];
        parts.extend(media_parts);
        return Ok(vec![parts]);
    }

    // Try packing all extracted document text into a single snapshot first.
    let single_files: Vec<AiFile> = local_files
        .iter()
        .cloned()
        .chain(text_docs.iter().map(|(name, chunks)| AiFile {
            name: name.clone(),
            text: chunks.join("\n\n"),
        }))
        .collect();
    if let Ok(input) = (AiSnapshot {
        prompt: prompt.to_owned(),
        messages: messages.to_vec(),
        files: single_files,
    })
    .encode()
    {
        let mut parts = vec![AiInputPart::InputText { text: input }];
        parts.extend(media_parts);
        return Ok(vec![parts]);
    }

    // Otherwise distribute extracted document chunks sequentially, bounded by MAX_AI_CHUNKS.
    let mut segments: Vec<AiFile> = Vec::new();
    for (name, chunks) in text_docs {
        let total = chunks.len();
        for (idx, chunk) in chunks.into_iter().enumerate() {
            let label = if total > 1 {
                format!("{name} (chunk {}/{})", idx + 1, total)
            } else {
                name.clone()
            };
            segments.push(AiFile {
                name: label,
                text: chunk,
            });
        }
    }

    let mut requests: Vec<Vec<AiInputPart>> = Vec::new();
    let mut current_batch: Vec<AiFile> = Vec::new();
    for segment in segments {
        let mut candidate: Vec<AiFile> = if requests.is_empty() {
            local_files.to_vec()
        } else {
            Vec::new()
        };
        candidate.extend(current_batch.iter().cloned());
        candidate.push(segment.clone());

        let snapshot = AiSnapshot {
            prompt: prompt.to_owned(),
            messages: messages.to_vec(),
            files: candidate,
        };
        if snapshot.encode().is_ok() {
            current_batch.push(segment);
            continue;
        }

        if current_batch.is_empty() {
            bail!(
                "Extracted content from '{}' exceeds the per-request limit even after splitting.",
                segment.name
            );
        }
        let mut flush_files = if requests.is_empty() {
            local_files.to_vec()
        } else {
            Vec::new()
        };
        flush_files.extend(std::mem::take(&mut current_batch));
        let input = AiSnapshot {
            prompt: prompt.to_owned(),
            messages: messages.to_vec(),
            files: flush_files,
        }
        .encode()?;
        let mut parts = vec![AiInputPart::InputText { text: input }];
        if requests.is_empty() {
            parts.extend(media_parts.clone());
        }
        requests.push(parts);
        ensure!(
            requests.len() < MAX_AI_CHUNKS,
            "Selected content exceeds the maximum of {MAX_AI_CHUNKS} sequential AI chunks. Choose fewer files or messages."
        );
        current_batch.push(segment);
    }

    if !current_batch.is_empty() {
        let mut flush_files = if requests.is_empty() {
            local_files.to_vec()
        } else {
            Vec::new()
        };
        flush_files.extend(current_batch);
        let input = AiSnapshot {
            prompt: prompt.to_owned(),
            messages: messages.to_vec(),
            files: flush_files,
        }
        .encode()?;
        let mut parts = vec![AiInputPart::InputText { text: input }];
        if requests.is_empty() {
            parts.extend(media_parts);
        }
        requests.push(parts);
    }

    ensure!(
        requests.len() <= MAX_AI_CHUNKS,
        "Selected content exceeds the maximum of {MAX_AI_CHUNKS} sequential AI chunks. Choose fewer files or messages."
    );
    Ok(requests)
}

fn read_bounded(file: File, max_bytes: u64, name: &str) -> Result<Vec<u8>, ExtractError> {
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("Could not read '{name}'"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "File '{name}' exceeds size limit"
        )));
    }
    Ok(bytes)
}

fn validate_image_magic(
    kind: SupportedAttachmentKind,
    bytes: &[u8],
    name: &str,
) -> Result<(), ExtractError> {
    let valid = match kind {
        SupportedAttachmentKind::ImageJpeg => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        SupportedAttachmentKind::ImagePng => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        SupportedAttachmentKind::ImageWebp => {
            bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
        }
        _ => false,
    };
    if !valid {
        return Err(ExtractError::UnsupportedFileFormat(name.to_owned()));
    }
    Ok(())
}

fn split_text_chunks(text: &str, max_bytes: usize) -> Result<Vec<String>, ExtractError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(vec![String::new()]);
    }
    if trimmed.len() <= max_bytes {
        return Ok(vec![trimmed.to_owned()]);
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in trimmed.split_inclusive('\n') {
        if line.len() > max_bytes {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            let mut rest = line;
            while !rest.is_empty() {
                let mut end = rest.len().min(max_bytes);
                while end > 0 && !rest.is_char_boundary(end) {
                    end -= 1;
                }
                if end == 0 {
                    break;
                }
                chunks.push(rest[..end].to_owned());
                rest = &rest[end..];
            }
            continue;
        }
        if current.len() + line.len() > max_bytes && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.len() > MAX_AI_CHUNKS {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "File exceeds the maximum of {MAX_AI_CHUNKS} sequential AI chunks"
        )));
    }
    Ok(chunks)
}

fn pack_sections_into_chunks(
    sections: &[String],
    max_bytes: usize,
) -> Result<Vec<String>, ExtractError> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for section in sections {
        let section = section.trim();
        if section.is_empty() {
            continue;
        }
        if section.len() > max_bytes {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.extend(split_text_chunks(section, max_bytes)?);
            continue;
        }
        let sep_len = if current.is_empty() { 0 } else { 2 };
        if current.len() + sep_len + section.len() > max_bytes && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(section);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    if chunks.len() > MAX_AI_CHUNKS {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "File exceeds the maximum of {MAX_AI_CHUNKS} sequential AI chunks"
        )));
    }
    Ok(chunks)
}

fn read_zip_entry_utf8(
    archive: &mut ZipArchive<BufReader<File>>,
    entry_name: &str,
    doc_name: &str,
) -> Result<String, ExtractError> {
    let entry = archive
        .by_name(entry_name)
        .map_err(|_| ExtractError::UnsupportedFileFormat(doc_name.to_owned()))?;
    let mut bytes = Vec::new();
    entry
        .take(MAX_XML_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("Could not read '{entry_name}' in '{doc_name}'"))?;
    if bytes.len() as u64 > MAX_XML_ENTRY_BYTES {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "Document '{doc_name}' XML content is too large"
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| ExtractError::Other(anyhow::anyhow!("Invalid UTF-8 in '{doc_name}'")))
        .map_err(Into::into)
}

fn append_general_ref(out: &mut String, reference: &quick_xml::events::BytesRef<'_>) {
    if let Ok(Some(ch)) = reference.resolve_char_ref() {
        out.push(ch);
        return;
    }
    if let Some(resolved) = resolve_xml_entity(reference.as_ref()) {
        out.push_str(resolved);
    }
}

fn extract_docx_text(file: File, doc_name: &str) -> Result<String, ExtractError> {
    let mut archive = ZipArchive::new(BufReader::new(file))
        .map_err(|_| ExtractError::UnsupportedFileFormat(doc_name.to_owned()))?;
    let xml = read_zip_entry_utf8(&mut archive, "word/document.xml", doc_name)?;
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(false);

    let mut out = String::new();
    let mut in_text = false;
    let mut in_cell = false;
    let mut cell_has_para = false;
    let mut row_has_cell = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                "t" => in_text = true,
                "tr" => row_has_cell = false,
                "tc" => {
                    if row_has_cell {
                        out.push_str(" | ");
                    }
                    in_cell = true;
                    cell_has_para = false;
                }
                "p" => {
                    if in_cell && cell_has_para {
                        out.push(' ');
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match e.local_name().as_ref() {
                "tab" => out.push('\t'),
                "br" | "cr" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(ref e)) => {
                if in_text {
                    out.push_str(e.as_ref());
                }
            }
            Ok(Event::GeneralRef(ref e)) => {
                if in_text {
                    append_general_ref(&mut out, e);
                }
            }
            Ok(Event::End(ref e)) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "p" => {
                    if in_cell {
                        cell_has_para = true;
                    } else if !out.ends_with('\n') && !out.is_empty() {
                        out.push('\n');
                    }
                }
                "tc" => {
                    in_cell = false;
                    row_has_cell = true;
                }
                "tr" => {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Other(anyhow::anyhow!(
                    "Could not parse DOCX XML in '{doc_name}': {err}"
                )));
            }
            _ => {}
        }
    }
    Ok(out.trim().to_owned())
}

fn extract_xlsx_sheets(file: File, doc_name: &str) -> Result<Vec<String>, ExtractError> {
    let mut archive = ZipArchive::new(BufReader::new(file))
        .map_err(|_| ExtractError::UnsupportedFileFormat(doc_name.to_owned()))?;

    let shared_strings = if archive.by_name("xl/sharedStrings.xml").is_ok() {
        let xml = read_zip_entry_utf8(&mut archive, "xl/sharedStrings.xml", doc_name)?;
        parse_xlsx_shared_strings(&xml)?
    } else {
        Vec::new()
    };

    let rels = if archive.by_name("xl/_rels/workbook.xml.rels").is_ok() {
        let xml = read_zip_entry_utf8(&mut archive, "xl/_rels/workbook.xml.rels", doc_name)?;
        parse_xlsx_rels(&xml)?
    } else {
        HashMap::new()
    };

    let workbook_xml = read_zip_entry_utf8(&mut archive, "xl/workbook.xml", doc_name)?;
    let sheets = parse_xlsx_workbook(&workbook_xml, &rels);

    let mut sections = Vec::new();
    for (idx, (sheet_name, target_path)) in sheets.into_iter().enumerate() {
        let fallback_path = format!("xl/worksheets/sheet{}.xml", idx + 1);
        let entry_path = target_path.unwrap_or(fallback_path);
        if archive.by_name(&entry_path).is_err() {
            continue;
        }
        let sheet_xml = read_zip_entry_utf8(&mut archive, &entry_path, doc_name)?;
        let rows = parse_xlsx_sheet(&sheet_xml, &shared_strings)?;
        if rows.is_empty() {
            sections.push(format!("[Sheet: {sheet_name}]\n(empty)"));
        } else {
            sections.push(format!("[Sheet: {sheet_name}]\n{}", rows.join("\n")));
        }
    }

    if sections.is_empty() {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "No readable worksheets found in '{doc_name}'"
        )));
    }
    Ok(sections)
}

fn parse_xlsx_shared_strings(xml: &str) -> Result<Vec<String>, ExtractError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut strings = Vec::new();
    let mut current = String::new();
    let mut in_si = false;
    let mut in_t = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                "si" => {
                    in_si = true;
                    current.clear();
                }
                "t" if in_si => in_t = true,
                _ => {}
            },
            Ok(Event::Text(ref e)) if in_t => current.push_str(e.as_ref()),
            Ok(Event::GeneralRef(ref e)) if in_t => append_general_ref(&mut current, e),
            Ok(Event::End(ref e)) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "si" => {
                    in_si = false;
                    strings.push(std::mem::take(&mut current));
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Other(anyhow::anyhow!(
                    "Invalid sharedStrings.xml: {err}"
                )));
            }
            _ => {}
        }
    }
    Ok(strings)
}

fn parse_xlsx_rels(xml: &str) -> Result<HashMap<String, String>, ExtractError> {
    let mut reader = Reader::from_str(xml);
    let mut map = HashMap::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e) | Event::Empty(ref e))
                if e.local_name().as_ref() == "Relationship" =>
            {
                let mut id = None;
                let mut target = None;
                let mut external = false;
                for attr in e.attributes().flatten() {
                    let key = attr.key.local_name();
                    let val = attr
                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                        .unwrap_or_default()
                        .into_owned();
                    match key.as_ref() {
                        "Id" => id = Some(val),
                        "Target" => target = Some(val),
                        "TargetMode" if val.eq_ignore_ascii_case("External") => external = true,
                        _ => {}
                    }
                }
                if !external
                    && let (Some(id), Some(target)) = (id, target)
                    && !target.contains("..")
                    && !target.starts_with('/')
                {
                    let normalized = if target.starts_with("xl/") {
                        target
                    } else {
                        format!("xl/{}", target.trim_start_matches("./"))
                    };
                    map.insert(id, normalized);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    Ok(map)
}

fn parse_xlsx_workbook(
    xml: &str,
    rels: &HashMap<String, String>,
) -> Vec<(String, Option<String>)> {
    let mut reader = Reader::from_str(xml);
    let mut sheets = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e) | Event::Empty(ref e))
                if e.local_name().as_ref() == "sheet" =>
            {
                let mut name = None;
                let mut rid = None;
                for attr in e.attributes().flatten() {
                    let key = attr.key.local_name();
                    let val = attr
                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                        .unwrap_or_default()
                        .into_owned();
                    match key.as_ref() {
                        "name" => name = Some(val),
                        "id" => rid = Some(val),
                        _ => {}
                    }
                }
                if let Some(name) = name {
                    let path = rid.and_then(|id| rels.get(&id).cloned());
                    sheets.push((name, path));
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    sheets
}

fn parse_xlsx_sheet(xml: &str, shared_strings: &[String]) -> Result<Vec<String>, ExtractError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut rows = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut cell_type = String::new();
    let mut cell_value = String::new();
    let mut in_cell = false;
    let mut in_value = false;
    let mut in_inline_t = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                "row" => current_row.clear(),
                "c" => {
                    in_cell = true;
                    cell_type.clear();
                    cell_value.clear();
                    for attr in e.attributes().flatten() {
                        if attr.key.local_name().as_ref() == "t" {
                            cell_type = attr
                                .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                .unwrap_or_default()
                                .into_owned();
                        }
                    }
                }
                "v" if in_cell => in_value = true,
                "t" if in_cell => in_inline_t = true,
                _ => {}
            },
            Ok(Event::Text(ref e)) if in_value || in_inline_t => {
                cell_value.push_str(e.as_ref());
            }
            Ok(Event::GeneralRef(ref e)) if in_value || in_inline_t => {
                append_general_ref(&mut cell_value, e);
            }
            Ok(Event::End(ref e)) => match e.local_name().as_ref() {
                "v" => in_value = false,
                "t" => in_inline_t = false,
                "c" if in_cell => {
                    in_cell = false;
                    let resolved = match cell_type.as_str() {
                        "s" => cell_value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|idx| shared_strings.get(idx))
                            .cloned()
                            .unwrap_or_default(),
                        "b" => match cell_value.trim() {
                            "1" => "TRUE".into(),
                            "0" => "FALSE".into(),
                            other => other.to_owned(),
                        },
                        _ => cell_value.clone(),
                    };
                    if !resolved.is_empty() {
                        current_row.push(resolved);
                    }
                }
                "row" => {
                    if !current_row.is_empty() {
                        rows.push(current_row.join("\t"));
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Other(anyhow::anyhow!(
                    "Invalid worksheet XML: {err}"
                )));
            }
            _ => {}
        }
    }
    Ok(rows)
}

// --- Pure-Rust PDF text & scanned-PDF detection ---

struct PdfStream<'a> {
    dict: &'a [u8],
    data: &'a [u8],
}

fn extract_pdf_pages(bytes: &[u8]) -> Result<Vec<String>, ExtractError> {
    let has_image = contains_subslice(bytes, b"/Subtype /Image")
        || contains_subslice(bytes, b"/Subtype/Image");

    let objects = parse_pdf_objects(bytes);
    let page_streams = collect_page_streams(&objects, bytes);

    let mut pages = Vec::new();
    for (idx, stream_list) in page_streams.into_iter().enumerate() {
        let mut page_text = String::new();
        for stream_bytes in stream_list {
            let extracted = extract_text_from_pdf_content_stream(&stream_bytes);
            if !extracted.trim().is_empty() {
                if !page_text.is_empty() {
                    page_text.push('\n');
                }
                page_text.push_str(extracted.trim());
            }
        }
        if !page_text.trim().is_empty() {
            pages.push(format!("[Page {}]\n{}", idx + 1, page_text.trim()));
        }
    }

    let meaningful_chars: usize = pages
        .iter()
        .map(|p| {
            p.strip_prefix("[Page ")
                .and_then(|rest| rest.split_once(']'))
                .map_or(p.as_str(), |(_, body)| body)
                .chars()
                .filter(|c| c.is_alphanumeric())
                .count()
        })
        .sum();

    if meaningful_chars < 16 {
        if has_image {
            return Err(ExtractError::ScannedPdfUnsupported);
        }
        return Err(ExtractError::Other(anyhow::anyhow!(
            "Could not extract readable text from this PDF"
        )));
    }
    Ok(pages)
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn parse_pdf_objects(bytes: &[u8]) -> HashMap<u32, &[u8]> {
    let mut map = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((obj_num, body_start)) = parse_obj_header(bytes, i) {
            if let Some(end_pos) = find_subslice(bytes, body_start, b"endobj") {
                map.insert(obj_num, &bytes[body_start..end_pos]);
                i = end_pos + 6;
                continue;
            }
        }
        i += 1;
    }
    map
}

fn parse_obj_header(bytes: &[u8], pos: usize) -> Option<(u32, usize)> {
    if pos > 0 && !matches!(bytes[pos - 1], b'\n' | b'\r' | b' ' | b'\t') {
        return None;
    }
    let mut i = pos;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == pos {
        return None;
    }
    let obj_num: u32 = std::str::from_utf8(&bytes[pos..i]).ok()?.parse().ok()?;
    if i >= bytes.len() || !bytes[i].is_ascii_whitespace() {
        return None;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let gen_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == gen_start || i >= bytes.len() || !bytes[i].is_ascii_whitespace() {
        return None;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes[i..].starts_with(b"obj")
        && (i + 3 == bytes.len() || bytes[i + 3].is_ascii_whitespace() || bytes[i + 3] == b'<')
    {
        Some((obj_num, i + 3))
    } else {
        None
    }
}

fn find_subslice(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if start >= haystack.len() || needle.is_empty() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|offset| start + offset)
}

fn collect_page_streams(objects: &HashMap<u32, &[u8]>, raw_pdf: &[u8]) -> Vec<Vec<Vec<u8>>> {
    let mut page_nums: Vec<u32> = objects
        .iter()
        .filter(|(_, body)| is_page_object(body))
        .map(|(num, _)| *num)
        .collect();
    page_nums.sort_unstable();

    let mut pages = Vec::new();
    for num in page_nums {
        if let Some(body) = objects.get(&num) {
            let content_refs = parse_contents_refs(body);
            let mut streams = Vec::new();
            for obj_ref in content_refs {
                if let Some(obj_body) = objects.get(&obj_ref)
                    && let Some(decoded) = extract_stream_bytes(obj_body)
                {
                    streams.push(decoded);
                }
            }
            if !streams.is_empty() {
                pages.push(streams);
            }
        }
    }

    if !pages.is_empty() {
        return pages;
    }

    // Fallback when page tree references are non-standard: decode all content streams in order.
    let mut fallback = Vec::new();
    let mut pos = 0;
    while let Some(stream_idx) = find_subslice(raw_pdf, pos, b"stream") {
        let dict_start = raw_pdf[..stream_idx]
            .iter()
            .rposition(|&b| b == b'<')
            .unwrap_or(0);
        if let Some(decoded) = extract_stream_bytes(&raw_pdf[dict_start..]) {
            if contains_subslice(&decoded, b"BT") {
                fallback.push(decoded);
            }
        }
        pos = stream_idx + 6;
    }
    if fallback.is_empty() {
        Vec::new()
    } else {
        vec![fallback]
    }
}

fn is_page_object(body: &[u8]) -> bool {
    let mut pos = 0;
    while let Some(idx) = find_subslice(body, pos, b"/Type") {
        let mut after = idx + 5;
        while after < body.len() && body[after].is_ascii_whitespace() {
            after += 1;
        }
        if body[after..].starts_with(b"/Page") {
            let next = after + 5;
            if next >= body.len() || !body[next].is_ascii_alphabetic() {
                return true;
            }
        }
        pos = idx + 5;
    }
    false
}

fn parse_contents_refs(body: &[u8]) -> Vec<u32> {
    let Some(idx) = find_subslice(body, 0, b"/Contents") else {
        return Vec::new();
    };
    let mut i = idx + 9;
    while i < body.len() && body[i].is_ascii_whitespace() {
        i += 1;
    }
    let slice = if i < body.len() && body[i] == b'[' {
        let end = body[i..]
            .iter()
            .position(|&b| b == b']')
            .map_or(body.len(), |offset| i + offset);
        &body[i + 1..end]
    } else {
        let end = find_subslice(body, i, b"/").unwrap_or(body.len());
        &body[i..end]
    };
    let text = String::from_utf8_lossy(slice);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut refs = Vec::new();
    for window in tokens.windows(3) {
        if window[2] == "R"
            && let Ok(num) = window[0].parse::<u32>()
            && window[1].parse::<u16>().is_ok()
        {
            refs.push(num);
        }
    }
    refs
}

fn extract_stream_bytes(obj_body: &[u8]) -> Option<Vec<u8>> {
    let stream_kw = find_subslice(obj_body, 0, b"stream")?;
    let dict = &obj_body[..stream_kw];
    let mut data_start = stream_kw + 6;
    if obj_body.get(data_start..data_start + 2) == Some(b"\r\n") {
        data_start += 2;
    } else if obj_body.get(data_start) == Some(&b'\n') || obj_body.get(data_start) == Some(&b'\r') {
        data_start += 1;
    } else {
        return None;
    }
    let endstream = find_subslice(obj_body, data_start, b"endstream")?;
    let mut data = &obj_body[data_start..endstream];
    while data.last().is_some_and(|b| matches!(b, b'\r' | b'\n')) {
        data = &data[..data.len() - 1];
    }

    let parsed = PdfStream { dict, data };
    if contains_subslice(parsed.dict, b"/FlateDecode") {
        let mut out = Vec::new();
        let mut decoder = ZlibDecoder::new(parsed.data).take((MAX_PDF_STREAM_BYTES + 1) as u64);
        if decoder.read_to_end(&mut out).is_ok() && !out.is_empty() {
            if out.len() <= MAX_PDF_STREAM_BYTES {
                return Some(out);
            }
            return None;
        }
        out.clear();
        let mut raw_decoder =
            DeflateDecoder::new(parsed.data).take((MAX_PDF_STREAM_BYTES + 1) as u64);
        if raw_decoder.read_to_end(&mut out).is_ok() && out.len() <= MAX_PDF_STREAM_BYTES {
            return Some(out);
        }
        None
    } else if parsed.data.len() <= MAX_PDF_STREAM_BYTES {
        Some(parsed.data.to_vec())
    } else {
        None
    }
}

fn extract_text_from_pdf_content_stream(stream: &[u8]) -> String {
    let mut out = String::new();
    let mut in_text = false;
    let mut pending_strings: Vec<String> = Vec::new();
    let mut in_array = false;
    let mut i = 0;

    while i < stream.len() {
        let b = stream[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'%' {
            while i < stream.len() && stream[i] != b'\n' && stream[i] != b'\r' {
                i += 1;
            }
            continue;
        }
        if b == b'(' && in_text {
            let (decoded, next) = parse_pdf_literal_string(stream, i);
            pending_strings.push(decoded);
            i = next;
            continue;
        }
        if b == b'<' && stream.get(i + 1) != Some(&b'<') && in_text {
            let (decoded, next) = parse_pdf_hex_string(stream, i);
            pending_strings.push(decoded);
            i = next;
            continue;
        }
        if b == b'[' && in_text {
            in_array = true;
            pending_strings.clear();
            i += 1;
            continue;
        }
        if b == b']' && in_text {
            in_array = false;
            i += 1;
            continue;
        }
        if matches!(b, b'<' | b'>' | b'/' | b'{' | b'}') {
            i += 1;
            continue;
        }

        let start = i;
        while i < stream.len()
            && !stream[i].is_ascii_whitespace()
            && !matches!(
                stream[i],
                b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
            )
        {
            i += 1;
        }
        let token = &stream[start..i];
        match token {
            b"BT" => {
                in_text = true;
                pending_strings.clear();
            }
            b"ET" => {
                in_text = false;
                pending_strings.clear();
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            b"Tj" | b"TJ" if in_text => {
                for piece in pending_strings.drain(..) {
                    out.push_str(&piece);
                }
            }
            b"'" | b"\"" if in_text => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                for piece in pending_strings.drain(..) {
                    out.push_str(&piece);
                }
            }
            b"Td" | b"TD" | b"T*" if in_text => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                pending_strings.clear();
            }
            _ => {
                if in_text && in_array {
                    if let Ok(num) = std::str::from_utf8(token)
                        && let Ok(kern) = num.parse::<f64>()
                        && kern < -150.0
                        && !out.ends_with([' ', '\n'])
                    {
                        pending_strings.push(" ".into());
                    }
                } else if in_text
                    && !matches!(
                        token,
                        b"Tf" | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tr" | b"Ts" | b"Tm"
                    )
                {
                    // Keep numeric arguments discarded before non-text operators.
                }
            }
        }
    }
    out
}

fn parse_pdf_literal_string(bytes: &[u8], start: usize) -> (String, usize) {
    let mut i = start + 1;
    let mut depth = 1usize;
    let mut raw = Vec::new();
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'(' => {
                depth += 1;
                raw.push(b'(');
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth > 0 {
                    raw.push(b')');
                }
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                match bytes[i] {
                    b'n' => raw.push(b'\n'),
                    b'r' => raw.push(b'\r'),
                    b't' => raw.push(b'\t'),
                    b'b' => raw.push(0x08),
                    b'f' => raw.push(0x0c),
                    b'\r' => {
                        if bytes.get(i + 1) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    b'\n' => {}
                    c @ b'0'..=b'7' => {
                        let mut oct = c - b'0';
                        for _ in 0..2 {
                            if let Some(&next) = bytes.get(i + 1)
                                && (b'0'..=b'7').contains(&next)
                            {
                                oct = oct.wrapping_mul(8).wrapping_add(next - b'0');
                                i += 1;
                            } else {
                                break;
                            }
                        }
                        raw.push(oct);
                    }
                    other => raw.push(other),
                }
                i += 1;
            }
            other => {
                raw.push(other);
                i += 1;
            }
        }
    }
    (decode_pdf_bytes(&raw), i)
}

fn parse_pdf_hex_string(bytes: &[u8], start: usize) -> (String, usize) {
    let mut i = start + 1;
    let mut hex = Vec::new();
    while i < bytes.len() && bytes[i] != b'>' {
        if !bytes[i].is_ascii_whitespace() {
            hex.push(bytes[i]);
        }
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'>' {
        i += 1;
    }
    if hex.len() % 2 == 1 {
        hex.push(b'0');
    }
    let mut raw = Vec::with_capacity(hex.len() / 2);
    for pair in hex.chunks_exact(2) {
        if let Ok(s) = std::str::from_utf8(pair)
            && let Ok(byte) = u8::from_str_radix(s, 16)
        {
            raw.push(byte);
        }
    }
    (decode_pdf_bytes(&raw), i)
}

fn decode_pdf_bytes(raw: &[u8]) -> String {
    if raw.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) => raw.iter().map(|&b| b as char).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;
    use uuid::Uuid;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn fixture_attachment(name: &str, size: u64) -> Attachment {
        Attachment {
            id: Uuid::new_v4(),
            name: name.into(),
            kind: AttachmentKind::File,
            size,
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn rejects_unsupported_formats_and_magic_mismatches() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["archive.zip", "tool.exe", "native.dll", "unknown.bin"] {
            let path = dir.path().join(bad);
            std::fs::write(&path, b"MZ\x90\x00").unwrap();
            let err = extract_attachment(&path, &fixture_attachment(bad, 4), AiProvider::OpenAi)
                .unwrap_err();
            assert!(
                matches!(err, ExtractError::UnsupportedFileFormat(_)),
                "expected UnsupportedFileFormat for {bad}, got {err:?}"
            );
        }

        // Disguised EXE renamed as .png must be rejected by magic bytes check.
        let fake_png = dir.path().join("fake.png");
        std::fs::write(&fake_png, b"MZ\x90\x00not-a-png").unwrap();
        let err = extract_attachment(
            &fake_png,
            &fixture_attachment("fake.png", 13),
            AiProvider::OpenAi,
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::UnsupportedFileFormat(_)));
    }

    #[test]
    fn extracts_images_docx_xlsx_and_text_formats() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // PNG
        let png_path = dir.path().join("pic.png");
        let png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        std::fs::write(&png_path, png_bytes)?;
        let extracted = extract_attachment(
            &png_path,
            &fixture_attachment("pic.png", png_bytes.len() as u64),
            AiProvider::DeepSeek,
        )?;
        match extracted {
            ExtractedAttachment::Image { image_url, .. } => {
                assert!(image_url.starts_with("data:image/png;base64,"));
            }
            other => panic!("expected Image, got {other:?}"),
        }

        // DOCX
        let docx_path = dir.path().join("report.docx");
        {
            let file = File::create(&docx_path)?;
            let mut zip = ZipWriter::new(file);
            zip.start_file("word/document.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
                <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
                  <w:body>
                    <w:p><w:r><w:t>Quarterly &amp; Summary</w:t></w:r></w:p>
                    <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Col1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Col2</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
                  </w:body>
                </w:document>"#,
            )?;
            zip.finish()?;
        }
        let extracted_docx = extract_attachment(
            &docx_path,
            &fixture_attachment("report.docx", std::fs::metadata(&docx_path)?.len()),
            AiProvider::OpenAi,
        )?;
        match extracted_docx {
            ExtractedAttachment::TextChunks { chunks, .. } => {
                let text = chunks.join("\n");
                assert!(text.contains("Quarterly & Summary"));
                assert!(text.contains("Col1 | Col2"));
            }
            other => panic!("expected TextChunks, got {other:?}"),
        }

        // XLSX
        let xlsx_path = dir.path().join("budget.xlsx");
        {
            let file = File::create(&xlsx_path)?;
            let mut zip = ZipWriter::new(file);
            zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
                    <sheets><sheet name="Revenue" sheetId="1" r:id="rId1"/></sheets>
                </workbook>"#,
            )?;
            zip.start_file("xl/_rels/workbook.xml.rels", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<Relationships>
                    <Relationship Id="rId1" Target="worksheets/sheet1.xml"/>
                </Relationships>"#,
            )?;
            zip.start_file("xl/sharedStrings.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<sst><si><t>Item</t></si><si><t>Total</t></si></sst>"#)?;
            zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<worksheet><sheetData>
                    <row><c t="s"><v>0</v></c><c t="s"><v>1</v></c></row>
                    <row><c><v>420</v></c><c t="inlineStr"><is><t>USD</t></is></c></row>
                </sheetData></worksheet>"#,
            )?;
            zip.finish()?;
        }
        let extracted_xlsx = extract_attachment(
            &xlsx_path,
            &fixture_attachment("budget.xlsx", std::fs::metadata(&xlsx_path)?.len()),
            AiProvider::Grok,
        )?;
        match extracted_xlsx {
            ExtractedAttachment::TextChunks { chunks, .. } => {
                let text = chunks.join("\n");
                assert!(text.contains("[Sheet: Revenue]"));
                assert!(text.contains("Item\tTotal"));
                assert!(text.contains("420\tUSD"));
            }
            other => panic!("expected TextChunks, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn extracts_flate_pdf_and_rejects_scanned_pdf() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // Valid FlateDecode PDF
        let stream_content = b"BT /F1 12 Tf 72 720 Td (Hello Zibblu PDF World 2026) Tj ET";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(stream_content)?;
        let compressed = encoder.finish()?;
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
        pdf.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
        pdf.extend_from_slice(b"3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n");
        pdf.extend_from_slice(
            format!(
                "4 0 obj << /Filter /FlateDecode /Length {} >>\nstream\n",
                compressed.len()
            )
            .as_bytes(),
        );
        pdf.extend_from_slice(&compressed);
        pdf.extend_from_slice(b"\nendstream\nendobj\n%%EOF");

        let pdf_path = dir.path().join("notes.pdf");
        std::fs::write(&pdf_path, &pdf)?;
        // Non-OpenAI uses pure-Rust text extraction.
        let extracted = extract_attachment(
            &pdf_path,
            &fixture_attachment("notes.pdf", pdf.len() as u64),
            AiProvider::DeepSeek,
        )?;
        match extracted {
            ExtractedAttachment::TextChunks { chunks, .. } => {
                assert!(chunks.join("\n").contains("Hello Zibblu PDF World 2026"));
            }
            other => panic!("expected TextChunks, got {other:?}"),
        }
        // OpenAI within 10 MiB uses native input_file.
        let extracted_openai = extract_attachment(
            &pdf_path,
            &fixture_attachment("notes.pdf", pdf.len() as u64),
            AiProvider::OpenAi,
        )?;
        assert!(matches!(extracted_openai, ExtractedAttachment::NativePdf { .. }));

        // Scanned image-only PDF must fail explicitly with ScannedPdfUnsupported.
        let scanned_pdf = b"%PDF-1.4\n1 0 obj << /Type /Page /Contents 2 0 R >> endobj\n2 0 obj << /Length 12 >> stream\nBT /Im1 Do ET\nendstream\nendobj\n3 0 obj << /Type /XObject /Subtype /Image /Width 100 /Height 100 >> endobj\n%%EOF";
        let scanned_path = dir.path().join("scan.pdf");
        std::fs::write(&scanned_path, scanned_pdf)?;
        let err = extract_attachment(
            &scanned_path,
            &fixture_attachment("scan.pdf", scanned_pdf.len() as u64),
            AiProvider::DeepSeek,
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::ScannedPdfUnsupported));
        Ok(())
    }
}
