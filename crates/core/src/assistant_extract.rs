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
    #[error("当前模型提供商不支持直接读取图片，请换用支持视觉能力的模型")]
    VisionUnsupported,
    #[error("当前模型或接口不支持直接读取扫描版 PDF，请换用支持原生 PDF/视觉的模型")]
    ScannedPdfUnsupported,
    #[error("附件 '{name}' 包含本地提取无法完整读取的内容（{}），已终止发送，请换用支持完整读取该文件的模型或转为纯文本", reasons.join("；"))]
    IncompleteExtraction { name: String, reasons: Vec<String> },
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

pub fn check_supported_attachment_for_provider(
    attachment: &Attachment,
    provider: AiProvider,
) -> Result<(), ExtractError> {
    check_supported_attachment(attachment)?;
    let kind = classify_attachment(attachment)?;
    if matches!(
        kind,
        SupportedAttachmentKind::ImageJpeg
            | SupportedAttachmentKind::ImagePng
            | SupportedAttachmentKind::ImageWebp
    ) && !provider.supports_image_input()
    {
        return Err(ExtractError::VisionUnsupported);
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

#[derive(Clone, Debug)]
pub struct ChunkedAiRequests {
    pub requests: Vec<Vec<AiInputPart>>,
}

pub fn extract_attachment(
    path: &Path,
    attachment: &Attachment,
    provider: AiProvider,
) -> Result<ExtractedAttachment, ExtractError> {
    let kind = classify_attachment(attachment)?;
    check_supported_attachment_for_provider(attachment, provider)?;

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
            let docx = extract_docx_text(file, &attachment.name)?;
            let chunks = split_text_chunks(&docx.text, MAX_CHUNK_TEXT_BYTES)?;
            Ok(ExtractedAttachment::TextChunks {
                name: attachment.name.clone(),
                chunks,
            })
        }
        SupportedAttachmentKind::Xlsx => {
            let xlsx = extract_xlsx_sheets(file, &attachment.name)?;
            let chunks = pack_sections_into_chunks(&xlsx.sections, MAX_CHUNK_TEXT_BYTES)?;
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
            if provider.supports_native_pdf_input() && bytes.len() as u64 <= MAX_AI_NATIVE_PDF_BYTES {
                return Ok(ExtractedAttachment::NativePdf {
                    name: attachment.name.clone(),
                    file_data: format!("data:application/pdf;base64,{}", STANDARD.encode(&bytes)),
                });
            }
            let pdf = extract_pdf_pages(&bytes, &attachment.name)?;
            let chunks = pack_sections_into_chunks(&pdf.pages, MAX_CHUNK_TEXT_BYTES)?;
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
) -> Result<ChunkedAiRequests> {
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
        return Ok(ChunkedAiRequests {
            requests: vec![parts],
        });
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
        return Ok(ChunkedAiRequests {
            requests: vec![parts],
        });
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
    Ok(ChunkedAiRequests { requests })
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

struct DocxExtraction {
    text: String,
}

struct XlsxExtraction {
    sections: Vec<String>,
}

struct PdfExtraction {
    pages: Vec<String>,
}

fn collect_zip_visual_entries(archive: &ZipArchive<BufReader<File>>, prefix: &str) -> Vec<String> {
    let mut uncovered = Vec::new();
    for entry in archive.file_names() {
        if let Some(rest) = entry.strip_prefix(prefix) {
            if rest.starts_with("media/") {
                uncovered.push(format!("嵌入图片/媒体 ({entry})"));
            } else if rest.starts_with("charts/") {
                uncovered.push(format!("嵌入图表 ({entry})"));
            } else if rest.starts_with("diagrams/") {
                uncovered.push(format!("嵌入图示 ({entry})"));
            } else if rest.starts_with("embeddings/") {
                uncovered.push(format!("嵌入对象 ({entry})"));
            } else if rest.starts_with("drawings/") {
                uncovered.push(format!("绘图内容 ({entry})"));
            }
        }
    }
    uncovered.sort();
    uncovered
}

fn collect_docx_uncovered_entries(archive: &ZipArchive<BufReader<File>>) -> Vec<String> {
    let mut uncovered = collect_zip_visual_entries(archive, "word/");
    for entry in archive.file_names() {
        if entry.ends_with('/') {
            continue;
        }
        let Some(rest) = entry.strip_prefix("word/") else {
            continue;
        };
        if rest.starts_with("media/")
            || rest.starts_with("charts/")
            || rest.starts_with("diagrams/")
            || rest.starts_with("embeddings/")
            || rest.starts_with("drawings/")
            || rest.starts_with("_rels/")
            || rest.starts_with("theme/")
            || matches!(
                rest,
                "document.xml"
                    | "styles.xml"
                    | "stylesWithEffects.xml"
                    | "settings.xml"
                    | "webSettings.xml"
                    | "fontTable.xml"
                    | "numbering.xml"
                    | "people.xml"
            )
        {
            continue;
        }
        let reason = if rest.starts_with("header") {
            format!("未解析的页眉 ({entry})")
        } else if rest.starts_with("footer") {
            format!("未解析的页脚 ({entry})")
        } else if rest.starts_with("footnotes") {
            format!("未解析的脚注 ({entry})")
        } else if rest.starts_with("endnotes") {
            format!("未解析的尾注 ({entry})")
        } else if rest.starts_with("comments") {
            format!("未解析的批注 ({entry})")
        } else {
            format!("未解析的文档部件 ({entry})")
        };
        uncovered.push(reason);
    }
    uncovered.sort();
    uncovered.dedup();
    uncovered
}

fn collect_xlsx_uncovered_entries(archive: &ZipArchive<BufReader<File>>) -> Vec<String> {
    let mut uncovered = collect_zip_visual_entries(archive, "xl/");
    for entry in archive.file_names() {
        if entry.ends_with('/') {
            continue;
        }
        let Some(rest) = entry.strip_prefix("xl/") else {
            continue;
        };
        if rest.starts_with("comments")
            || rest.starts_with("threadedComments/")
            || rest.starts_with("pivotTables/")
            || rest.starts_with("chartsheets/")
        {
            uncovered.push(format!("未解析的工作簿附加内容 ({entry})"));
        }
    }
    uncovered.sort();
    uncovered.dedup();
    uncovered
}

fn extract_docx_text(file: File, doc_name: &str) -> Result<DocxExtraction, ExtractError> {
    let mut archive = ZipArchive::new(BufReader::new(file))
        .map_err(|_| ExtractError::UnsupportedFileFormat(doc_name.to_owned()))?;
    let mut uncovered = collect_docx_uncovered_entries(&archive);
    let xml = read_zip_entry_utf8(&mut archive, "word/document.xml", doc_name)?;
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(false);

    let mut out = String::new();
    let mut in_text = false;
    let mut in_cell = false;
    let mut cell_has_para = false;
    let mut row_has_cell = false;
    let mut has_inline_unsupported = false;

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
                "drawing" | "pict" | "blip" | "imagedata" | "chart" | "object" | "altChunk" => {
                    has_inline_unsupported = true;
                }
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match e.local_name().as_ref() {
                "tab" => out.push('\t'),
                "br" | "cr" => out.push('\n'),
                "drawing" | "pict" | "blip" | "imagedata" | "chart" | "object" | "altChunk" => {
                    has_inline_unsupported = true;
                }
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
    if has_inline_unsupported && uncovered.is_empty() {
        uncovered.push("文档正文包含未提取的图像、图表或嵌入对象".into());
    }
    if !uncovered.is_empty() {
        return Err(ExtractError::IncompleteExtraction {
            name: doc_name.to_owned(),
            reasons: uncovered,
        });
    }
    let text = out.trim().to_owned();
    Ok(DocxExtraction { text })
}

fn extract_xlsx_sheets(file: File, doc_name: &str) -> Result<XlsxExtraction, ExtractError> {
    let mut archive = ZipArchive::new(BufReader::new(file))
        .map_err(|_| ExtractError::UnsupportedFileFormat(doc_name.to_owned()))?;
    let uncovered = collect_xlsx_uncovered_entries(&archive);
    if !uncovered.is_empty() {
        return Err(ExtractError::IncompleteExtraction {
            name: doc_name.to_owned(),
            reasons: uncovered,
        });
    }

    let shared_strings = if archive.by_name("xl/sharedStrings.xml").is_ok() {
        let xml = read_zip_entry_utf8(&mut archive, "xl/sharedStrings.xml", doc_name)?;
        parse_xlsx_shared_strings(&xml)?
    } else {
        Vec::new()
    };

    if archive.by_name("xl/_rels/workbook.xml.rels").is_err() {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "Missing xl/_rels/workbook.xml.rels in '{doc_name}'"
        )));
    }
    let rels_xml = read_zip_entry_utf8(&mut archive, "xl/_rels/workbook.xml.rels", doc_name)?;
    let rels = parse_xlsx_rels(&rels_xml)?;

    let workbook_xml = read_zip_entry_utf8(&mut archive, "xl/workbook.xml", doc_name)?;
    let sheets = parse_xlsx_workbook(&workbook_xml, &rels, doc_name)?;
    if sheets.is_empty() {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "No worksheets declared in '{doc_name}'"
        )));
    }

    let mut sections = Vec::new();
    for (sheet_name, entry_path) in sheets {
        if archive.by_name(&entry_path).is_err() {
            return Err(ExtractError::Other(anyhow::anyhow!(
                "Worksheet '{sheet_name}' ({entry_path}) declared in workbook.xml is missing from '{doc_name}'"
            )));
        }
        let sheet_xml = read_zip_entry_utf8(&mut archive, &entry_path, doc_name)?;
        let rows = parse_xlsx_sheet(&sheet_xml, &shared_strings)?;
        if rows.is_empty() {
            sections.push(format!("[Sheet: {sheet_name}]\n(empty)"));
        } else {
            sections.push(format!("[Sheet: {sheet_name}]\n{}", rows.join("\n")));
        }
    }

    Ok(XlsxExtraction { sections })
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
            Ok(Event::Empty(ref e)) if e.local_name().as_ref() == "si" => {
                strings.push(String::new());
            }
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
                for attr in e.attributes() {
                    let attr = attr.map_err(|err| {
                        ExtractError::Other(anyhow::anyhow!(
                            "Invalid attribute in workbook.xml.rels: {err}"
                        ))
                    })?;
                    let key = attr.key.local_name();
                    let val = attr
                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                        .map_err(|err| {
                            ExtractError::Other(anyhow::anyhow!(
                                "Invalid attribute value in workbook.xml.rels: {err}"
                            ))
                        })?
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
                    && !target.contains('\\')
                {
                    let normalized = if let Some(abs) = target.strip_prefix('/') {
                        if !abs.starts_with("xl/") {
                            return Err(ExtractError::Other(anyhow::anyhow!(
                                "Unsupported workbook relationship target '{target}'"
                            )));
                        }
                        abs.to_owned()
                    } else if target.starts_with("xl/") {
                        target
                    } else {
                        format!("xl/{}", target.trim_start_matches("./"))
                    };
                    map.insert(id, normalized);
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Other(anyhow::anyhow!(
                    "Invalid workbook.xml.rels: {err}"
                )));
            }
            _ => {}
        }
    }
    Ok(map)
}

