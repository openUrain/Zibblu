use crate::{App, Auth, Error, Result, chats::lock_message_feed, files};
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::Response,
};
use futures_util::{TryStreamExt, stream};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, PgPool};
use std::{io, path::Path as FsPath, process::Stdio, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, process::Command};
use uuid::Uuid;
use zibblu_core::CHUNK_SIZE;

const COVER_LIMIT: i64 = 512 * 1024;
const PREVIEW_LIMIT: i64 = 32 * 1024 * 1024;
const MAX_DURATION: f64 = 600.0;
const MAX_PIXELS: u64 = 3840 * 2160;
const FORMATS: &str = "mov,matroska,webm,avi";

pub(crate) fn candidate(bytes: &[u8]) -> bool {
    bytes.get(4..8).is_some_and(|tag| matches!(tag, b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free"))
        || bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"AVI ".as_slice()))
}

pub(crate) async fn enqueue(db: &mut PgConnection, id: Uuid) -> Result<()> {
    sqlx::query("INSERT INTO video_resources (attachment_id, kind)
                 VALUES ($1, 'cover'), ($1, 'preview') ON CONFLICT DO NOTHING")
        .bind(id).execute(db).await?;
    Ok(())
}

#[derive(FromRow)]
struct Resource {
    expired: bool,
    present: bool,
    kind: String,
    status: String,
    size: Option<i64>,
    sha256: Option<String>,
}

fn mime(kind: &str) -> &'static str {
    if kind == "cover" { "image/jpeg" } else { "video/mp4" }
}

async fn resources(db: &mut PgConnection, id: Uuid, auth: &Auth) -> Result<Vec<Resource>> {
    auth.require(db).await?;
    let rows = sqlx::query_as::<_, Resource>(
        "SELECT coalesce(m.expires_at <= statement_timestamp(), false)
                    OR coalesce(a.expires_at <= statement_timestamp(), false) AS expired,
                coalesce(a.complete, false) AS present,
                coalesce(r.kind, '') AS kind, coalesce(r.status, 'unsupported') AS status, r.size, r.sha256
         FROM m1_visible_messages m LEFT JOIN attachments a ON a.id = m.attachment_id
         LEFT JOIN video_resources r ON r.attachment_id = coalesce(a.payload_id, a.id)
         WHERE m.attachment_id = $1 AND m.account_id = $2 AND m.recall_seq IS NULL",
    ).bind(id).bind(auth.user_id).fetch_all(db).await?;
    let first = rows.first().ok_or_else(Error::missing)?;
    if first.expired { return Err(Error(StatusCode::GONE, "The file expired.")); }
    if !first.present { return Err(Error::missing()); }
    Ok(rows)
}

pub(crate) async fn metadata(
    State(app): State<App>, auth: Auth, Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    let mut db = app.db.acquire().await?;
    let rows = resources(&mut db, id, &auth).await?;
    let resource = |kind: &str| {
        let row = rows.iter().find(|r| r.kind == kind);
        json!({
            "id": format!("{id}/{kind}"),
            "status": row.map_or("unsupported", |r| r.status.as_str()),
            "mime": mime(kind),
            "size": row.and_then(|r| r.size),
            "sha256": row.and_then(|r| r.sha256.as_deref()),
        })
    };
    Ok(Json(json!({"attachment_id": id, "cover": resource("cover"), "preview": resource("preview")})))
}

// Single byte range. An omitted end means EOF; suffix ranges clamp to the resource length.
fn range(value: &str, size: i64) -> Option<(i64, i64)> {
    let (first, last) = value.strip_prefix("bytes=")?.split_once('-')?;
    if size <= 0 { return None; }
    let number = |v: &str| -> Option<i64> {
        if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) { return None; }
        v.parse().ok()
    };
    if first.is_empty() {
        let suffix = number(last)?;
        return (suffix > 0).then(|| ((size - suffix).max(0), size - 1));
    }
    let start = number(first)?;
    let end = if last.is_empty() { size - 1 } else { number(last)?.min(size - 1) };
    (start < size && start <= end).then_some((start, end))
}

