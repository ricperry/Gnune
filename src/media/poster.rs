//! Serial, bounded local artwork discovery. Call from a background worker, never a GTK callback.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, UNIX_EPOCH},
};

/// A cheap UI lookup: cache versioning retires the old unconditional first-frame thumbnails.
pub fn poster_path(path: &Path, dir: &Path) -> PathBuf {
    dir.join(format!(
        "v2-{}.jpg",
        crate::enrichment::key(&[&path.to_string_lossy()])
    ))
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Fingerprint {
    size: u64,
    modified_ns: u128,
}

fn fingerprint(path: &Path) -> Result<Fingerprint, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a media file", path.display()));
    }
    Ok(Fingerprint {
        size: metadata.len(),
        modified_ns: metadata
            .modified()
            .map_err(|e| e.to_string())?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    })
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Cache {
    media: Fingerprint,
    artwork: Vec<(PathBuf, Fingerprint)>,
    ready: bool,
}

fn sidecars(path: &Path) -> Vec<PathBuf> {
    let Some(stem) = path.file_stem() else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    // Deliberately per-video: a shared folder cover can describe an entirely different clip.
    for suffix in ["-poster", ""] {
        for extension in ["jpg", "jpeg", "png", "webp", "JPG", "JPEG", "PNG", "WEBP"] {
            let mut name = stem.to_os_string();
            name.push(format!("{suffix}.{extension}"));
            candidates.push(path.with_file_name(name));
        }
    }
    candidates
}

/// Validate a cached poster, discover local artwork, or sample the video.
/// Return whether the cached image appeared, changed, or disappeared. I/O and missing tools
/// remain retryable errors; an unchanged unavailable image returns `Ok(false)`.
/// A single caller should process its queue serially. Each child uses one CPU thread and a timeout.
pub fn prepare_poster(path: &Path, dir: &Path) -> Result<bool, String> {
    // An unavailable removable mount or unreadable source must not become a permanent miss.
    fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut cache = Cache {
        media: fingerprint(path)?,
        artwork: Vec::new(),
        ready: false,
    };
    for candidate in sidecars(path) {
        match fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => {
                fs::File::open(&candidate).map_err(|e| format!("{}: {e}", candidate.display()))?;
                cache
                    .artwork
                    .push((candidate.clone(), fingerprint(&candidate)?));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", candidate.display())),
        }
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let destination = poster_path(path, dir);
    let record = destination.with_extension("json");
    if let Some(previous) = fs::read(&record)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        && previous.media == cache.media
        && previous.artwork == cache.artwork
    {
        if !previous.ready {
            return Ok(false);
        }
        if fs::metadata(&destination).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Ok(false);
        }
    }

    let had_image = destination.is_file();
    let mut selected = None;
    for (artwork, _) in &cache.artwork {
        if let Some(frame) = frame(artwork, None, None)?
            && frame_score(&frame).is_some()
        {
            selected = Some(frame);
            break;
        }
    }
    if selected.is_none() {
        let info = probe(path)?;
        if let Some(index) = info.as_ref().and_then(|info| info.attached_picture)
            && let Some(frame) = frame(path, None, Some(index))?
            && frame_score(&frame).is_some()
        {
            selected = Some(frame);
        }
        if let Some(info) = info
            && selected.is_none()
            && info.video.is_some()
        {
            for position in sample_positions(info.duration) {
                if let Some(frame) = frame(path, Some(position), info.video)?
                    && frame_score(&frame).is_some()
                {
                    selected = Some(frame);
                    break;
                }
            }
        }
    }
    if let Some(frame) = selected {
        save_frame(&frame, &destination, dir)?;
        cache.ready = true;
    } else if had_image {
        fs::remove_file(&destination).map_err(|e| e.to_string())?;
    }
    // Also cache uniformly black/white, corrupt, and unsupported clips; no repeated startup work.
    let mut temporary = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    serde_json::to_writer(temporary.as_file_mut(), &cache).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(record).map_err(|e| e.to_string())?;
    Ok(cache.ready || had_image)
}

struct VideoInfo {
    duration: f64,
    video: Option<u64>,
    attached_picture: Option<u64>,
}