fn parse_xlsx_workbook(
    xml: &str,
    rels: &HashMap<String, String>,
    doc_name: &str,
) -> Result<Vec<(String, String)>, ExtractError> {
    let mut reader = Reader::from_str(xml);
    let mut sheets = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e) | Event::Empty(ref e))
                if e.local_name().as_ref() == "sheet" =>
            {
                let mut name = None;
                let mut rid = None;
                for attr in e.attributes() {
                    let attr = attr.map_err(|err| {
                        ExtractError::Other(anyhow::anyhow!(
                            "Invalid attribute in workbook.xml of '{doc_name}': {err}"
                        ))
                    })?;
                    let key = attr.key.local_name();
                    let val = attr
                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                        .map_err(|err| {
                            ExtractError::Other(anyhow::anyhow!(
                                "Invalid attribute value in workbook.xml of '{doc_name}': {err}"
                            ))
                        })?
                        .into_owned();
                    match key.as_ref() {
                        "name" => name = Some(val),
                        "id" => rid = Some(val),
                        _ => {}
                    }
                }
                let sheet_name = name.ok_or_else(|| {
                    ExtractError::Other(anyhow::anyhow!(
                        "Worksheet element in '{doc_name}' is missing its name"
                    ))
                })?;
                let rel_id = rid.ok_or_else(|| {
                    ExtractError::Other(anyhow::anyhow!(
                        "Worksheet '{sheet_name}' in '{doc_name}' is missing relationship id"
                    ))
                })?;
                let path = rels.get(&rel_id).cloned().ok_or_else(|| {
                    ExtractError::Other(anyhow::anyhow!(
                        "Worksheet '{sheet_name}' ({rel_id}) in '{doc_name}' has no valid relationship in workbook.xml.rels"
                    ))
                })?;
                sheets.push((sheet_name, path));
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Other(anyhow::anyhow!(
                    "Invalid workbook.xml in '{doc_name}': {err}"
                )));
            }
            _ => {}
        }
    }
    Ok(sheets)
}

