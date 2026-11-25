use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
    sync::Notify,
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const MIN_TRIM_DURATION_SEC: f64 = 0.2;
const DURATION_TOLERANCE_SEC: f64 = 0.05;
const PROBE_TIMEOUT_SECS: u64 = 15;
const THUMBNAIL_TIMEOUT_SECS: u64 = 20;
const MAX_THUMBNAIL_BYTES: usize = 512 * 1024;
const TRIM_TEMP_PREFIX: &str = "trim-";
const TRIM_TEMP_SUFFIX: &str = ".mp4";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoProbeInfo {
    pub duration_sec: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoExportProgress {
    pub export_id: Uuid,
    pub status: String,
    pub progress: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    format: Option<FfprobeFormat>,
    streams: Option<Vec<FfprobeStream>>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    duration: Option<String>,
}

pub fn resolve_media_binary(program: &str, bundled_dir: Option<&Path>) -> PathBuf {
    let exe_name = if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_owned()
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = bundled_dir {
        candidates.push(dir.join(&exe_name));
        candidates.push(dir.join("ffmpeg").join(&exe_name));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        candidates.push(exe_dir.join("ffmpeg").join(&exe_name));
        candidates.push(exe_dir.join("bin").join("ffmpeg").join(&exe_name));
        candidates.push(exe_dir.join(&exe_name));
    }
    if let Some(env_dir) = std::env::var_os("ZIBBLU_FFMPEG_DIR") {
        let dir = PathBuf::from(env_dir);
        candidates.push(dir.join(&exe_name));
        candidates.push(dir.join("ffmpeg").join(&exe_name));
    }
    #[cfg(debug_assertions)]
    {
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("bin")
                .join("ffmpeg")
                .join(&exe_name),
        );
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(program))
}

fn new_media_command(program: &str, bundled_dir: Option<&Path>) -> Command {
    let resolved = resolve_media_binary(program, bundled_dir);
    let mut cmd = Command::new(resolved);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

fn map_spawn_error(program: &str, err: std::io::Error) -> anyhow::Error {
    if err.kind() == ErrorKind::NotFound {
        anyhow!("`{program}` runtime was not found in the bundled ffmpeg directory or on PATH.")
    } else {
        anyhow!("Failed to launch `{program}`: {err}")
    }
}

fn parse_frame_rate(raw: Option<&str>) -> Option<f64> {
    let value = raw?.trim();
    if value.is_empty() {
        return None;
    }
    let fps = if let Some((num, den)) = value.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if !n.is_finite() || !d.is_finite() || d <= 0.0 || n <= 0.0 {
            return None;
        }
        n / d
    } else {
        value.parse::<f64>().ok()?
    };
    (fps.is_finite() && fps > 0.0 && fps <= 1000.0).then_some(fps)
}

fn parse_positive_f64(raw: Option<&str>) -> Option<f64> {
    let parsed = raw?.trim().parse::<f64>().ok()?;
    (parsed.is_finite() && parsed > 0.0).then_some(parsed)
}

pub fn parse_ffprobe_json(raw: &str) -> Result<VideoProbeInfo> {
    let parsed: FfprobeOutput = serde_json::from_str(raw).context("Invalid ffprobe JSON output")?;
    let streams = parsed.streams.unwrap_or_default();
    let video = streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .context("No video stream found")?;

    let width = video.width.filter(|&w| w > 0).context("Invalid video width")?;
    let height = video.height.filter(|&h| h > 0).context("Invalid video height")?;

    let duration_sec = parse_positive_f64(parsed.format.as_ref().and_then(|f| f.duration.as_deref()))
        .or_else(|| parse_positive_f64(video.duration.as_deref()))
        .context("Could not determine video duration")?;

    let fps = parse_frame_rate(video.avg_frame_rate.as_deref())
        .or_else(|| parse_frame_rate(video.r_frame_rate.as_deref()))
        .unwrap_or(30.0);

    let has_audio = streams
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("audio"));

    Ok(VideoProbeInfo {
        duration_sec,
        width,
        height,
        fps,
        has_audio,
    })
}