pub(crate) async fn read(
    State(app): State<App>, auth: Auth, Path((id, kind)): Path<(Uuid, String)>,
    method: Method, headers: HeaderMap,
) -> Result<Response> {
    if !matches!(kind.as_str(), "cover" | "preview") { return Err(Error::missing()); }
    let mut db = app.db.acquire().await?;
    let rows = resources(&mut db, id, &auth).await?;
    let row = rows.iter().find(|r| r.kind == kind);
    let size = match row.map(|r| r.status.as_str()) {
        Some("ready") => row.and_then(|r| r.size).ok_or_else(Error::internal)?,
        Some("pending") => return Err(Error::conflict("Video resource is being prepared.")),
        Some("failed") => return Err(Error(StatusCode::UNPROCESSABLE_ENTITY, "Video processing failed.")),
        Some("quota") => return Err(Error(StatusCode::INSUFFICIENT_STORAGE, "Video resource storage limit reached.")),
        _ => return Err(Error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "No derivative is available. Download the original.")),
    };
    drop(db);
    let requested = if method == Method::HEAD { None } else { headers.get(header::RANGE) };
    let (start, end) = match requested {
        None => (0, size - 1),
        Some(value) => match value.to_str().ok().and_then(|v| range(v, size)) {
            Some(bounds) => bounds,
            None => return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .header(header::ACCEPT_RANGES, "bytes")
                .body(Body::empty()).map_err(|_| Error::internal()),
        },
    };
    let mut response = Response::builder()
        .status(if requested.is_some() { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_TYPE, mime(&kind))
        .header(header::CONTENT_LENGTH, (end - start + 1).to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header("x-content-type-options", "nosniff");
    if requested.is_some() {
        response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    let body = if method == Method::HEAD { Body::empty() } else {
        Body::from_stream(stream::try_unfold((app.db, auth, kind, start), move |(db, auth, kind, pos)| async move {
            if pos > end { return Ok::<_, io::Error>(None); }
            let offset = pos / CHUNK_SIZE as i64 * CHUNK_SIZE as i64;
            let failure = || io::Error::other("Video resource is no longer available");
            let mut conn = db.acquire().await.map_err(|_| failure())?;
            auth.require(&mut conn).await.map_err(|_| failure())?;
            // Recheck visibility and both expiry deadlines on each bounded read.
            let chunk: Vec<u8> = sqlx::query_scalar(
                "SELECT c.data FROM video_resource_chunks c
                 JOIN video_resources r ON (r.attachment_id, r.kind) = (c.attachment_id, c.kind)
                 JOIN attachments a ON coalesce(a.payload_id, a.id) = r.attachment_id
                 JOIN m1_visible_messages m ON m.attachment_id = a.id
                 WHERE a.id = $1 AND r.kind = $2 AND c.byte_offset = $3
                   AND m.account_id = $4 AND m.recall_seq IS NULL AND a.complete AND r.status = 'ready'
                   AND coalesce(m.expires_at > statement_timestamp(), true)
                   AND coalesce(a.expires_at > statement_timestamp(), true)",
            ).bind(id).bind(&kind).bind(offset).bind(auth.user_id)
                .fetch_optional(&mut *conn).await.map_err(|_| failure())?.ok_or_else(failure)?;
            drop(conn);
            let skip = (pos - offset) as usize;
            let count = (end - pos + 1).min(CHUNK_SIZE as i64 - (pos - offset)) as usize;
            let data = chunk.get(skip..skip + count).ok_or_else(failure)?.to_vec();
            Ok(Some((data, (db, auth, kind, pos + count as i64))))
        }))
    };
    response.body(body).map_err(|_| Error::internal())
}

#[derive(FromRow)]
struct Job {
    id: Uuid,
    owner_id: Uuid,
    size: i64,
    sha256: String,
}

// The advisory transaction lock prevents multiple workers from processing simultaneously.
// No feed/attachment lock is held while staging or running external tools. A crash rolls
// back the uncommitted work and leaves the pending resources available for the next worker.
pub async fn process_one(db: &PgPool) -> anyhow::Result<bool> {
    let mut tx = db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(1002)")
        .fetch_one(&mut *tx).await?;
    if !locked { return Ok(false); }
    let job = sqlx::query_as::<_, Job>(
        "SELECT a.id, a.owner_id, a.size, a.sha256 FROM attachments a
         JOIN video_resources r ON r.attachment_id = a.id AND r.kind = 'cover'
         WHERE a.complete AND r.status = 'pending' AND EXISTS (
             SELECT 1 FROM attachments ref JOIN messages m ON m.attachment_id = ref.id
             WHERE coalesce(ref.payload_id, ref.id) = a.id AND ref.complete AND m.recall_seq IS NULL
               AND coalesce(ref.expires_at > clock_timestamp(), true)
               AND coalesce(m.expires_at > clock_timestamp(), true))
         ORDER BY a.id LIMIT 1",
    ).fetch_optional(&mut *tx).await?;
    let Some(job) = job else { return Ok(false); };
    let scratch = tempfile::tempdir();
    let outcomes = match scratch.as_ref() {
        Ok(dir) if cfg!(target_os = "linux") => {
            match tokio::time::timeout(Duration::from_secs(30), stage(&mut tx, &job, dir.path())).await {
                Ok(Ok(())) => generate(dir.path(), job.size).await,
                _ => ["failed", "failed"],
            }
        }
        _ => ["failed", "failed"],
    };
    lock_message_feed(&mut tx).await.map_err(|_| anyhow::anyhow!("Could not lock delivery feed"))?;
    if source_live(&mut tx, job.id).await? {
        let (mut account, _, mut total, _) = files::storage_usage(&mut tx, job.owner_id).await
            .map_err(|_| anyhow::anyhow!("Could not check media storage quota"))?;
        for (index, (kind, name)) in [("cover", "cover.jpg"), ("preview", "preview.mp4")].into_iter().enumerate() {
            let mut status = outcomes[index];
            if status == "ready" {
                let path = scratch.as_ref().unwrap().path().join(name);
                let size = tokio::fs::metadata(&path).await?.len() as i64;
                if account + size > files::ACCOUNT_BYTES || total + size > files::SERVICE_BYTES {
                    status = "quota";
                } else {
                    store(&mut tx, job.id, kind, &path, size).await?;
                    account += size;
                    total += size;
                    continue;
                }
            }
            sqlx::query("UPDATE video_resources SET status = $3 WHERE attachment_id = $1 AND kind = $2 AND status = 'pending'")
                .bind(job.id).bind(kind).bind(status).execute(&mut *tx).await?;
        }
    }
    if !source_live(&mut tx, job.id).await? {
        sqlx::query("DELETE FROM video_resources WHERE attachment_id = $1")
            .bind(job.id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(true)
}

async fn source_live(db: &mut PgConnection, id: Uuid) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM attachments a JOIN messages m ON m.attachment_id = a.id
         WHERE coalesce(a.payload_id, a.id) = $1 AND a.complete AND m.recall_seq IS NULL
         AND coalesce(a.expires_at > clock_timestamp(), true)
         AND coalesce(m.expires_at > clock_timestamp(), true))",
    ).bind(id).fetch_one(db).await?)
}