fn probe(path: &Path) -> Result<Option<VideoInfo>, String> {
    let mut command = Command::new("ffprobe");
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-of",
            "json",
            "-show_entries",
            "format=duration:stream=index,codec_type,duration:stream_disposition=attached_pic",
            "-i",
        ])
        .arg(path);
    let Some(output) = run(&mut command)? else {
        return Ok(None);
    };
    let document: serde_json::Value = serde_json::from_slice(&output).map_err(|e| e.to_string())?;
    let streams = document["streams"].as_array();
    let videos = || {
        streams
            .into_iter()
            .flatten()
            .filter(|s| s["codec_type"] == "video")
    };
    let attached = |s: &serde_json::Value| s["disposition"]["attached_pic"].as_u64() == Some(1);
    let video = videos().find(|s| !attached(s));
    let seconds = |value: &serde_json::Value| {
        value
            .as_f64()
            .or_else(|| value.as_str()?.parse().ok())
            .filter(|n: &f64| n.is_finite() && *n > 0.)
    };
    Ok(Some(VideoInfo {
        duration: seconds(&document["format"]["duration"])
            .or_else(|| video.and_then(|s| seconds(&s["duration"])))
            .unwrap_or(0.),
        video: video.and_then(|s| s["index"].as_u64()),
        attached_picture: videos()
            .find(|s| attached(s))
            .and_then(|s| s["index"].as_u64()),
    }))
}

fn sample_positions(duration: f64) -> Vec<f64> {
    if duration.is_finite() && duration > 0. {
        // Include near-end and first-frame fallbacks, and never seek past a short clip.
        [0.2, 0.4, 0.6, 0.8, 0.95, 0.05, 0.99, 0.0]
            .map(|fraction| (duration * fraction).min((duration - 0.05).max(0.)))
            .into_iter()
            .fold(Vec::new(), |mut positions, position| {
                if !positions
                    .iter()
                    .any(|p: &f64| (*p - position).abs() < 0.001)
                {
                    positions.push(position);
                }
                positions
            })
    } else {
        vec![1., 5., 15., 30., 60., 0.]
    }
}

fn ffmpeg() -> Command {
    let mut command = Command::new("ffmpeg");
    command.args([
        "-nostdin",
        "-loglevel",
        "error",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-protocol_whitelist",
        "file,pipe",
    ]);
    command
}

fn frame(
    path: &Path,
    position: Option<f64>,
    stream: Option<u64>,
) -> Result<Option<Vec<u8>>, String> {
    let mut command = ffmpeg();
    if let Some(position) = position {
        command.args(["-ss", &format!("{position:.6}")]);
    }
    command.arg("-i").arg(path);
    if let Some(index) = stream {
        command.args(["-map", &format!("0:{index}")]);
    }
    command.args([
        "-frames:v",
        "1",
        "-an",
        "-sn",
        "-vf",
        "scale=384:384:force_original_aspect_ratio=decrease",
        "-threads",
        "1",
        "-pix_fmt",
        "rgb24",
        "-c:v",
        "ppm",
        "-f",
        "image2pipe",
        "pipe:1",
    ]);
    run(&mut command)
}