fn parse_xlsx_cell_col(cell_ref: &str) -> Result<usize, ExtractError> {
    let trimmed = cell_ref.trim();
    let split_idx = trimmed
        .bytes()
        .position(|b| !b.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let (col_part, row_part) = trimmed.split_at(split_idx);
    if col_part.is_empty()
        || row_part.is_empty()
        || !row_part.bytes().all(|b| b.is_ascii_digit())
        || row_part.parse::<usize>().ok().is_none_or(|r| r == 0)
    {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "Invalid XLSX cell reference '{cell_ref}'"
        )));
    }
    let mut col_num = 0usize;
    for b in col_part.bytes() {
        let upper = b.to_ascii_uppercase();
        if !upper.is_ascii_uppercase() {
            return Err(ExtractError::Other(anyhow::anyhow!(
                "Invalid XLSX cell reference '{cell_ref}'"
            )));
        }
        col_num = col_num
            .checked_mul(26)
            .and_then(|n| n.checked_add((upper - b'A') as usize + 1))
            .ok_or_else(|| {
                ExtractError::Other(anyhow::anyhow!("XLSX column '{cell_ref}' is out of range"))
            })?;
    }
    let zero_based = col_num - 1;
    if zero_based > 16_383 {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "XLSX column '{cell_ref}' exceeds maximum worksheet columns"
        )));
    }
    Ok(zero_based)
}

fn parse_xlsx_sheet(xml: &str, shared_strings: &[String]) -> Result<Vec<String>, ExtractError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut rows = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut next_col = 0usize;
    let mut cell_col = 0usize;
    let mut cell_type = String::new();
    let mut cell_value = String::new();
    let mut in_cell = false;
    let mut in_value = false;
    let mut in_inline_t = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                "row" => {
                    current_row.clear();
                    next_col = 0;
                }
                "c" => {
                    in_cell = true;
                    cell_type.clear();
                    cell_value.clear();
                    let mut cell_ref = None;
                    for attr in e.attributes() {
                        let attr = attr.map_err(|err| {
                            ExtractError::Other(anyhow::anyhow!(
                                "Invalid attribute in worksheet XML: {err}"
                            ))
                        })?;
                        let key = attr.key.local_name();
                        let val = attr
                            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                            .map_err(|err| {
                                ExtractError::Other(anyhow::anyhow!(
                                    "Invalid attribute value in worksheet XML: {err}"
                                ))
                            })?
                            .into_owned();
                        match key.as_ref() {
                            "t" => cell_type = val,
                            "r" => cell_ref = Some(val),
                            _ => {}
                        }
                    }
                    cell_col = match cell_ref {
                        Some(r) => {
                            let col = parse_xlsx_cell_col(&r)?;
                            next_col = col + 1;
                            col
                        }
                        None => {
                            let col = next_col;
                            next_col += 1;
                            col
                        }
                    };
                }
                "v" if in_cell => in_value = true,
                "t" if in_cell => in_inline_t = true,
                _ => {}
            },
            Ok(Event::Empty(ref e)) if e.local_name().as_ref() == "c" => {
                let mut empty_type = String::new();
                let mut cell_ref = None;
                for attr in e.attributes() {
                    let attr = attr.map_err(|err| {
                        ExtractError::Other(anyhow::anyhow!(
                            "Invalid attribute in worksheet XML: {err}"
                        ))
                    })?;
                    let key = attr.key.local_name();
                    let val = attr
                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                        .map_err(|err| {
                            ExtractError::Other(anyhow::anyhow!(
                                "Invalid attribute value in worksheet XML: {err}"
                            ))
                        })?
                        .into_owned();
                    match key.as_ref() {
                        "t" => empty_type = val,
                        "r" => cell_ref = Some(val),
                        _ => {}
                    }
                }
                if let Some(r) = cell_ref {
                    let col = parse_xlsx_cell_col(&r)?;
                    next_col = col + 1;
                } else {
                    next_col += 1;
                }
                if empty_type == "s" {
                    return Err(ExtractError::Other(anyhow::anyhow!(
                        "XLSX shared string cell is missing string index"
                    )));
                }
            }
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
                        "s" => {
                            let idx = cell_value.trim().parse::<usize>().map_err(|_| {
                                ExtractError::Other(anyhow::anyhow!(
                                    "Invalid XLSX shared string index '{}'",
                                    cell_value.trim()
                                ))
                            })?;
                            shared_strings.get(idx).cloned().ok_or_else(|| {
                                ExtractError::Other(anyhow::anyhow!(
                                    "XLSX shared string index {idx} is out of bounds (len {})",
                                    shared_strings.len()
                                ))
                            })?
                        }
                        "b" => match cell_value.trim() {
                            "1" => "TRUE".into(),
                            "0" => "FALSE".into(),
                            other => other.to_owned(),
                        },
                        _ => cell_value.clone(),
                    };
                    if !resolved.is_empty() {
                        if current_row.len() <= cell_col {
                            current_row.resize(cell_col + 1, String::new());
                        }
                        current_row[cell_col] = resolved;
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

fn extract_pdf_pages(bytes: &[u8], doc_name: &str) -> Result<PdfExtraction, ExtractError> {
    if contains_pdf_name(bytes, b"/Encrypt") {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "Encrypted PDFs are not supported for local text extraction"
        )));
    }

    let pdf_has_any_image = contains_pdf_subtype(bytes, b"/Image");
    let objects = parse_pdf_objects(bytes);
    let page_entries = collect_ordered_pages(&objects, bytes)?;

    let mut pages = Vec::with_capacity(page_entries.len());
    let mut uncovered = Vec::new();
    for (idx, entry) in page_entries.into_iter().enumerate() {
        let page_num = idx + 1;
        let page_body = objects
            .get(&entry.obj_num)
            .copied()
            .ok_or_else(|| {
                ExtractError::Other(anyhow::anyhow!("PDF page {page_num} object is missing"))
            })?;
        let content_refs = parse_contents_refs(page_body);
        let mut page_text = String::new();
        let mut stream_has_inline_image = false;
        let mut stream_has_do = false;

        for obj_ref in content_refs {
            let obj_body = objects.get(&obj_ref).copied().ok_or_else(|| {
                ExtractError::Other(anyhow::anyhow!(
                    "PDF content stream {obj_ref} for page {page_num} is missing"
                ))
            })?;
            let decoded = extract_stream_bytes(obj_body, page_num)?;
            let (has_inline, has_do) = scan_content_stream_xobjects(&decoded);
            if has_inline {
                stream_has_inline_image = true;
            }
            if has_do {
                stream_has_do = true;
            }
            let extracted = extract_text_from_pdf_content_stream(&decoded);
            if !extracted.trim().is_empty() {
                if !page_text.is_empty() {
                    page_text.push('\n');
                }
                page_text.push_str(extracted.trim());
            }
        }

        let (xobj_has_image, xobj_reasons) =
            inspect_page_xobjects(page_body, &entry.inherited_resources, &objects, page_num);
        let page_has_image = stream_has_inline_image || xobj_has_image;
        let meaningful_chars = page_text
            .chars()
            .filter(|c| c.is_alphanumeric())
            .count();

        if meaningful_chars < 16 {
            if page_has_image || pdf_has_any_image {
                return Err(ExtractError::ScannedPdfUnsupported);
            }
            return Err(ExtractError::Other(anyhow::anyhow!(
                "PDF 第 {page_num} 页无可提取文本，本地纯文本提取无法确认完整内容"
            )));
        }

        if stream_has_inline_image {
            let reason = format!("第 {page_num} 页包含未提取的图像内容");
            if !uncovered.contains(&reason) {
                uncovered.push(reason);
            }
        }
        for reason in &xobj_reasons {
            if !uncovered.contains(reason) {
                uncovered.push(reason.clone());
            }
        }
        if stream_has_do && xobj_reasons.is_empty() {
            let reason = format!("第 {page_num} 页包含未提取的 XObject 绘制操作 (Do)");
            if !uncovered.contains(&reason) {
                uncovered.push(reason);
            }
        }
        pages.push(format!("[Page {page_num}]\n{}", page_text.trim()));
    }

    if !uncovered.is_empty() {
        return Err(ExtractError::IncompleteExtraction {
            name: doc_name.to_owned(),
            reasons: uncovered,
        });
    }

    Ok(PdfExtraction { pages })
}