async fn stage(db: &mut PgConnection, job: &Job, dir: &FsPath) -> anyhow::Result<()> {
    anyhow::ensure!((1..=256 * 1024 * 1024).contains(&job.size));
    let mut file = tokio::fs::File::create(dir.join("input")).await?;
    let mut chunks = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT data FROM attachment_chunks WHERE attachment_id = $1 ORDER BY byte_offset",
    ).bind(job.id).fetch(db);
    let mut size = 0_i64;
    let mut hash = Sha256::new();
    while let Some(chunk) = chunks.try_next().await? {
        size += chunk.len() as i64;
        anyhow::ensure!(size <= job.size);
        hash.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    anyhow::ensure!(size == job.size && format!("{:x}", hash.finalize()) == job.sha256);
    Ok(())
}

async fn store(db: &mut PgConnection, id: Uuid, kind: &str, path: &FsPath, size: i64) -> anyhow::Result<()> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hash = Sha256::new();
    let mut offset = 0_i64;
    while offset < size {
        let mut chunk = vec![0; (size - offset).min(CHUNK_SIZE as i64) as usize];
        file.read_exact(&mut chunk).await?;
        hash.update(&chunk);
        sqlx::query("INSERT INTO video_resource_chunks (attachment_id, kind, byte_offset, data) VALUES ($1,$2,$3,$4)")
            .bind(id).bind(kind).bind(offset).bind(&chunk).execute(&mut *db).await?;
        offset += chunk.len() as i64;
    }
    sqlx::query("UPDATE video_resources SET status = 'ready', size = $3, sha256 = $4 WHERE attachment_id = $1 AND kind = $2 AND status = 'pending'")
        .bind(id).bind(kind).bind(size).bind(format!("{:x}", hash.finalize())).execute(db).await?;
    Ok(())
}