/// Run one local process at a time, avoiding hung decoders and unbounded pipe buffers.
fn run(command: &mut Command) -> Result<Option<Vec<u8>>, String> {
    let output = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut child = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(output.try_clone().map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| {
            format!(
                "Could not run {}: {e}",
                command.get_program().to_string_lossy()
            )
        })?;
    let started = Instant::now();
    let success = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status.success();
        }
        if started.elapsed() >= Duration::from_secs(12) {
            let _ = child.kill();
            let _ = child.wait();
            // A timeout may be transient (slow removable media), so do not negative-cache it.
            return Err("Local thumbnail decoding timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !success {
        return Ok(None);
    }
    use std::io::{Seek, SeekFrom};
    let mut output = output;
    output.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    output
        .take(2 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(Some(bytes))
}

fn pixels(ppm: &[u8]) -> Option<(usize, &[u8])> {
    let mut offset = 0;
    let mut token = || {
        while ppm.get(offset).is_some_and(u8::is_ascii_whitespace) {
            offset += 1;
        }
        let start = offset;
        while ppm.get(offset).is_some_and(|v| !v.is_ascii_whitespace()) {
            offset += 1;
        }
        std::str::from_utf8(&ppm[start..offset]).ok()
    };
    if token()? != "P6" {
        return None;
    }
    let width = token()?.parse::<usize>().ok()?;
    let height = token()?.parse::<usize>().ok()?;
    if token()? != "255" || width == 0 || height == 0 {
        return None;
    }
    let data = ppm.get(offset + 1..)?;
    (data.len() == width.checked_mul(height)?.checked_mul(3)?).then_some((width, data))
}

fn frame_score(ppm: &[u8]) -> Option<f64> {
    let (width, data) = pixels(ppm)?;
    let mut sum = 0.;
    let mut squares = 0.;
    let mut extremes = 0;
    let mut edges = 0;
    let count = data.len() / 3;
    for (index, rgb) in data.chunks_exact(3).enumerate() {
        let luma = (rgb[0] as f64 * 0.2126) + (rgb[1] as f64 * 0.7152) + (rgb[2] as f64 * 0.0722);
        sum += luma;
        squares += luma * luma;
        extremes += usize::from(!(9. ..=246.).contains(&luma));
        if index % width > 0 {
            let previous = &data[(index - 1) * 3..index * 3];
            let difference: u32 = rgb
                .iter()
                .zip(previous)
                .map(|(a, b)| a.abs_diff(*b) as u32)
                .sum();
            edges += usize::from(difference > 24);
        }
    }
    let count = count as f64;
    let variance = squares / count - (sum / count).powi(2);
    (variance >= 30. && extremes as f64 / count < 0.97 && edges as f64 / count > 0.015)
        .then_some(variance.sqrt())
}

fn save_frame(ppm: &[u8], destination: &Path, dir: &Path) -> Result<(), String> {
    let mut source = tempfile::Builder::new()
        .suffix(".ppm")
        .tempfile_in(dir)
        .map_err(|e| e.to_string())?;
    source.write_all(ppm).map_err(|e| e.to_string())?;
    let temporary = tempfile::Builder::new()
        .suffix(".jpg")
        .tempfile_in(dir)
        .map_err(|e| e.to_string())?;
    let mut command = ffmpeg();
    command
        .arg("-y")
        .arg("-i")
        .arg(source.path())
        .args(["-frames:v", "1", "-threads", "1", "-q:v", "3"])
        .arg(temporary.path());
    if run(&mut command)?.is_none()
        || temporary
            .as_file()
            .metadata()
            .map_err(|e| e.to_string())?
            .len()
            == 0
    {
        return Err("Could not save the video thumbnail".into());
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(destination).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools_available() -> bool {
        ["ffmpeg", "ffprobe"].into_iter().all(|tool| {
            Command::new(tool)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }

    fn fixture(path: &Path, source: &str) {
        let status = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                source,
                "-threads",
                "1",
            ])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn usable(path: &Path) -> bool {
        frame_score(&frame(path, None, None).unwrap().unwrap()).is_some()
    }

    #[test]
    fn rejects_black_white_and_flat_color_images() {
        for color in [[0, 0, 0], [255, 255, 255], [40, 80, 130]] {
            let mut ppm = b"P6\n64 36\n255\n".to_vec();
            ppm.extend(color.repeat(64 * 36));
            assert!(frame_score(&ppm).is_none());
        }
        assert!(frame_score(b"P6\n999 999\n255\n").is_none());
        assert!(frame_score(b"broken").is_none());
    }

    #[test]
    fn sampling_handles_short_and_unknown_duration_and_reaches_late_content() {
        assert_eq!(sample_positions(0.04), vec![0.]);
        assert!(
            sample_positions(0.25)
                .iter()
                .all(|position| *position < 0.25)
        );
        assert!(sample_positions(100.).contains(&99.));
        assert_eq!(sample_positions(f64::NAN).last(), Some(&0.));
    }

    #[test]
    fn finds_late_content_and_subsecond_video_without_adopting_legacy_cache() {
        if !tools_available() {
            eprintln!("ffmpeg/ffprobe unavailable; skipping decoder fixtures");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let late = dir.path().join("late.mp4");
        fixture(
            &late,
            "color=black:s=160x90:r=10:d=9.4[black];testsrc2=s=160x90:r=10:d=0.6[content];[black][content]concat=n=2:v=1:a=0",
        );
        fs::create_dir(&cache).unwrap();
        let legacy = cache.join(format!(
            "{}.jpg",
            crate::enrichment::key(&[&late.to_string_lossy()])
        ));
        fs::write(&legacy, b"old black thumbnail").unwrap();
        assert!(prepare_poster(&late, &cache).unwrap());
        assert!(usable(&poster_path(&late, &cache)));
        assert_eq!(fs::read(legacy).unwrap(), b"old black thumbnail");
        assert!(!prepare_poster(&late, &cache).unwrap());

        let short = dir.path().join("short.mp4");
        fixture(&short, "testsrc2=s=160x90:r=25:d=0.04");
        assert!(prepare_poster(&short, &cache).unwrap());
        assert!(usable(&poster_path(&short, &cache)));
    }

    #[test]
    fn caches_unusable_results_and_invalidates_changed_media() {
        if !tools_available() {
            eprintln!("ffmpeg/ffprobe unavailable; skipping decoder fixtures");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let video = dir.path().join("video.mp4");
        fixture(&video, "color=white:s=160x90:r=10:d=0.2");
        assert!(!prepare_poster(&video, &cache).unwrap());
        assert!(!poster_path(&video, &cache).exists());
        let record = poster_path(&video, &cache).with_extension("json");
        let modified = fs::metadata(&record).unwrap().modified().unwrap();
        assert!(!prepare_poster(&video, &cache).unwrap());
        assert_eq!(fs::metadata(&record).unwrap().modified().unwrap(), modified);
        fixture(&video, "testsrc2=s=160x90:r=10:d=0.2");
        assert!(prepare_poster(&video, &cache).unwrap());
        assert!(usable(&poster_path(&video, &cache)));
        fixture(&video, "color=black:s=160x90:r=10:d=0.2");
        assert!(prepare_poster(&video, &cache).unwrap());
        assert!(!poster_path(&video, &cache).exists());
        assert!(!prepare_poster(&video, &cache).unwrap());
        fs::remove_file(&video).unwrap();
        assert!(prepare_poster(&video, &cache).is_err());
        fs::write(&video, b"invalid video").unwrap();
        assert!(!prepare_poster(&video, &cache).unwrap());
        assert!(!prepare_poster(&video, &cache).unwrap());
    }

    #[test]
    fn per_video_sidecar_is_preferred_and_its_changes_invalidate_cache() {
        if !tools_available() {
            eprintln!("ffmpeg/ffprobe unavailable; skipping decoder fixtures");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let video = dir.path().join("video.mp4");
        let artwork = dir.path().join("video-poster.jpg");
        fixture(&video, "color=black:s=160x90:r=10:d=0.2");
        fixture(&artwork, "testsrc2=s=100x100:r=1:d=1");
        assert!(prepare_poster(&video, &cache).unwrap());
        let output = frame(&poster_path(&video, &cache), None, None)
            .unwrap()
            .unwrap();
        let (width, data) = pixels(&output).unwrap();
        assert_eq!(
            width,
            data.len() / 3 / width,
            "sidecar kept its square aspect ratio"
        );
        fixture(&artwork, "color=black:s=100x100:r=1:d=1");
        assert!(prepare_poster(&video, &cache).unwrap());
        assert!(!poster_path(&video, &cache).exists());
        fixture(&artwork, "testsrc2=s=100x100:r=1:d=1");
        assert!(prepare_poster(&video, &cache).unwrap());
        fs::remove_file(artwork).unwrap();
        assert!(prepare_poster(&video, &cache).unwrap());
        assert!(!poster_path(&video, &cache).exists());
    }

    #[test]
    fn attached_artwork_wins_and_black_attachment_falls_back_to_video() {
        if !tools_available() {
            eprintln!("ffmpeg/ffprobe unavailable; skipping decoder fixtures");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let base = dir.path().join("base.mp4");
        let artwork = dir.path().join("artwork.jpg");
        let embedded = dir.path().join("embedded.m4v");
        for (source, art) in [
            (
                "color=black:s=160x90:r=10:d=0.2",
                "testsrc2=s=100x100:r=1:d=1",
            ),
            (
                "testsrc2=s=160x90:r=10:d=0.2",
                "color=black:s=100x100:r=1:d=1",
            ),
        ] {
            fixture(&base, source);
            fixture(&artwork, art);
            assert!(
                Command::new("ffmpeg")
                    .args(["-nostdin", "-loglevel", "error", "-y", "-i"])
                    .arg(&base)
                    .arg("-i")
                    .arg(&artwork)
                    .args([
                        "-map",
                        "0:v",
                        "-map",
                        "1:v",
                        "-c",
                        "copy",
                        "-disposition:v:1",
                        "attached_pic",
                        "-f",
                        "mp4",
                    ])
                    .arg(&embedded)
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(
                probe(&embedded)
                    .unwrap()
                    .unwrap()
                    .attached_picture
                    .is_some()
            );
            assert!(prepare_poster(&embedded, &cache).unwrap());
            assert!(usable(&poster_path(&embedded, &cache)));
        }
    }
}