fn contains_pdf_name(haystack: &[u8], name: &[u8]) -> bool {
    let mut pos = 0;
    while let Some(idx) = find_subslice(haystack, pos, name) {
        let next = idx + name.len();
        if next >= haystack.len() || !haystack[next].is_ascii_alphanumeric() {
            return true;
        }
        pos = idx + name.len();
    }
    false
}

fn contains_pdf_subtype(body: &[u8], subtype: &[u8]) -> bool {
    let mut pos = 0;
    while let Some(idx) = find_subslice(body, pos, b"/Subtype") {
        let mut after = idx + 8;
        while after < body.len() && body[after].is_ascii_whitespace() {
            after += 1;
        }
        if body[after..].starts_with(subtype) {
            let next = after + subtype.len();
            if next >= body.len() || !body[next].is_ascii_alphabetic() {
                return true;
            }
        }
        pos = idx + 8;
    }
    false
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

struct PdfPageEntry {
    obj_num: u32,
    inherited_resources: Vec<u32>,
}

fn collect_ordered_pages(
    objects: &HashMap<u32, &[u8]>,
    raw_pdf: &[u8],
) -> Result<Vec<PdfPageEntry>, ExtractError> {
    let root_pages = find_root_pages_obj(objects, raw_pdf);
    if let Some(pages_obj) = root_pages {
        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        walk_pages_node(pages_obj, objects, &[], &mut visited, 0, &mut out)?;
        if let Some(body) = objects.get(&pages_obj)
            && let Some(expected_count) = parse_integer_entry(body, b"/Count")
            && out.len() != expected_count
        {
            return Err(ExtractError::Other(anyhow::anyhow!(
                "PDF page tree count mismatch (expected {expected_count}, found {})",
                out.len()
            )));
        }
        if out.is_empty() {
            return Err(ExtractError::Other(anyhow::anyhow!(
                "PDF contains no pages in its page tree"
            )));
        }
        return Ok(out);
    }

    let mut page_nums: Vec<u32> = objects
        .iter()
        .filter(|(_, body)| is_page_object(body))
        .map(|(num, _)| *num)
        .collect();
    page_nums.sort_unstable();
    if page_nums.len() == 1 {
        return Ok(vec![PdfPageEntry {
            obj_num: page_nums[0],
            inherited_resources: Vec::new(),
        }]);
    }
    Err(ExtractError::Other(anyhow::anyhow!(
        "PDF lacks a valid page tree; cannot verify page order and completeness"
    )))
}

fn find_root_pages_obj(objects: &HashMap<u32, &[u8]>, raw_pdf: &[u8]) -> Option<u32> {
    if let Some(root_ref) = parse_single_ref(raw_pdf, b"/Root")
        && let Some(catalog) = objects.get(&root_ref)
        && let Some(pages_ref) = parse_single_ref(catalog, b"/Pages")
    {
        return Some(pages_ref);
    }
    for &body in objects.values() {
        if is_catalog_object(body)
            && let Some(pages_ref) = parse_single_ref(body, b"/Pages")
        {
            return Some(pages_ref);
        }
    }
    let mut candidates: Vec<u32> = objects
        .iter()
        .filter(|(_, body)| is_pages_node(body) && parse_single_ref(body, b"/Parent").is_none())
        .map(|(&num, _)| num)
        .collect();
    candidates.sort_unstable();
    candidates.into_iter().next()
}

fn walk_pages_node(
    node_num: u32,
    objects: &HashMap<u32, &[u8]>,
    parent_resources: &[u32],
    visited: &mut std::collections::HashSet<u32>,
    depth: usize,
    out: &mut Vec<PdfPageEntry>,
) -> Result<(), ExtractError> {
    if depth > 64 || !visited.insert(node_num) {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "PDF page tree contains a cycle or excessive depth"
        )));
    }
    let body = objects.get(&node_num).copied().ok_or_else(|| {
        ExtractError::Other(anyhow::anyhow!("PDF page tree object {node_num} is missing"))
    })?;
    if is_page_object(body) {
        out.push(PdfPageEntry {
            obj_num: node_num,
            inherited_resources: parent_resources.to_vec(),
        });
        return Ok(());
    }
    if !is_pages_node(body) {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "PDF object {node_num} in page tree is neither /Pages nor /Page"
        )));
    }
    let mut resources = parent_resources.to_vec();
    if let Some(res_ref) = parse_single_ref(body, b"/Resources") {
        resources.push(res_ref);
    } else if contains_pdf_name(body, b"/Resources") {
        resources.push(node_num);
    }
    let kids = parse_array_refs(body, b"/Kids");
    if kids.is_empty() {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "PDF /Pages object {node_num} has no /Kids"
        )));
    }
    for kid in kids {
        walk_pages_node(kid, objects, &resources, visited, depth + 1, out)?;
    }
    Ok(())
}

fn is_catalog_object(body: &[u8]) -> bool {
    has_pdf_type(body, b"/Catalog")
}

fn is_pages_node(body: &[u8]) -> bool {
    has_pdf_type(body, b"/Pages")
}

fn is_page_object(body: &[u8]) -> bool {
    has_pdf_type(body, b"/Page")
}

fn has_pdf_type(body: &[u8], expected: &[u8]) -> bool {
    let mut pos = 0;
    while let Some(idx) = find_subslice(body, pos, b"/Type") {
        let mut after = idx + 5;
        while after < body.len() && body[after].is_ascii_whitespace() {
            after += 1;
        }
        if body[after..].starts_with(expected) {
            let next = after + expected.len();
            if next >= body.len() || !body[next].is_ascii_alphabetic() {
                return true;
            }
        }
        pos = idx + 5;
    }
    false
}

fn is_pdf_ref_marker(token: &str) -> bool {
    token.starts_with('R')
        && token
            .as_bytes()
            .get(1)
            .is_none_or(|b| !b.is_ascii_alphanumeric())
}