// Linux service runtime: one subprocess, strict CPU/address-space/file-size limits,
// no inherited database environment, no network protocols, no unbounded stderr/stdout.
async fn run(dir: &FsPath, program: &str, args: &[&str], output_limit: i64, seconds: u64, stdout: Stdio) -> std::result::Result<(), &'static str> {
    let mut command = Command::new("prlimit");
    command.env_clear().env("PATH", "/usr/bin:/bin")
        .current_dir(dir)
        .args(["--as=2147483648", "--cpu=90", &format!("--fsize={output_limit}"), "--", program])
        .args(args).stdin(Stdio::null()).stdout(stdout).stderr(Stdio::null()).kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| "failed")?;
    match tokio::time::timeout(Duration::from_secs(seconds), child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) if status.code().is_some_and(|code| code != 126 && code != 127) => Err("unsupported"),
        _ => { let _ = child.kill().await; Err("failed") }
    }
}

async fn probe(dir: &FsPath, input: &str) -> std::result::Result<f64, &'static str> {
    let output = dir.join("probe.json");
    let stdout = Stdio::from(std::fs::File::create(&output).map_err(|_| "failed")?);
    run(dir, "ffprobe", &[
        "-v", "error", "-max_alloc", "134217728", "-protocol_whitelist", "file",
        "-format_whitelist", FORMATS, "-max_streams", "32", "-probesize", "8388608", "-analyzeduration", "5000000",
        "-select_streams", "V:0", "-show_entries", "stream=width,height:format=duration",
        "-of", "json", input,
    ], 16384, 10, stdout).await?;
    let bytes = tokio::fs::read(output).await.map_err(|_| "failed")?;
    fn parse(bytes: &[u8]) -> Option<f64> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        let video = value["streams"].as_array()?.first()?;
        let width = video["width"].as_u64()?;
        let height = video["height"].as_u64()?;
        let duration = value["format"]["duration"].as_str()?.parse::<f64>().ok()?;
        (width > 0 && height > 0 && width <= 4096 && height <= 4096
            && width.checked_mul(height)? <= MAX_PIXELS && duration.is_finite()
            && duration > 0.0 && duration <= MAX_DURATION + 0.1).then_some(duration)
    }
    parse(&bytes).ok_or("unsupported")
}

async fn good_size(dir: &FsPath, name: &str, limit: i64) -> Option<i64> {
    tokio::fs::metadata(dir.join(name)).await.ok().map(|m| m.len() as i64)
        .filter(|size| (1..=limit).contains(size))
}