pub async fn probe_video(path: &Path, bundled_dir: Option<&Path>) -> Result<VideoProbeInfo> {
    ensure!(path.is_file(), "Video file does not exist");
    let mut cmd = new_media_command("ffprobe", bundled_dir);
    cmd.args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = cmd.spawn().map_err(|err| map_spawn_error("ffprobe", err))?;
    let output = timeout(Duration::from_secs(PROBE_TIMEOUT_SECS), child.wait_with_output())
        .await
        .map_err(|_| anyhow!("Timed out probing video metadata with ffprobe"))?
        .context("Failed to read ffprobe output")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("ffprobe exited with status {}", output.status);
        }
        bail!("ffprobe failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout).context("ffprobe output was not valid UTF-8")?;
    parse_ffprobe_json(&stdout)
}

async fn extract_single_thumbnail(
    path: &Path,
    timestamp_sec: f64,
    bundled_dir: Option<&Path>,
) -> Result<Vec<u8>> {
    let mut cmd = new_media_command("ffmpeg", bundled_dir);
    cmd.args(["-v", "error", "-ss", &format!("{timestamp_sec:.3}"), "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=-2:72",
            "-q:v",
            "6",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|err| map_spawn_error("ffmpeg", err))?;
    let stdout = child.stdout.take().context("Missing ffmpeg stdout")?;
    let stderr = child.stderr.take();

    let mut bytes = Vec::with_capacity(8192);
    let mut limited = stdout.take((MAX_THUMBNAIL_BYTES + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .await
        .context("Failed to read thumbnail bytes from ffmpeg")?;
    if bytes.len() > MAX_THUMBNAIL_BYTES {
        let _ = child.kill().await;
        let _ = child.wait().await;
        bail!("Thumbnail frame exceeded {} bytes", MAX_THUMBNAIL_BYTES);
    }

    let status = child.wait().await.context("Failed waiting for ffmpeg thumbnail")?;
    if !status.success() {
        let mut err_buf = Vec::new();
        if let Some(stderr) = stderr {
            let _ = stderr.take(4096).read_to_end(&mut err_buf).await;
        }
        let msg = String::from_utf8_lossy(&err_buf).trim().to_string();
        if msg.is_empty() {
            bail!("ffmpeg thumbnail extraction exited with {status}");
        }
        bail!("ffmpeg thumbnail extraction failed: {msg}");
    }
    Ok(bytes)
}

#[cfg(windows)]
pub async fn extract_video_cover(path: &Path, bundled_dir: Option<&Path>) -> Result<Vec<u8>> {
    let mut cmd = new_media_command("ffmpeg", bundled_dir);
    cmd.args([
        "-nostdin", "-v", "error", "-max_alloc", "134217728",
        "-protocol_whitelist", "file", "-format_whitelist", "mov,matroska,webm,avi",
        "-max_streams", "32", "-probesize", "8388608", "-analyzeduration", "5000000",
        "-threads", "2", "-max_pixels", "8294400", "-i",
    ])
    .arg(path)
    .args([
        "-map", "0:V:0", "-an", "-sn", "-dn", "-map_metadata", "-1", "-filter_threads", "1",
        "-vf", "blackframe=amount=0:threshold=24,metadata=mode=select:key=lavfi.blackframe.pblack:function=less:value=98,scale=w='min(iw,640)':h='min(ih,640)':force_original_aspect_ratio=decrease:force_divisible_by=2",
        "-frames:v", "1", "-q:v", "4", "-threads", "2", "-f", "image2pipe", "-vcodec", "mjpeg", "pipe:1",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    timeout(Duration::from_secs(THUMBNAIL_TIMEOUT_SECS), async {
        let mut child = cmd.spawn().map_err(|err| map_spawn_error("ffmpeg", err))?;
        let stdout = child.stdout.take().context("Missing ffmpeg stdout")?;
        let mut bytes = Vec::new();
        stdout.take((MAX_THUMBNAIL_BYTES + 1) as u64).read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= MAX_THUMBNAIL_BYTES, "Video cover exceeded the size limit");
        ensure!(child.wait().await?.success(), "Video cover generation failed");
        Ok(bytes)
    })
    .await
    .context("Timed out generating video cover")?
}

pub async fn extract_thumbnails(
    path: &Path,
    count: usize,
    bundled_dir: Option<&Path>,
) -> Result<Vec<Vec<u8>>> {
    let info = probe_video(path, bundled_dir).await?;
    let target_count = count.clamp(8, 12);

    timeout(Duration::from_secs(THUMBNAIL_TIMEOUT_SECS), async {
        let mut frames = Vec::with_capacity(target_count);
        for index in 0..target_count {
            let timestamp = info.duration_sec * (index as f64 + 0.5) / (target_count as f64);
            let frame = extract_single_thumbnail(path, timestamp, bundled_dir).await?;
            if !frame.is_empty() {
                frames.push(frame);
            }
        }
        if frames.is_empty() {
            let fallback = extract_single_thumbnail(path, 0.0, bundled_dir).await?;
            if !fallback.is_empty() {
                frames.push(fallback);
            }
        }
        ensure!(!frames.is_empty(), "No video thumbnails could be extracted");
        Ok(frames)
    })
    .await
    .map_err(|_| anyhow!("Timed out extracting video thumbnails"))?
}

pub fn validate_and_clamp_trim(
    start_sec: f64,
    end_sec: f64,
    duration_sec: f64,
) -> Result<(f64, f64)> {
    ensure!(
        duration_sec.is_finite() && duration_sec >= MIN_TRIM_DURATION_SEC - 1e-6,
        "Video duration is too short to trim"
    );
    ensure!(
        start_sec.is_finite() && end_sec.is_finite(),
        "Trim start and end times must be finite numbers"
    );
    ensure!(start_sec >= 0.0, "Trim start time must not be negative");
    ensure!(
        end_sec <= duration_sec + DURATION_TOLERANCE_SEC,
        "Trim end time exceeds video duration"
    );
    let clamped_end = end_sec.min(duration_sec);
    ensure!(clamped_end > start_sec, "Trim end time must be after start time");
    ensure!(
        clamped_end - start_sec >= MIN_TRIM_DURATION_SEC - 1e-6,
        "Trimmed clip must be at least 0.2 seconds long"
    );
    Ok((start_sec, clamped_end))
}

pub fn parse_progress_line(line: &str, trim_duration_sec: f64) -> Option<f64> {
    if !trim_duration_sec.is_finite() || trim_duration_sec <= 0.0 {
        return None;
    }
    let trimmed = line.trim();
    if trimmed == "progress=end" {
        return Some(1.0);
    }
    // FFmpeg `-progress pipe:1` emits microseconds for both `out_time_us` and `out_time_ms`.
    if let Some(raw) = trimmed
        .strip_prefix("out_time_us=")
        .or_else(|| trimmed.strip_prefix("out_time_ms="))
    {
        let micros: i64 = raw.trim().parse().ok()?;
        if micros < 0 {
            return Some(0.0);
        }
        let elapsed_sec = (micros as f64) / 1_000_000.0;
        return Some((elapsed_sec / trim_duration_sec).clamp(0.0, 1.0));
    }
    None
}

pub fn cleanup_stale_trim_files(media_root: &Path) {
    let tmp_dir = media_root.join("tmp");
    let Ok(entries) = std::fs::read_dir(&tmp_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with(TRIM_TEMP_PREFIX) || !name_str.ends_with(TRIM_TEMP_SUFFIX) {
            continue;
        }
        let path = entry.path();
        if let Ok(meta) = std::fs::symlink_metadata(&path)
            && meta.is_file()
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct ExportEntry {
    attachment_id: Uuid,
    progress: VideoExportProgress,
    output_path: Option<PathBuf>,
    _temp_guard: Option<zibblu_core::TempFileGuard>,
    cancelled: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
    task: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub struct VideoExportRegistry {
    entries: Arc<StdMutex<HashMap<Uuid, ExportEntry>>>,
    bundled_dir: StdMutex<Option<PathBuf>>,
}

impl VideoExportRegistry {
    pub fn set_bundled_ffmpeg_dir(&self, dir: PathBuf) {
        *self.bundled_dir.lock().expect("bundled ffmpeg dir") = Some(dir);
    }

    pub fn bundled_ffmpeg_dir(&self) -> Option<PathBuf> {
        self.bundled_dir.lock().expect("bundled ffmpeg dir").clone()
    }

    pub async fn start_trim_export(
        &self,
        attachment_id: Uuid,
        source_path: &Path,
        media_root: &Path,
        client: Option<&zibblu_core::Client>,
        start_sec: f64,
        end_sec: f64,
        duration_sec: f64,
    ) -> Result<VideoExportProgress> {
        ensure!(source_path.is_file(), "Source video file does not exist");
        let (start_sec, end_sec) = validate_and_clamp_trim(start_sec, end_sec, duration_sec)?;
        let trim_len = (end_sec - start_sec).max(MIN_TRIM_DURATION_SEC);

        let tmp_dir = media_root.join("tmp");
        tokio::fs::create_dir_all(&tmp_dir)
            .await
            .context("Could not prepare temporary export directory")?;

        let export_id = Uuid::new_v4();
        let temp_guard = client.map(|c| c.track_temp_prefix(format!("{TRIM_TEMP_PREFIX}{export_id}")));
        let output_path = tmp_dir.join(format!("{TRIM_TEMP_PREFIX}{export_id}{TRIM_TEMP_SUFFIX}"));

        let bundled = self.bundled_ffmpeg_dir();
        let mut cmd = new_media_command("ffmpeg", bundled.as_deref());
        cmd.args(["-y", "-ss", &format!("{start_sec:.6}"), "-i"])
            .arg(source_path)
            .args([
                "-t",
                &format!("{trim_len:.6}"),
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "20",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-nostats",
            ])
            .arg(&output_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|err| map_spawn_error("ffmpeg", err))?;

        let initial = VideoExportProgress {
            export_id,
            status: "running".into(),
            progress: 0.0,
            output_size: None,
            error: None,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_notify = Arc::new(Notify::new());

        {
            let mut map = self.entries.lock().expect("video export registry");
            map.insert(
                export_id,
                ExportEntry {
                    attachment_id,
                    progress: initial.clone(),
                    output_path: Some(output_path.clone()),
                    _temp_guard: temp_guard,
                    cancelled: Arc::clone(&cancelled),
                    cancel_notify: Arc::clone(&cancel_notify),
                    task: None,
                },
            );
        }

        let entries = Arc::clone(&self.entries);
        let cancelled_flag = Arc::clone(&cancelled);
        let notify = Arc::clone(&cancel_notify);
        let out_path = output_path.clone();

        let task = tokio::spawn(async move {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let stderr_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(stderr) = stderr {
                    let _ = stderr.take(16 * 1024).read_to_end(&mut buf).await;
                }
                String::from_utf8_lossy(&buf).trim().to_string()
            });

            let progress_entries = Arc::clone(&entries);
            let progress_task = tokio::spawn(async move {
                if let Some(stdout) = stdout {
                    let mut lines = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if let Some(pct) = parse_progress_line(&line, trim_len) {
                            let mut map = progress_entries.lock().expect("video export registry");
                            if let Some(entry) = map.get_mut(&export_id)
                                && entry.progress.status == "running"
                            {
                                entry.progress.progress = pct.min(0.99);
                            }
                        }
                    }
                }
            });

            let was_cancelled = tokio::select! {
                biased;
                _ = notify.notified() => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    progress_task.abort();
                    stderr_task.abort();
                    true
                }
                wait_res = child.wait() => {
                    if cancelled_flag.load(Ordering::Relaxed) {
                        progress_task.abort();
                        stderr_task.abort();
                        true
                    } else {
                        let _ = progress_task.await;
                        let stderr_msg = stderr_task.await.unwrap_or_default();
                        match wait_res {
                            Ok(status) if status.success() => {
                                match tokio::fs::metadata(&out_path).await {
                                    Ok(meta) if meta.len() > 0 => {
                                        let mut map = entries.lock().expect("video export registry");
                                        if let Some(entry) = map.get_mut(&export_id)
                                            && !cancelled_flag.load(Ordering::Relaxed)
                                        {
                                            entry.progress.status = "complete".into();
                                            entry.progress.progress = 1.0;
                                            entry.progress.output_size = Some(meta.len());
                                            entry.progress.error = None;
                                        }
                                    }
                                    _ => {
                                        let _ = tokio::fs::remove_file(&out_path).await;
                                        let mut map = entries.lock().expect("video export registry");
                                        if let Some(entry) = map.get_mut(&export_id) {
                                            entry.progress.status = "failed".into();
                                            entry.progress.error = Some("Export produced an empty file".into());
                                            entry.output_path = None;
                                            entry._temp_guard = None;
                                        }
                                    }
                                }
                            }
                            Ok(status) => {
                                let _ = tokio::fs::remove_file(&out_path).await;
                                let msg = if stderr_msg.is_empty() {
                                    format!("ffmpeg exited with status {status}")
                                } else {
                                    stderr_msg
                                };
                                let mut map = entries.lock().expect("video export registry");
                                if let Some(entry) = map.get_mut(&export_id) {
                                    entry.progress.status = "failed".into();
                                    entry.progress.error = Some(msg);
                                    entry.output_path = None;
                                    entry._temp_guard = None;
                                }
                            }
                            Err(err) => {
                                let _ = tokio::fs::remove_file(&out_path).await;
                                let mut map = entries.lock().expect("video export registry");
                                if let Some(entry) = map.get_mut(&export_id) {
                                    entry.progress.status = "failed".into();
                                    entry.progress.error = Some(format!("ffmpeg wait error: {err}"));
                                    entry.output_path = None;
                                    entry._temp_guard = None;
                                }
                            }
                        }
                        false
                    }
                }
            };

            if was_cancelled {
                let _ = tokio::fs::remove_file(&out_path).await;
                let mut map = entries.lock().expect("video export registry");
                if let Some(entry) = map.get_mut(&export_id) {
                    entry.progress.status = "cancelled".into();
                    entry.progress.error = None;
                    entry.output_path = None;
                    entry._temp_guard = None;
                }
            }
        });

        {
            let mut map = self.entries.lock().expect("video export registry");
            if let Some(entry) = map.get_mut(&export_id) {
                entry.task = Some(task);
            }
        }

        Ok(initial)
    }

    pub fn status(&self, export_id: Uuid) -> Option<VideoExportProgress> {
        let map = self.entries.lock().expect("video export registry");
        map.get(&export_id).map(|entry| entry.progress.clone())
    }

    pub async fn cancel(&self, export_id: Uuid) -> Option<VideoExportProgress> {
        let (task, notify) = {
            let mut map = self.entries.lock().expect("video export registry");
            let entry = map.get_mut(&export_id)?;
            if entry.progress.status != "running" {
                return Some(entry.progress.clone());
            }
            entry.cancelled.store(true, Ordering::Relaxed);
            (entry.task.take(), Arc::clone(&entry.cancel_notify))
        };
        notify.notify_waiters();
        if let Some(task) = task {
            let _ = task.await;
        }
        self.status(export_id)
    }

    pub async fn cancel_for_attachment(&self, attachment_id: Uuid) {
        let ids: Vec<Uuid> = {
            let map = self.entries.lock().expect("video export registry");
            map.iter()
                .filter(|(_, entry)| {
                    entry.attachment_id == attachment_id && entry.progress.status == "running"
                })
                .map(|(id, _)| *id)
                .collect()
        };
        for id in ids {
            let _ = self.cancel(id).await;
        }
    }

    pub async fn cancel_all(&self) {
        let ids: Vec<Uuid> = {
            let map = self.entries.lock().expect("video export registry");
            map.iter()
                .filter(|(_, entry)| entry.progress.status == "running")
                .map(|(id, _)| *id)
                .collect()
        };
        for id in ids {
            let _ = self.cancel(id).await;
        }
    }

    pub fn take_completed_path(&self, export_id: Uuid) -> Option<PathBuf> {
        let mut map = self.entries.lock().expect("video export registry");
        let entry = map.get_mut(&export_id)?;
        if entry.progress.status == "complete" {
            entry.output_path.take()
        } else {
            None
        }
    }

    pub async fn cleanup_entry(&self, export_id: Uuid) {
        let _ = self.cancel(export_id).await;
        let path = {
            let mut map = self.entries.lock().expect("video export registry");
            map.remove(&export_id).and_then(|entry| entry.output_path)
        };
        if let Some(path) = path {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_stale_trim_files, parse_ffprobe_json, parse_progress_line, validate_and_clamp_trim,
    };
    use uuid::Uuid;

    #[test]
    fn validates_and_clamps_trim_parameters() {
        assert!(validate_and_clamp_trim(-0.1, 2.0, 5.0).is_err());
        assert!(validate_and_clamp_trim(2.0, 2.0, 5.0).is_err());
        assert!(validate_and_clamp_trim(2.5, 2.0, 5.0).is_err());
        assert!(validate_and_clamp_trim(1.0, 1.19, 5.0).is_err());
        assert!(validate_and_clamp_trim(1.0, 5.1, 5.0).is_err());
        assert!(validate_and_clamp_trim(f64::NAN, 2.0, 5.0).is_err());

        let (s, e) = validate_and_clamp_trim(1.0, 5.04, 5.0).unwrap();
        assert_eq!((s, e), (1.0, 5.0));

        // Floating-point edge: 10.2 - 10.0 = 0.1999999999999993 in IEEE 754.
        let (s2, e2) = validate_and_clamp_trim(10.0, 10.2, 10.2).unwrap();
        assert_eq!((s2, e2), (10.0, 10.2));
    }

    #[test]
    fn parses_ffmpeg_progress_lines() {
        assert_eq!(parse_progress_line("out_time_us=5000000", 10.0), Some(0.5));
        assert_eq!(parse_progress_line("out_time_ms=2500000", 10.0), Some(0.25));
        assert_eq!(parse_progress_line("out_time_us=12000000", 10.0), Some(1.0));
        assert_eq!(parse_progress_line("out_time_us=N/A", 10.0), None);
        assert_eq!(parse_progress_line("progress=end", 10.0), Some(1.0));
        assert_eq!(parse_progress_line("frame=42", 10.0), None);
    }

    #[test]
    fn parses_ffprobe_json_streams_and_format() {
        let json = r#"{
            "streams": [
                {
                    "codec_type": "video",
                    "width": 1920,
                    "height": 1080,
                    "r_frame_rate": "30000/1001",
                    "avg_frame_rate": "30000/1001"
                },
                {
                    "codec_type": "audio"
                }
            ],
            "format": {
                "duration": "12.500000"
            }
        }"#;
        let info = parse_ffprobe_json(json).unwrap();
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert!((info.duration_sec - 12.5).abs() < 1e-6);
        assert!((info.fps - (30000.0 / 1001.0)).abs() < 1e-4);
        assert!(info.has_audio);

        let no_audio_fallback = r#"{
            "streams": [
                {
                    "codec_type": "video",
                    "width": 640,
                    "height": 360,
                    "r_frame_rate": "24/1",
                    "avg_frame_rate": "0/0",
                    "duration": "4.25"
                }
            ]
        }"#;
        let info2 = parse_ffprobe_json(no_audio_fallback).unwrap();
        assert!(!info2.has_audio);
        assert_eq!(info2.fps, 24.0);
        assert!((info2.duration_sec - 4.25).abs() < 1e-6);

        assert!(parse_ffprobe_json(r#"{"streams": []}"#).is_err());
    }

    #[test]
    fn cleans_only_stale_trim_files() {
        let root = std::env::temp_dir().join(format!("zibblu-trim-test-{}", Uuid::new_v4()));
        let tmp = root.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();

        let trim_a = tmp.join(format!("trim-{}.mp4", Uuid::new_v4()));
        let trim_b = tmp.join("trim-stale.mp4");
        let keep_transfer = tmp.join("vorig-123.part");

        std::fs::write(&trim_a, b"partial").unwrap();
        std::fs::write(&trim_b, b"partial").unwrap();
        std::fs::write(&keep_transfer, b"active-download").unwrap();

        cleanup_stale_trim_files(&root);

        assert!(!trim_a.exists());
        assert!(!trim_b.exists());
        assert!(keep_transfer.is_file());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn bundled_ffmpeg_probes_extracts_thumbnails_and_exports_trimmed_mp4_without_path() {
        use super::{
            VideoExportRegistry, extract_thumbnails, probe_video, resolve_media_binary,
        };
        use sha2::{Digest, Sha256};

        let staged_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("bin")
            .join("ffmpeg");
        let ffmpeg_bin = resolve_media_binary("ffmpeg", Some(&staged_dir));
        let ffprobe_bin = resolve_media_binary("ffprobe", Some(&staged_dir));
        if cfg!(windows) && (!staged_dir.join("ffmpeg.exe").is_file() || !staged_dir.join("ffprobe.exe").is_file()) {
            // Skip only if CI has not staged Windows FFmpeg runtime before running unit tests.
            return;
        }
        assert!(ffmpeg_bin.is_absolute() && ffmpeg_bin.is_file());
        assert!(ffprobe_bin.is_absolute() && ffprobe_bin.is_file());

        let root = std::env::temp_dir().join(format!("zibblu-trim-e2e-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let fixture = root.join("source.mp4");

        let status = super::new_media_command("ffmpeg", Some(&staged_dir))
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=25",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "3",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
            ])
            .arg(&fixture)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "failed to generate test mp4 fixture");

        let orig_bytes = std::fs::read(&fixture).unwrap();
        let orig_sha = Sha256::digest(&orig_bytes);

        let info = probe_video(&fixture, Some(&staged_dir)).await.unwrap();
        assert_eq!((info.width, info.height), (320, 240));
        assert!(info.has_audio);
        assert!((2.7..=3.3).contains(&info.duration_sec));

        let thumbs = extract_thumbnails(&fixture, 8, Some(&staged_dir))
            .await
            .unwrap();
        assert_eq!(thumbs.len(), 8);
        assert!(thumbs.iter().all(|f| !f.is_empty()));

        let registry = VideoExportRegistry::default();
        registry.set_bundled_ffmpeg_dir(staged_dir.clone());
        let initial = registry
            .start_trim_export(
                Uuid::new_v4(),
                &fixture,
                &root,
                None,
                0.5,
                2.0,
                info.duration_sec,
            )
            .await
            .unwrap();
        let mut final_progress = initial.clone();
        for _ in 0..80 {
            if let Some(p) = registry.status(initial.export_id) {
                final_progress = p;
                if final_progress.status != "running" {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(
            final_progress.status,
            "complete",
            "export failed: {:?}",
            final_progress.error
        );
        let exported = registry.take_completed_path(initial.export_id).unwrap();
        let exported_info = probe_video(&exported, Some(&staged_dir)).await.unwrap();
        assert!(exported_info.has_audio);
        assert!((1.2..=1.8).contains(&exported_info.duration_sec));
        assert_eq!(
            Sha256::digest(std::fs::read(&fixture).unwrap()),
            orig_sha,
            "original video fixture must remain unchanged"
        );
        registry.cleanup_entry(initial.export_id).await;
        let _ = std::fs::remove_dir_all(&root);
    }
}