fn parse_single_ref(body: &[u8], key: &[u8]) -> Option<u32> {
    let mut pos = 0;
    while let Some(idx) = find_subslice(body, pos, key) {
        let after = idx + key.len();
        if after < body.len() && body[after].is_ascii_alphanumeric() {
            pos = after;
            continue;
        }
        let mut i = after;
        while i < body.len() && body[i].is_ascii_whitespace() {
            i += 1;
        }
        let slice = &body[i..body.len().min(i + 48)];
        let text = String::from_utf8_lossy(slice);
        let tokens: Vec<&str> = text.split_whitespace().collect();
        if tokens.len() >= 3
            && is_pdf_ref_marker(tokens[2])
            && let Ok(num) = tokens[0].parse::<u32>()
            && tokens[1].parse::<u16>().is_ok()
        {
            return Some(num);
        }
        pos = after;
    }
    None
}

fn parse_integer_entry(body: &[u8], key: &[u8]) -> Option<usize> {
    let mut pos = 0;
    while let Some(idx) = find_subslice(body, pos, key) {
        let after = idx + key.len();
        if after < body.len() && body[after].is_ascii_alphanumeric() {
            pos = after;
            continue;
        }
        let mut i = after;
        while i < body.len() && body[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < body.len() && body[i].is_ascii_digit() {
            i += 1;
        }
        if i > start
            && let Ok(val) = std::str::from_utf8(&body[start..i]).ok()?.parse()
        {
            return Some(val);
        }
        pos = after;
    }
    None
}

fn parse_array_refs(body: &[u8], key: &[u8]) -> Vec<u32> {
    let Some(idx) = find_subslice(body, 0, key) else {
        return Vec::new();
    };
    let mut i = idx + key.len();
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

fn parse_contents_refs(body: &[u8]) -> Vec<u32> {
    parse_array_refs(body, b"/Contents")
}

fn extract_balanced_pdf_dict(slice: &[u8]) -> Option<&[u8]> {
    let mut i = 0;
    while i < slice.len() && slice[i].is_ascii_whitespace() {
        i += 1;
    }
    if !slice[i..].starts_with(b"<<") {
        return None;
    }
    let start = i;
    i += 2;
    let mut depth = 1usize;
    while i < slice.len() && depth > 0 {
        if slice[i] == b'(' {
            let (_, next) = parse_pdf_literal_string(slice, i);
            i = next;
            continue;
        }
        if slice[i..].starts_with(b"<<") {
            depth += 1;
            i += 2;
            continue;
        }
        if slice[i..].starts_with(b">>") {
            depth -= 1;
            i += 2;
            continue;
        }
        i += 1;
    }
    Some(&slice[start..i])
}

fn extract_all_pdf_refs(slice: &[u8]) -> Vec<u32> {
    let text = String::from_utf8_lossy(slice);
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_ascii_whitespace() || matches!(c, '<' | '>' | '[' | ']' | '/'))
        .filter(|s| !s.is_empty())
        .collect();
    let mut refs = Vec::new();
    for window in tokens.windows(3) {
        if is_pdf_ref_marker(window[2])
            && let Ok(num) = window[0].parse::<u32>()
            && window[1].parse::<u16>().is_ok()
        {
            refs.push(num);
        }
    }
    refs
}

fn inspect_page_xobjects(
    page_body: &[u8],
    inherited_resources: &[u32],
    objects: &HashMap<u32, &[u8]>,
    page_num: usize,
) -> (bool, Vec<String>) {
    let mut has_image = false;
    let mut reasons = Vec::new();
    let mut push_reason = |reason: String| {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    };

    if contains_pdf_subtype(page_body, b"/Image") {
        has_image = true;
        push_reason(format!("第 {page_num} 页包含未提取的图像内容"));
    }
    if contains_pdf_subtype(page_body, b"/Form") {
        push_reason(format!(
            "第 {page_num} 页包含未提取的 Form XObject（可能含正文、图表或嵌入图像）"
        ));
    }

    let mut res_bodies = vec![page_body];
    if let Some(res_ref) = parse_single_ref(page_body, b"/Resources") {
        if let Some(res_body) = objects.get(&res_ref) {
            res_bodies.push(res_body);
        } else {
            push_reason(format!("第 {page_num} 页引用了缺失的资源字典 ({res_ref})"));
        }
    }
    for res_ref in inherited_resources {
        if let Some(res_body) = objects.get(res_ref) {
            res_bodies.push(res_body);
        } else {
            push_reason(format!("第 {page_num} 页继承了缺失的资源字典 ({res_ref})"));
        }
    }

    for res_body in res_bodies {
        if contains_pdf_subtype(res_body, b"/Image") {
            has_image = true;
            push_reason(format!("第 {page_num} 页包含未提取的图像内容"));
        }
        if contains_pdf_subtype(res_body, b"/Form") {
            push_reason(format!(
                "第 {page_num} 页包含未提取的 Form XObject（可能含正文、图表或嵌入图像）"
            ));
        }

        let mut pos = 0;
        while let Some(idx) = find_subslice(res_body, pos, b"/XObject") {
            let after = idx + 8;
            pos = after;
            if after < res_body.len() && res_body[after].is_ascii_alphanumeric() {
                continue;
            }
            let tail = &res_body[after..];
            let mut xobj_refs = Vec::new();
            if let Some(dict_slice) = extract_balanced_pdf_dict(tail) {
                xobj_refs.extend(extract_all_pdf_refs(dict_slice));
            } else if let Some(xobj_ref) = parse_single_ref(&res_body[idx..], b"/XObject") {
                if let Some(xobj_dict) = objects.get(&xobj_ref) {
                    let dict_slice = extract_balanced_pdf_dict(xobj_dict).unwrap_or(xobj_dict);
                    xobj_refs.extend(extract_all_pdf_refs(dict_slice));
                } else {
                    push_reason(format!("第 {page_num} 页引用了缺失的 XObject 字典 ({xobj_ref})"));
                }
            }

            for num in xobj_refs {
                match objects.get(&num) {
                    None => {
                        push_reason(format!("第 {page_num} 页引用了缺失的 XObject ({num})"));
                    }
                    Some(target) => {
                        if contains_pdf_subtype(target, b"/Image") {
                            has_image = true;
                            push_reason(format!("第 {page_num} 页包含未提取的图像内容"));
                        } else if contains_pdf_subtype(target, b"/Form") {
                            push_reason(format!(
                                "第 {page_num} 页包含未提取的 Form XObject（可能含正文、图表或嵌入图像）"
                            ));
                        } else {
                            push_reason(format!("第 {page_num} 页包含未提取的 XObject 内容"));
                        }
                    }
                }
            }
        }
    }

    (has_image, reasons)
}

fn scan_content_stream_xobjects(stream: &[u8]) -> (bool, bool) {
    let mut has_inline_image = false;
    let mut has_do_operator = false;
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
        if b == b'(' {
            let (_, next) = parse_pdf_literal_string(stream, i);
            i = next;
            continue;
        }
        if b == b'<' && stream.get(i + 1) != Some(&b'<') {
            let (_, next) = parse_pdf_hex_string(stream, i);
            i = next;
            continue;
        }
        if b == b'/' {
            i += 1;
            while i < stream.len()
                && !stream[i].is_ascii_whitespace()
                && !matches!(
                    stream[i],
                    b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
                )
            {
                i += 1;
            }
            continue;
        }
        if matches!(b, b'<' | b'>' | b'[' | b']' | b'{' | b'}') {
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
        match &stream[start..i] {
            b"BI" => has_inline_image = true,
            b"Do" => has_do_operator = true,
            _ => {}
        }
        if has_inline_image && has_do_operator {
            break;
        }
    }
    (has_inline_image, has_do_operator)
}

fn extract_stream_bytes(obj_body: &[u8], page_num: usize) -> Result<Vec<u8>, ExtractError> {
    let stream_kw = find_subslice(obj_body, 0, b"stream").ok_or_else(|| {
        ExtractError::Other(anyhow::anyhow!(
            "PDF content object on page {page_num} has no stream"
        ))
    })?;
    let dict = &obj_body[..stream_kw];
    let mut data_start = stream_kw + 6;
    if obj_body.get(data_start..data_start + 2) == Some(b"\r\n") {
        data_start += 2;
    } else if obj_body.get(data_start) == Some(&b'\n') || obj_body.get(data_start) == Some(&b'\r') {
        data_start += 1;
    } else {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "Malformed PDF stream header on page {page_num}"
        )));
    }
    let endstream = find_subslice(obj_body, data_start, b"endstream").ok_or_else(|| {
        ExtractError::Other(anyhow::anyhow!(
            "Malformed PDF stream end on page {page_num}"
        ))
    })?;
    let mut data = &obj_body[data_start..endstream];
    while data.last().is_some_and(|b| matches!(b, b'\r' | b'\n')) {
        data = &data[..data.len() - 1];
    }

    let parsed = PdfStream { dict, data };
    if let Some(filter_pos) = find_subslice(parsed.dict, 0, b"/Filter") {
        let after = &parsed.dict[filter_pos + 7..];
        let trimmed = after
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map_or(&[][..], |idx| &after[idx..]);
        let is_flate = trimmed.starts_with(b"/FlateDecode")
            && trimmed
                .get(12)
                .is_none_or(|b| !b.is_ascii_alphanumeric());
        if !is_flate {
            return Err(ExtractError::Other(anyhow::anyhow!(
                "PDF stream on page {page_num} uses an unsupported filter"
            )));
        }
        let mut out = Vec::new();
        let mut decoder = ZlibDecoder::new(parsed.data).take((MAX_PDF_STREAM_BYTES + 1) as u64);
        if decoder.read_to_end(&mut out).is_ok()
            && !out.is_empty()
            && out.len() <= MAX_PDF_STREAM_BYTES
        {
            return Ok(out);
        }
        out.clear();
        let mut raw_decoder =
            DeflateDecoder::new(parsed.data).take((MAX_PDF_STREAM_BYTES + 1) as u64);
        if raw_decoder.read_to_end(&mut out).is_ok()
            && !out.is_empty()
            && out.len() <= MAX_PDF_STREAM_BYTES
        {
            return Ok(out);
        }
        return Err(ExtractError::Other(anyhow::anyhow!(
            "Could not decompress PDF FlateDecode stream on page {page_num}"
        )));
    }
    if parsed.data.len() > MAX_PDF_STREAM_BYTES {
        return Err(ExtractError::Other(anyhow::anyhow!(
            "PDF stream on page {page_num} exceeds size limit"
        )));
    }
    Ok(parsed.data.to_vec())
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

        // PNG succeeds for vision providers and fails explicitly for non-vision providers.
        let png_path = dir.path().join("pic.png");
        let png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        std::fs::write(&png_path, png_bytes)?;
        let extracted = extract_attachment(
            &png_path,
            &fixture_attachment("pic.png", png_bytes.len() as u64),
            AiProvider::OpenAi,
        )?;
        match extracted {
            ExtractedAttachment::Image { image_url, .. } => {
                assert!(image_url.starts_with("data:image/png;base64,"));
            }
            other => panic!("expected Image, got {other:?}"),
        }
        let vision_err = extract_attachment(
            &png_path,
            &fixture_attachment("pic.png", png_bytes.len() as u64),
            AiProvider::DeepSeek,
        )
        .unwrap_err();
        assert!(matches!(vision_err, ExtractError::VisionUnsupported));

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

    #[test]
    fn mixed_text_and_scanned_pdf_is_never_reported_as_fully_read() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
        pdf.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >> endobj\n");
        pdf.extend_from_slice(b"3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n");
        pdf.extend_from_slice(b"4 0 obj << /Length 60 >>\nstream\nBT /F1 12 Tf (Page one carries plenty of searchable words) Tj ET\nendstream\nendobj\n");
        pdf.extend_from_slice(b"5 0 obj << /Type /Page /Parent 2 0 R /Resources<</XObject<</Im1 7 0 R>>>> /Contents 6 0 R >> endobj\n");
        pdf.extend_from_slice(b"6 0 obj << /Length 13 >>\nstream\nBT /Im1 Do ET\nendstream\nendobj\n");
        pdf.extend_from_slice(b"7 0 obj << /Type /XObject /Subtype /Image /Width 10 /Height 10 >> endobj\n%%EOF");
        let path = dir.path().join("mixed.pdf");
        std::fs::write(&path, &pdf)?;
        let result = extract_attachment(
            &path,
            &fixture_attachment("mixed.pdf", pdf.len() as u64),
            AiProvider::DeepSeek,
        );
        assert!(matches!(result, Err(ExtractError::ScannedPdfUnsupported)), "a scanned page must not be silently dropped");
        Ok(())
    }

    #[test]
    fn xlsx_with_a_missing_declared_sheet_fails_instead_of_skipping_it() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("partial.xlsx");
        {
            let mut zip = ZipWriter::new(File::create(&path)?);
            zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                    <sheet name="One" sheetId="1" r:id="rId1"/><sheet name="Two" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
            )?;
            zip.start_file("xl/_rels/workbook.xml.rels", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/>
                    <Relationship Id="rId2" Target="worksheets/sheet2.xml"/></Relationships>"#,
            )?;
            zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<worksheet><sheetData><row><c t="inlineStr"><is><t>only</t></is></c></row></sheetData></worksheet>"#)?;
            zip.finish()?;
        }
        let err = extract_attachment(
            &path,
            &fixture_attachment("partial.xlsx", std::fs::metadata(&path)?.len()),
            AiProvider::Grok,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Two"), "error must name the missing sheet: {err}");
        Ok(())
    }

    #[test]
    fn docx_with_images_or_unparsed_parts_fails_incomplete() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // (a) Text + image in DOCX must fail with IncompleteExtraction before any AI send.
        let pictured = dir.path().join("pictured.docx");
        {
            let mut zip = ZipWriter::new(File::create(&pictured)?);
            zip.start_file("word/document.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
                    <w:p><w:r><w:t>Text next to a picture</w:t></w:r></w:p>
                    <w:p><w:r><w:drawing/></w:r></w:p></w:body></w:document>"#,
            )?;
            zip.start_file("word/media/image1.png", SimpleFileOptions::default())?;
            zip.write_all(b"\x89PNG")?;
            zip.finish()?;
        }
        let err = extract_attachment(
            &pictured,
            &fixture_attachment("pictured.docx", std::fs::metadata(&pictured)?.len()),
            AiProvider::DeepSeek,
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::IncompleteExtraction { .. }),
            "pictured DOCX must fail with IncompleteExtraction, got {err:?}"
        );

        // (b) Text + footnotes.xml must fail conservatively instead of dropping footnotes.
        let footnoted = dir.path().join("footnoted.docx");
        {
            let mut zip = ZipWriter::new(File::create(&footnoted)?);
            zip.start_file("word/document.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
                    <w:p><w:r><w:t>Main body text</w:t></w:r></w:p></w:body></w:document>"#,
            )?;
            zip.start_file("word/footnotes.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#)?;
            zip.finish()?;
        }
        let err = extract_attachment(
            &footnoted,
            &fixture_attachment("footnoted.docx", std::fs::metadata(&footnoted)?.len()),
            AiProvider::OpenAi,
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::IncompleteExtraction { ref reasons, .. } if reasons.iter().any(|r| r.contains("footnotes.xml"))),
            "DOCX with footnotes must fail with IncompleteExtraction, got {err:?}"
        );

        // (c) Text + header1.xml must fail conservatively.
        let headered = dir.path().join("headered.docx");
        {
            let mut zip = ZipWriter::new(File::create(&headered)?);
            zip.start_file("word/document.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
                    <w:p><w:r><w:t>Body text</w:t></w:r></w:p></w:body></w:document>"#,
            )?;
            zip.start_file("word/header1.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#)?;
            zip.finish()?;
        }
        let err = extract_attachment(
            &headered,
            &fixture_attachment("headered.docx", std::fs::metadata(&headered)?.len()),
            AiProvider::Grok,
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::IncompleteExtraction { ref reasons, .. } if reasons.iter().any(|r| r.contains("header1.xml"))),
            "DOCX with header1.xml must fail with IncompleteExtraction, got {err:?}"
        );
        Ok(())
    }

    #[test]
    fn pdf_with_text_and_form_or_image_xobject_fails_local_extraction() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // (a) Text + inline /XObject Form + /Fm1 Do
        let mut form_inline_pdf = Vec::new();
        form_inline_pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
        form_inline_pdf.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
        form_inline_pdf.extend_from_slice(b"3 0 obj << /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm1 5 0 R >> >> /Contents 4 0 R >> endobj\n");
        form_inline_pdf.extend_from_slice(b"4 0 obj << /Length 73 >>\nstream\nBT /F1 12 Tf (Searchable paragraph with plenty of characters) Tj ET /Fm1 Do\nendstream\nendobj\n");
        form_inline_pdf.extend_from_slice(b"5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length 28 >>\nstream\nBT (Hidden form text) Tj ET\nendstream\nendobj\n%%EOF");
        let form_inline_path = dir.path().join("form-inline.pdf");
        std::fs::write(&form_inline_path, &form_inline_pdf)?;

        let err = extract_attachment(
            &form_inline_path,
            &fixture_attachment("form-inline.pdf", form_inline_pdf.len() as u64),
            AiProvider::DeepSeek,
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::IncompleteExtraction { ref reasons, .. } if reasons.iter().any(|r| r.contains("Form XObject"))),
            "PDF with Form XObject must fail local extraction, got {err:?}"
        );
        // OpenAI within 10 MiB still uses native full-PDF submission.
        let native = extract_attachment(
            &form_inline_path,
            &fixture_attachment("form-inline.pdf", form_inline_pdf.len() as u64),
            AiProvider::OpenAi,
        )?;
        assert!(matches!(native, ExtractedAttachment::NativePdf { .. }));

        // (b) Text + indirect /Resources + indirect /XObject dict pointing to /Form
        let mut form_indirect_pdf = Vec::new();
        form_indirect_pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
        form_indirect_pdf.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
        form_indirect_pdf.extend_from_slice(b"3 0 obj << /Type /Page /Parent 2 0 R /Resources 6 0 R /Contents 4 0 R >> endobj\n");
        form_indirect_pdf.extend_from_slice(b"4 0 obj << /Length 73 >>\nstream\nBT /F1 12 Tf (Searchable paragraph with plenty of characters) Tj ET /Fm1 Do\nendstream\nendobj\n");
        form_indirect_pdf.extend_from_slice(b"5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 50 50] /Length 0 >>\nstream\n\nendstream\nendobj\n");
        form_indirect_pdf.extend_from_slice(b"6 0 obj << /XObject 7 0 R >> endobj\n");
        form_indirect_pdf.extend_from_slice(b"7 0 obj << /Fm1 5 0 R >> endobj\n%%EOF");
        let form_indirect_path = dir.path().join("form-indirect.pdf");
        std::fs::write(&form_indirect_path, &form_indirect_pdf)?;

        let err = extract_attachment(
            &form_indirect_path,
            &fixture_attachment("form-indirect.pdf", form_indirect_pdf.len() as u64),
            AiProvider::DeepSeek,
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::IncompleteExtraction { ref reasons, .. } if reasons.iter().any(|r| r.contains("Form XObject"))),
            "PDF with indirect Form XObject dict must fail, got {err:?}"
        );

        // (c) Text + /Image XObject on the same page must also fail local extraction.
        let mut text_and_img_pdf = Vec::new();
        text_and_img_pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
        text_and_img_pdf.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
        text_and_img_pdf.extend_from_slice(b"3 0 obj << /Type /Page /Parent 2 0 R /Resources << /XObject << /Im1 5 0 R >> >> /Contents 4 0 R >> endobj\n");
        text_and_img_pdf.extend_from_slice(b"4 0 obj << /Length 73 >>\nstream\nBT /F1 12 Tf (Searchable paragraph with plenty of characters) Tj ET /Im1 Do\nendstream\nendobj\n");
        text_and_img_pdf.extend_from_slice(b"5 0 obj << /Type /XObject /Subtype /Image /Width 10 /Height 10 >> endobj\n%%EOF");
        let text_and_img_path = dir.path().join("text-and-image.pdf");
        std::fs::write(&text_and_img_path, &text_and_img_pdf)?;

        let err = extract_attachment(
            &text_and_img_path,
            &fixture_attachment("text-and-image.pdf", text_and_img_pdf.len() as u64),
            AiProvider::DeepSeek,
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::IncompleteExtraction { .. }),
            "PDF with text + image must fail with IncompleteExtraction, got {err:?}"
        );
        Ok(())
    }

    #[test]
    fn xlsx_preserves_sparse_columns_and_rejects_broken_rels_strings_and_xml() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // (a) Sparse coordinates A1/C1, A2/C2, C3 must keep empty middle columns.
        let sparse = dir.path().join("sparse.xlsx");
        {
            let mut zip = ZipWriter::new(File::create(&sparse)?);
            zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                    <sheet name="Ledger" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            )?;
            zip.start_file("xl/_rels/workbook.xml.rels", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>"#,
            )?;
            zip.start_file("xl/sharedStrings.xml", SimpleFileOptions::default())?;
            zip.write_all(r#"<sst><si><t>收入</t></si><si><t>成本</t></si></sst>"#.as_bytes())?;
            zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<worksheet><sheetData>
                    <row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" t="s"><v>1</v></c></row>
                    <row r="2"><c r="A2"><v>100</v></c><c r="C2"><v>40</v></c></row>
                    <row r="3"><c r="C3"><v>99</v></c></row>
                </sheetData></worksheet>"#,
            )?;
            zip.finish()?;
        }
        let extracted = extract_attachment(
            &sparse,
            &fixture_attachment("sparse.xlsx", std::fs::metadata(&sparse)?.len()),
            AiProvider::DeepSeek,
        )?;
        let ExtractedAttachment::TextChunks { chunks, .. } = extracted else {
            panic!("expected TextChunks");
        };
        let text = chunks.join("\n");
        assert!(text.contains("收入\t\t成本"), "got:\n{text}");
        assert!(text.contains("100\t\t40"), "got:\n{text}");
        assert!(text.contains("\t\t99"), "got:\n{text}");

        // (b) Missing relationship for rId1 must fail instead of guessing sheet1.xml.
        let missing_rel = dir.path().join("missing-rel.xlsx");
        {
            let mut zip = ZipWriter::new(File::create(&missing_rel)?);
            zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                    <sheet name="Actual" sheetId="1" r:id="rId9"/></sheets></workbook>"#,
            )?;
            zip.start_file("xl/_rels/workbook.xml.rels", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>"#,
            )?;
            zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<worksheet><sheetData><row><c r="A1"><v>wrong</v></c></row></sheetData></worksheet>"#)?;
            zip.finish()?;
        }
        let err = extract_attachment(
            &missing_rel,
            &fixture_attachment("missing-rel.xlsx", std::fs::metadata(&missing_rel)?.len()),
            AiProvider::Grok,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Actual"), "err: {err}");
        assert!(err.to_string().contains("rId9"), "err: {err}");

        // (c) Missing workbook.xml.rels file must fail instead of guessing sheetN.xml.
        let no_rels_file = dir.path().join("no-rels.xlsx");
        {
            let mut zip = ZipWriter::new(File::create(&no_rels_file)?);
            zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                    <sheet name="One" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            )?;
            zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<worksheet><sheetData><row><c r="A1"><v>guess</v></c></row></sheetData></worksheet>"#)?;
            zip.finish()?;
        }
        assert!(
            extract_attachment(
                &no_rels_file,
                &fixture_attachment("no-rels.xlsx", std::fs::metadata(&no_rels_file)?.len()),
                AiProvider::Grok,
            )
            .is_err()
        );

        // (d) Out-of-bounds and non-numeric shared string indices must error.
        for bad_idx in ["99", "not-a-num"] {
            let bad_ss = dir.path().join(format!("bad-ss-{bad_idx}.xlsx"));
            {
                let mut zip = ZipWriter::new(File::create(&bad_ss)?);
                zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
                zip.write_all(
                    br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                        <sheet name="One" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
                )?;
                zip.start_file("xl/_rels/workbook.xml.rels", SimpleFileOptions::default())?;
                zip.write_all(
                    br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>"#,
                )?;
                zip.start_file("xl/sharedStrings.xml", SimpleFileOptions::default())?;
                zip.write_all(br#"<sst><si><t>only</t></si></sst>"#)?;
                zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
                zip.write_all(
                    format!(
                        r#"<worksheet><sheetData><row><c r="A1" t="s"><v>{bad_idx}</v></c></row></sheetData></worksheet>"#
                    )
                    .as_bytes(),
                )?;
                zip.finish()?;
            }
            assert!(
                extract_attachment(
                    &bad_ss,
                    &fixture_attachment("bad-ss.xlsx", std::fs::metadata(&bad_ss)?.len()),
                    AiProvider::Grok,
                )
                .is_err(),
                "shared string index '{bad_idx}' must fail"
            );
        }

        // (e) Malformed XML in workbook.xml.rels or workbook.xml must error instead of returning partial sheets.
        let bad_rels_xml = dir.path().join("bad-rels-xml.xlsx");
        {
            let mut zip = ZipWriter::new(File::create(&bad_rels_xml)?);
            zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                    <sheet name="One" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            )?;
            zip.start_file("xl/_rels/workbook.xml.rels", SimpleFileOptions::default())?;
            zip.write_all(br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><broken"#)?;
            zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<worksheet><sheetData><row><c r="A1"><v>ok</v></c></row></sheetData></worksheet>"#)?;
            zip.finish()?;
        }
        assert!(
            extract_attachment(
                &bad_rels_xml,
                &fixture_attachment("bad-rels-xml.xlsx", std::fs::metadata(&bad_rels_xml)?.len()),
                AiProvider::Grok,
            )
            .is_err()
        );

        let bad_wb_xml = dir.path().join("bad-wb-xml.xlsx");
        {
            let mut zip = ZipWriter::new(File::create(&bad_wb_xml)?);
            zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                    <sheet name="One" sheetId="1" r:id="rId1"/><sheet name="Broken"#,
            )?;
            zip.start_file("xl/_rels/workbook.xml.rels", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>"#,
            )?;
            zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<worksheet><sheetData><row><c r="A1"><v>ok</v></c></row></sheetData></worksheet>"#)?;
            zip.finish()?;
        }
        assert!(
            extract_attachment(
                &bad_wb_xml,
                &fixture_attachment("bad-wb-xml.xlsx", std::fs::metadata(&bad_wb_xml)?.len()),
                AiProvider::Grok,
            )
            .is_err()
        );

        // (f) Embedded chart in XLSX must fail with IncompleteExtraction.
        let charted = dir.path().join("charted.xlsx");
        {
            let mut zip = ZipWriter::new(File::create(&charted)?);
            zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                    <sheet name="One" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            )?;
            zip.start_file("xl/_rels/workbook.xml.rels", SimpleFileOptions::default())?;
            zip.write_all(
                br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>"#,
            )?;
            zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<worksheet><sheetData><row><c r="A1"><v>ok</v></c></row></sheetData></worksheet>"#)?;
            zip.start_file("xl/charts/chart1.xml", SimpleFileOptions::default())?;
            zip.write_all(br#"<chartSpace/>"#)?;
            zip.finish()?;
        }
        let err = extract_attachment(
            &charted,
            &fixture_attachment("charted.xlsx", std::fs::metadata(&charted)?.len()),
            AiProvider::Grok,
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::IncompleteExtraction { .. }));
        Ok(())
    }
}