async fn generate(dir: &FsPath, original_size: i64) -> [&'static str; 2] {
    let duration = match probe(dir, "input").await {
        Ok(duration) => duration,
        Err(status) => return [status, status],
    };
    if duration > MAX_DURATION { return ["unsupported", "unsupported"]; }
    let input = [
        "-nostdin", "-v", "error", "-max_alloc", "134217728", "-protocol_whitelist", "file",
        "-format_whitelist", FORMATS, "-max_streams", "32", "-probesize", "8388608", "-analyzeduration", "5000000",
        "-threads", "2", "-max_pixels", "8294400", "-i", "input",
    ];
    let cover = [
        "-map", "0:V:0", "-an", "-sn", "-dn", "-map_metadata", "-1", "-filter_threads", "1",
        "-vf", "scale=w='min(iw,640)':h='min(ih,640)':force_original_aspect_ratio=decrease:force_divisible_by=2",
        "-frames:v", "1", "-q:v", "4", "-threads", "2", "-f", "image2", "cover.jpg",
    ];
    let preview = [
        "-map", "0:V:0", "-map", "0:a:0?", "-sn", "-dn", "-map_metadata", "-1", "-map_chapters", "-1",
        "-filter_threads", "1", "-vf", "scale=w='min(iw,1280)':h='min(ih,720)':force_original_aspect_ratio=decrease:force_divisible_by=2,fps=30",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "fast", "-crf", "25", "-threads", "2",
        "-c:a", "aac", "-b:a", "96k", "-ac", "2", "-movflags", "+faststart",
        "-t", "600", "-f", "mp4", "preview.mp4",
    ];
    let cover_status = if run(dir, "ffmpeg", &[input.as_slice(), &cover].concat(), COVER_LIMIT, 20, Stdio::null()).await.is_ok()
        && good_size(dir, "cover.jpg", COVER_LIMIT).await.is_some() { "ready" } else { "failed" };
    let preview_status = if run(dir, "ffmpeg", &[input.as_slice(), &preview].concat(), PREVIEW_LIMIT, 120, Stdio::null()).await.is_ok() {
        match good_size(dir, "preview.mp4", PREVIEW_LIMIT).await {
            Some(size) if size >= original_size => "not_smaller",
            Some(_) if probe(dir, "preview.mp4").await.is_ok_and(|d| (d - duration).abs() <= 0.1_f64.max(duration * 0.02)) => "ready",
            _ => "failed",
        }
    } else { "failed" };
    [cover_status, preview_status]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn actual_process_limits() -> anyhow::Result<()> {
        let scratch = tempfile::tempdir()?;
        assert_eq!(run(scratch.path(), "sleep", &["5"], 1024, 1, Stdio::null()).await, Err("failed"));
        let result = run(scratch.path(), "ffmpeg", &[
            "-nostdin", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=128x128:rate=30",
            "-frames:v", "1", "-pix_fmt", "yuv420p", "-threads", "1", "-f", "rawvideo", "capped.bin",
        ], 1024, 10, Stdio::null()).await;
        assert!(result.is_err());
        assert!(std::fs::metadata(scratch.path().join("capped.bin"))?.len() <= 1024);
        Ok(())
    }

    #[test]
    fn ranges_and_signatures() {
        assert_eq!(range("bytes=3-8", 10), Some((3, 8)));
        assert_eq!(range("bytes=3-", 10), Some((3, 9)));
        assert_eq!(range("bytes=-4", 10), Some((6, 9)));
        assert_eq!(range("bytes=8-99", 10), Some((8, 9)));
        assert_eq!(range("bytes=-99", 10), Some((0, 9)));
        for bad in ["bytes=10-", "bytes=8-3", "bytes=-0", "bytes=0-1,3-4", "items=0-1", "bytes=+1-2"] {
            assert_eq!(range(bad, 10), None);
        }
        assert!(candidate(b"\0\0\0\x18ftypisom"));
        assert!(candidate(b"\x1a\x45\xdf\xa3"));
        assert!(!candidate(b"RIFF\0\0\0\0WEBP"));
        assert!(!candidate(b"not a movie"));
    }
}
