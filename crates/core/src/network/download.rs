//! Bounded, credential-free downloads, modified 2026-10-27; original GPLv3 applies.
use crate::error::DownloadError;
use async_std::io::ReadExt;
use isahc::{
    config::{Configurable, RedirectPolicy},
    http::{header, Uri},
    HttpClient, Request,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;
fn invalid(message: &str) -> DownloadError {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

fn address(url: &str) -> Result<Uri, DownloadError> {
    let uri: Uri = url.parse().map_err(|_| invalid("Invalid download URL"))?;
    let local_fixture =
        cfg!(test) && uri.scheme_str() == Some("http") && uri.host() == Some("127.0.0.1");
    if (!local_fixture && uri.scheme_str() != Some("https"))
        || uri.host().is_none()
        || uri.authority().is_some_and(|a| a.as_str().contains('@'))
    {
        return Err(invalid("Download requires HTTPS without credentials"));
    }
    Ok(uri)
}

fn redirect_allowed(from: &Uri, to: &Uri) -> bool {
    from.authority() == to.authority()
        || (from.host() == Some("github.com")
            && to.host() == Some("release-assets.githubusercontent.com")
            && to.port_u16().is_none())
}

// Modified 2026-10-28; original GPLv3 applies.
// Modified 2026-10-28; original GPLv3 applies.
const MIN_RATE_WAIT: u64 = 60;
const FALLBACK_RATE_WAIT: u64 = 60 * 60;

// Reject duplicate, oversized and non-text values. Never retain header text.
fn rate_header<'a>(headers: &'a isahc::http::HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() || value.as_bytes().len() > 64 {
        return None;
    }
    value.to_str().ok().map(str::trim)
}

fn rate_number(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(value.bytes().fold(0u64, |number, ch| {
        number
            .saturating_mul(10)
            .saturating_add(u64::from(ch - b'0'))
    }))
}

// Strict IMF-fixdate, using the existing chrono dependency.
// Round-trip validation rejects alternate spellings, spacing and weekdays.
fn rate_date(value: &str) -> Option<u64> {
    const FORMAT: &str = "%a, %d %b %Y %H:%M:%S GMT";
    if value.len() != 29 || !value.is_ascii() {
        return None;
    }
    let date = chrono::NaiveDateTime::parse_from_str(value, FORMAT).ok()?;
    if chrono::Timelike::nanosecond(&date) != 0 || date.format(FORMAT).to_string() != value {
        return None;
    }
    let timestamp = date.timestamp();
    if timestamp < 0 {
        return None;
    }
    Some(timestamp as u64)
}

// Duration::MAX is the fail-closed marker for saturated numeric input.
// It must never become a shorter finite GUI wait.
fn rate_delay(seconds: u64) -> Duration {
    if seconds == u64::MAX {
        Duration::MAX
    } else {
        Duration::from_secs(seconds)
    }
}

fn rate_until(timestamp: u64, now: SystemTime) -> Duration {
    if timestamp == u64::MAX {
        return Duration::MAX;
    }
    match now.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => Duration::from_secs(timestamp).saturating_sub(elapsed),
        Err(_) => Duration::MAX,
    }
}

fn rate_wait(
    code: isahc::http::StatusCode,
    headers: &isahc::http::HeaderMap,
    now: SystemTime,
) -> Option<Duration> {
    let retry_after = rate_header(headers, "retry-after").and_then(|value| {
        rate_number(value)
            .map(rate_delay)
            .or_else(|| rate_date(value).map(|date| rate_until(date, now)))
    });
    let exhausted = rate_header(headers, "x-ratelimit-remaining").and_then(rate_number) == Some(0);

    if code != isahc::http::StatusCode::TOO_MANY_REQUESTS
        && !(code == isahc::http::StatusCode::FORBIDDEN && (exhausted || retry_after.is_some()))
    {
        return None;
    }

    let reset = if exhausted || code == isahc::http::StatusCode::TOO_MANY_REQUESTS {
        rate_header(headers, "x-ratelimit-reset")
            .and_then(rate_number)
            .map(|reset| rate_until(reset, now))
    } else {
        None
    };
    let wait = match (retry_after, reset) {
        (Some(retry), Some(reset)) => retry.max(reset),
        (Some(wait), None) | (None, Some(wait)) => wait,
        (None, None) => Duration::from_secs(FALLBACK_RATE_WAIT),
    };
    Some(wait.max(Duration::from_secs(MIN_RATE_WAIT)))
}

async fn receive(
    url: &str,
    limit: u64,
    expected: Option<u64>,
    digest: Option<&str>,
    timeout: Duration,
    sink: &mut impl Write,
) -> Result<(), DownloadError> {
    let hash = digest.map(|v| v.strip_prefix("sha256:").unwrap_or(v));
    if hash.is_some_and(|v| v.len() != 64 || !v.bytes().all(|c| c.is_ascii_hexdigit()))
        || expected.is_some_and(|n| n == 0 || n > limit)
    {
        return Err(invalid("Invalid expected download size or SHA-256"));
    }
    let client = HttpClient::builder()
        .redirect_policy(RedirectPolicy::None)
        .build()?;
    let mut uri = address(url)?;
    for hop in 0..=5 {
        let request = Request::get(uri.clone())
            .header(header::USER_AGENT, super::user_agent())
            .header(header::ACCEPT_ENCODING, "identity")
            .timeout(timeout)
            .body(())?;
        let mut response = client.send_async(request).await?;

        if response.status().is_redirection() {
            if hop == 5 {
                return Err(invalid("Too many download redirects"));
            }
            let next = address(
                response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| invalid("Missing redirect URL"))?,
            )?;
            if !redirect_allowed(&uri, &next) {
                return Err(invalid("Unapproved download redirect origin"));
            }
            uri = next;
            continue;
        }
        if response.status() != 200 {
            if let Some(retry_after) =
                rate_wait(response.status(), response.headers(), SystemTime::now())
            {
                return Err(DownloadError::RateLimited {
                    code: response.status(),
                    retry_after,
                });
            }
            return Err(DownloadError::InvalidStatusCode {
                code: response.status(),
                url: "download endpoint (no automatic retries)".into(),
            });
        }

        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|v| v != "identity")
        {
            return Err(invalid("Encoded downloads are not supported"));
        }
        let lengths: Vec<_> = response
            .headers()
            .get_all(header::CONTENT_LENGTH)
            .iter()
            .collect();
        if lengths.len() > 1 {
            return Err(invalid("Ambiguous Content-Length"));
        }
        let declared = lengths
            .first()
            .map(|v| {
                v.to_str()
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .ok_or_else(|| invalid("Invalid Content-Length"))
            })
            .transpose()?;
        if declared.is_some_and(|n| n > limit || expected.is_some_and(|e| e != n)) {
            return Err(invalid(
                "Download length exceeds or disagrees with expected size",
            ));
        }
        let mut hasher = Sha256::new();
        let mut count = 0u64;
        let mut chunk = vec![0u8; 16 * 1024];
        loop {
            let n = response.body_mut().read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > limit || expected.is_some_and(|e| count > e) {
                return Err(invalid("Download exceeded byte limit"));
            }
            hasher.update(&chunk[..n]);
            sink.write_all(&chunk[..n])?;
        }
        if count == 0
            || declared.is_some_and(|n| n != count)
            || expected.is_some_and(|n| n != count)
        {
            return Err(invalid("Incomplete or empty download"));
        }
        if hash.is_some_and(|v| !v.eq_ignore_ascii_case(&format!("{:x}", hasher.finalize()))) {
            return Err(invalid("Download SHA-256 mismatch"));
        }
        return Ok(());
    }
    unreachable!()
}

pub(crate) fn plain_directory(path: &Path) -> Result<(), DownloadError> {
    for ancestor in path.ancestors() {
        let m = fs::symlink_metadata(ancestor)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err(invalid("Download path has a reparse point"));
            }
        }
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(invalid("Download path has a link or is not a directory"));
        }
    }
    Ok(())
}

/// Publish only a complete validated file, without replacing an existing destination.
pub async fn download_verified(
    url: &str,
    destination: &Path,
    size: Option<u64>,
    digest: Option<&str>,
) -> Result<(), DownloadError> {
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("Missing download parent"))?;
    plain_directory(parent)?;
    if fs::symlink_metadata(destination).is_ok() {
        return Err(invalid("Download destination already exists"));
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".patchloom-download-")
        .tempfile_in(parent)?;
    receive(
        url,
        MAX_DOWNLOAD,
        size,
        digest,
        Duration::from_secs(120),
        temporary.as_file_mut(),
    )
    .await?;
    temporary.as_file().sync_all()?;
    plain_directory(parent)?;
    temporary
        .persist_noclobber(destination)
        .map_err(|e| e.error)?;
    Ok(())
}

pub(crate) async fn metadata(url: &str) -> Result<Vec<u8>, DownloadError> {
    let mut bytes = vec![];
    receive(
        url,
        1024 * 1024,
        None,
        None,
        Duration::from_secs(30),
        &mut bytes,
    )
    .await?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, net::TcpListener};
    fn accept_fixture(listener: &TcpListener) -> std::net::TcpStream {
        listener.set_nonblocking(true).unwrap();
        let start = std::time::Instant::now();
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    return stream;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => panic!("Fixture accept failed: {}", error),
            }
        }
    }
    fn server(response: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut stream = accept_fixture(&listener);
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
        });
        url
    }
    #[test]
    fn http_errors_partial_oversize_redirects_and_hashes_never_publish() {
        async_std::task::block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let dest = temp.path().join("addon");
            for status in [403, 404, 429, 500] {
                let url = server(
                    format!("HTTP/1.1 {status} Failed\r\nContent-Length: 3\r\n\r\nbad")
                        .into_bytes(),
                );
                assert!(download_verified(&url, &dest, None, None).await.is_err());
                assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
            }
            for (response, size, hash) in [
                ("HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nabc", None, None),
                ("HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n", None, None),
                ("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc", Some(2), None),
                ("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc", Some(3), Some("0000000000000000000000000000000000000000000000000000000000000000")),
                ("HTTP/1.1 302 Found\r\nLocation: https://untrusted.invalid/asset\r\nContent-Length: 0\r\n\r\n", None, None),
            ] {
                assert!(download_verified(&server(response.as_bytes().to_vec()), &dest, size, hash).await.is_err());
                assert_eq!(fs::read_dir(temp.path()).unwrap().count(),0);
            }
            let url = server(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabc".to_vec());
            download_verified(
                &url,
                &dest,
                Some(3),
                Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            )
            .await
            .unwrap();
            assert_eq!(fs::read(&dest).unwrap(), b"abc");
            assert!(
                download_verified("https://invalid.invalid", &dest, None, None)
                    .await
                    .is_err()
            );
            assert_eq!(fs::read(&dest).unwrap(), b"abc");
        });
    }
    #[test]
    fn streamed_limits_timeouts_and_linked_paths_fail_closed() {
        async_std::task::block_on(async {
            let mut sink = vec![];
            assert!(receive(
                &server(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n12345".to_vec()),
                3,
                None,
                None,
                Duration::from_secs(2),
                &mut sink
            )
            .await
            .is_err());
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let child = std::thread::spawn(move || {
                let _stream = accept_fixture(&listener);
                std::thread::sleep(Duration::from_millis(300));
            });
            assert!(receive(
                &url,
                100,
                None,
                None,
                Duration::from_millis(50),
                &mut vec![]
            )
            .await
            .is_err());
            child.join().unwrap();
            assert!(address("http://example.com/asset").is_err());
            assert!(address("https://user:secret@example.com/asset").is_err());
            let temp = tempfile::tempdir().unwrap();
            let outside = temp.path().join("outside");
            fs::create_dir(&outside).unwrap();
            let link = temp.path().join("linked");
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                assert!(std::process::Command::new("cmd.exe")
                    .args(["/D", "/C", "mklink", "/J"])
                    .arg(&link)
                    .arg(&outside)
                    .creation_flags(0x08000000)
                    .output()
                    .unwrap()
                    .status
                    .success());
            }
            assert!(
                download_verified("https://invalid.invalid", &link.join("file"), None, None)
                    .await
                    .is_err()
            );
            assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
        });
    }

    #[test]
    #[ignore = "subprocess protocol; fixture parent invokes exactly this test"]
    fn download_child_entry() {
        let Ok(url) = std::env::var("PATCHLOOM_FIXTURE_DOWNLOAD_URL") else {
            return;
        };
        let destination = std::env::var_os("PATCHLOOM_FIXTURE_DOWNLOAD_DEST").unwrap();
        async_std::task::block_on(download_verified(&url, Path::new(&destination), None, None))
            .unwrap();
    }
    #[test]
    fn forced_process_stop_leaves_no_published_partial_and_retry_preserves_orphan() {
        use std::{process::Command, time::Instant};
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("archive");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "network::download::tests::download_child_entry",
                "--ignored",
                "--nocapture",
            ])
            .env("PATCHLOOM_FIXTURE_DOWNLOAD_URL", url)
            .env("PATCHLOOM_FIXTURE_DOWNLOAD_DEST", &dest);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let start = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("{}", e),
            }
            if start.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("download child did not connect");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n")
            .unwrap();
        stream.write_all(&[b'p'; 32 * 1024]).unwrap();
        let start = Instant::now();
        while !fs::read_dir(temp.path())
            .unwrap()
            .any(|e| !fs::read(e.unwrap().path()).unwrap_or_default().is_empty())
        {
            if start.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("download did not start streaming");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        drop(stream);
        assert!(!dest.exists());
        let orphan = fs::read_dir(temp.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = fs::read(&orphan).unwrap();
        async_std::task::block_on(download_verified(
            &server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc".to_vec()),
            &dest,
            Some(3),
            None,
        ))
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"abc");
        assert_eq!(fs::read(orphan).unwrap(), bytes);
    }
    #[test]
    fn independent_destination_race_and_cancel_preserve_files() {
        async_std::task::block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let dest = temp.path().join("addon.zip");
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let raced = dest.clone();
            let worker = std::thread::spawn(move || {
                let mut stream = accept_fixture(&listener);
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                fs::write(raced, b"arrived-during-transfer").unwrap();
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc")
                    .unwrap();
            });
            assert!(download_verified(&url, &dest, Some(3), None).await.is_err());
            worker.join().unwrap();
            assert_eq!(fs::read(&dest).unwrap(), b"arrived-during-transfer");
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
            let cancel = temp.path().join("cancel.zip");
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let mut stream = accept_fixture(&listener);
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n");
                let _ = stream.write_all(&[b'p'; 32768]);
                std::thread::sleep(Duration::from_millis(250));
            });
            assert!(async_std::future::timeout(
                Duration::from_millis(100),
                download_verified(&url, &cancel, None, None)
            )
            .await
            .is_err());
            worker.join().unwrap();
            assert!(!cancel.exists());
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        });
    }
    #[test]
    fn same_origin_redirects_are_bounded_and_verified() {
        for redirects in [1, 6] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let location = url.clone();
            let worker = std::thread::spawn(move || {
                for hop in 0..=redirects.min(5) {
                    let mut stream = accept_fixture(&listener);
                    let mut request = [0; 4096];
                    let _ = stream.read(&mut request);
                    if hop < redirects {
                        write!(stream,"HTTP/1.1 302 Found\r\nLocation: {location}/asset\r\nContent-Length: 0\r\n\r\n").unwrap();
                    } else {
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc")
                            .unwrap();
                    }
                }
            });
            let temp = tempfile::tempdir().unwrap();
            let dest = temp.path().join("download");
            let result = async_std::task::block_on(download_verified(&url, &dest, Some(3), None));
            assert_eq!(result.is_ok(), redirects == 1);
            assert_eq!(dest.exists(), redirects == 1);
            worker.join().unwrap();
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    mod rate_retry {
        use super::super::*;
        use isahc::http::{HeaderMap, HeaderValue, StatusCode};

        fn headers(entries: &[(&'static str, &str)]) -> HeaderMap {
            let mut headers = HeaderMap::new();
            for &(name, value) in entries {
                headers.append(name, HeaderValue::from_str(value).unwrap());
            }
            headers
        }

        fn wait(code: StatusCode, entries: &[(&'static str, &str)]) -> Option<u64> {
            rate_wait(
                code,
                &headers(entries),
                UNIX_EPOCH + Duration::from_secs(784_111_777),
            )
            .map(|wait| wait.as_secs())
        }

        #[test]
        fn status_and_actual_rate_evidence_control_classification() {
            use StatusCode as S;

            for (code, evidence, expected) in [
                (S::TOO_MANY_REQUESTS, vec![], Some(3_600)),
                (S::FORBIDDEN, vec![], None),
                (S::FORBIDDEN, vec![("retry-after", "120")], Some(120)),
                (
                    S::FORBIDDEN,
                    vec![("x-ratelimit-remaining", "0")],
                    Some(3_600),
                ),
                (S::FORBIDDEN, vec![("x-ratelimit-remaining", "1")], None),
                (S::FORBIDDEN, vec![("x-ratelimit-reset", "784111957")], None),
                (S::FORBIDDEN, vec![("retry-after", "invalid")], None),
                (S::UNAUTHORIZED, vec![("retry-after", "120")], None),
                (S::INTERNAL_SERVER_ERROR, vec![("retry-after", "120")], None),
                (S::OK, vec![("x-ratelimit-remaining", "0")], None),
            ] {
                assert_eq!(wait(code, &evidence), expected, "{code}: {evidence:?}");
            }
        }

        #[test]
        fn delay_seconds_preserve_long_minima_and_overflow_fails_closed() {
            for (value, expected) in [
                ("0", Duration::from_secs(60)),
                ("59", Duration::from_secs(60)),
                ("60", Duration::from_secs(60)),
                ("120", Duration::from_secs(120)),
                ("86400", Duration::from_secs(86_400)),
                ("86401", Duration::from_secs(86_401)),
                ("172800", Duration::from_secs(172_800)),
                ("18446744073709551615", Duration::MAX),
                ("18446744073709551616", Duration::MAX),
            ] {
                assert_eq!(
                    rate_wait(
                        StatusCode::TOO_MANY_REQUESTS,
                        &headers(&[("retry-after", value)]),
                        UNIX_EPOCH,
                    ),
                    Some(expected),
                    "{}",
                    value,
                );
            }
        }

        #[test]
        fn applicable_reset_and_retry_after_use_the_later_wait() {
            let code = StatusCode::TOO_MANY_REQUESTS;
            assert_eq!(wait(code, &[("x-ratelimit-reset", "784111957")]), Some(180),);
            assert_eq!(
                wait(
                    code,
                    &[("retry-after", "120"), ("x-ratelimit-reset", "784111957"),]
                ),
                Some(180),
            );
            assert_eq!(
                wait(
                    code,
                    &[("retry-after", "240"), ("x-ratelimit-reset", "784111957"),]
                ),
                Some(240),
            );
            assert_eq!(
                wait(
                    StatusCode::FORBIDDEN,
                    &[
                        ("x-ratelimit-remaining", "0"),
                        ("x-ratelimit-reset", "784111957"),
                    ]
                ),
                Some(180),
            );
            // A non-exhausted primary reset does not extend a secondary-limit hint.
            assert_eq!(
                wait(
                    StatusCode::FORBIDDEN,
                    &[
                        ("retry-after", "120"),
                        ("x-ratelimit-remaining", "1"),
                        ("x-ratelimit-reset", "784115377"),
                    ]
                ),
                Some(120),
            );
            for (reset, expected) in [
                ("784111777", 60),
                ("0", 60),
                ("784198178", 86_401),
                ("18446744073709551616", u64::MAX),
                ("invalid", 3_600),
            ] {
                assert_eq!(
                    wait(code, &[("x-ratelimit-reset", reset)]),
                    Some(expected),
                    "{}",
                    reset,
                );
            }
        }

        #[test]
        fn imf_fixdate_uses_the_supplied_clock_and_wait_bounds() {
            assert_eq!(
                rate_date("Sun, 06 Nov 1994 08:49:37 GMT"),
                Some(784_111_777),
            );
            assert_eq!(
                rate_date("Thu, 29 Feb 2024 00:00:00 GMT"),
                Some(1_709_164_800),
            );
            for (date, expected) in [
                ("Sun, 06 Nov 1994 08:50:37 GMT", 60),
                ("Sun, 06 Nov 1994 08:51:37 GMT", 120),
                ("Sun, 06 Nov 1994 08:49:37 GMT", 60),
                ("Sat, 05 Nov 1994 08:49:37 GMT", 60),
                ("Tue, 08 Nov 1994 08:49:37 GMT", 172_800),
            ] {
                assert_eq!(
                    wait(StatusCode::FORBIDDEN, &[("retry-after", date)]),
                    Some(expected),
                    "{date}",
                );
            }

            for invalid in [
                "Sunday, 06-Nov-94 08:49:37 GMT",
                "Sun Nov  6 08:49:37 1994",
                "Wed, 29 Feb 2023 00:00:00 GMT",
                "Tue, 29 Feb 2100 00:00:00 GMT",
                "Sun, 31 Apr 2024 00:00:00 GMT",
                "Sun, 06 Nov 1994 24:00:00 GMT",
                "Sun, 06 Nov 1994 08:60:00 GMT",
                "Sun, 06 Nov 1994 08:49:60 GMT",
                "Sun, 06 Nov 1994 08:49:37 UTC",
                "Sun, 06 Nov 1969 08:49:37 GMT",
            ] {
                assert_eq!(rate_date(invalid), None, "{invalid}");
                assert_eq!(
                    wait(StatusCode::TOO_MANY_REQUESTS, &[("retry-after", invalid)]),
                    Some(3_600),
                    "{invalid}",
                );
                assert_eq!(
                    wait(StatusCode::FORBIDDEN, &[("retry-after", invalid)]),
                    None,
                    "{invalid}",
                );
            }
        }

        #[test]
        fn malformed_ambiguous_or_oversized_hints_do_not_create_evidence() {
            for value in ["", "-1", "+120", "1.5", "120 seconds", "120, 240"] {
                assert_eq!(
                    wait(StatusCode::TOO_MANY_REQUESTS, &[("retry-after", value)]),
                    Some(3_600),
                );
                assert_eq!(wait(StatusCode::FORBIDDEN, &[("retry-after", value)]), None,);
            }

            let duplicate = [("retry-after", "120"), ("retry-after", "240")];
            assert_eq!(wait(StatusCode::FORBIDDEN, &duplicate), None);
            assert_eq!(wait(StatusCode::TOO_MANY_REQUESTS, &duplicate), Some(3_600),);

            let oversized = "9".repeat(65);
            assert_eq!(
                wait(StatusCode::FORBIDDEN, &[("retry-after", &oversized)]),
                None,
            );
            assert_eq!(
                wait(
                    StatusCode::TOO_MANY_REQUESTS,
                    &[("retry-after", &oversized)]
                ),
                Some(3_600),
            );

            // Reject the ambiguous hint, but retain independent valid evidence/hints.
            assert_eq!(
                wait(
                    StatusCode::FORBIDDEN,
                    &[
                        ("x-ratelimit-remaining", "0"),
                        ("retry-after", "120"),
                        ("retry-after", "240"),
                        ("x-ratelimit-reset", "784111957"),
                    ]
                ),
                Some(180),
            );
            assert_eq!(
                wait(
                    StatusCode::TOO_MANY_REQUESTS,
                    &[
                        ("x-ratelimit-reset", "784111957"),
                        ("x-ratelimit-reset", "784112017"),
                    ]
                ),
                Some(3_600),
            );
        }

        #[test]
        fn long_server_minima_and_overflow_dominate_shorter_hints() {
            let now = UNIX_EPOCH + Duration::from_secs(784_111_777);
            for entries in [
                vec![
                    ("retry-after", "172800"),
                    ("x-ratelimit-reset", "784198178"),
                ],
                vec![("retry-after", "60"), ("x-ratelimit-reset", "784284577")],
                vec![
                    ("retry-after", "Tue, 08 Nov 1994 08:49:37 GMT"),
                    ("x-ratelimit-reset", "784198178"),
                ],
            ] {
                assert_eq!(
                    rate_wait(StatusCode::TOO_MANY_REQUESTS, &headers(&entries), now),
                    Some(Duration::from_secs(172_800)),
                );
            }
            for entries in [
                vec![
                    ("retry-after", "18446744073709551616"),
                    ("x-ratelimit-reset", "784111897"),
                ],
                vec![
                    ("retry-after", "120"),
                    ("x-ratelimit-reset", "18446744073709551616"),
                ],
            ] {
                assert_eq!(
                    rate_wait(StatusCode::TOO_MANY_REQUESTS, &headers(&entries), now),
                    Some(Duration::MAX),
                );
            }
        }

        #[test]
        fn absolute_minima_preserve_fractional_precision_and_fail_closed_clock() {
            let entries = headers(&[
                ("retry-after", "Sun, 06 Nov 1994 08:51:37 GMT"),
                ("x-ratelimit-reset", "784111958"),
            ]);
            let now = UNIX_EPOCH + Duration::new(784_111_777, 500_000_000);
            assert_eq!(
                rate_wait(StatusCode::TOO_MANY_REQUESTS, &entries, now),
                Some(Duration::new(180, 500_000_000)),
            );
            let before_epoch = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();
            assert_eq!(
                rate_wait(StatusCode::FORBIDDEN, &entries, before_epoch),
                Some(Duration::MAX),
            );
        }

        #[test]
        fn chrono_parser_requires_exact_imf_form_and_matching_weekday() {
            for invalid in [
                "Mon, 06 Nov 1994 08:49:37 GMT",
                "sun, 06 Nov 1994 08:49:37 GMT",
                "Sun, 6 Nov 1994 08:49:37 GMT",
                "Sun, 06 nov 1994 08:49:37 GMT",
                "Sun, 06 Nov 1994 08:49:37 +0000",
                "Sun, 06 Nov 1994 08:49:37 GMT ",
                "Sun, 06 Nov 1994 08:49:60 GMT",
            ] {
                assert_eq!(rate_date(invalid), None, "{}", invalid);
            }
            assert_eq!(
                rate_date("Sat, 01 Jan 2000 00:00:00 GMT"),
                Some(946_684_800),
            );
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    mod rate_transport {
        use super::*;
        use isahc::http::StatusCode;

        type Boxed = Box<dyn std::error::Error + Send + Sync>;

        fn response(code: u16, headers: &[(&str, &str)]) -> Vec<u8> {
            let mut response = format!("HTTP/1.1 {} Fixture\r\nConnection: close\r\n", code,);
            for &(name, value) in headers {
                response.push_str(name);
                response.push_str(": ");
                response.push_str(value);
                response.push_str("\r\n");
            }
            response.push_str("Content-Length: 3\r\n\r\nbad");
            response.into_bytes()
        }

        // Real response -> shared receive. No parser-only substitute.
        fn receive_error(code: u16, headers: &[(&str, &str)]) -> DownloadError {
            let url = server(response(code, headers));
            let mut sink = Vec::new();
            let error = async_std::task::block_on(receive(
                &url,
                1024,
                None,
                None,
                Duration::from_secs(2),
                &mut sink,
            ))
            .unwrap_err();
            assert!(sink.is_empty(), "An HTTP error body reached the sink");
            error
        }

        fn assert_error(error: &DownloadError, code: u16, expected_wait: Option<Duration>) {
            let expected_code = StatusCode::from_u16(code).unwrap();
            match (error, expected_wait) {
                (DownloadError::RateLimited { code, retry_after }, Some(expected)) => {
                    assert_eq!(*code, expected_code);
                    assert_eq!(*retry_after, expected);
                }
                (DownloadError::InvalidStatusCode { code, .. }, None) => {
                    assert_eq!(*code, expected_code);
                }
                other => panic!("Unexpected transport outcome: {:?}", other),
            }
        }

        #[test]
        fn real_responses_distinguish_429_evidenced_403_and_other_errors() {
            for (code, headers, expected) in [
                (429, vec![], Some(Duration::from_secs(3600))),
                (403, vec![], None),
                (
                    403,
                    vec![("X-RateLimit-Remaining", "0")],
                    Some(Duration::from_secs(3600)),
                ),
                (
                    403,
                    vec![("Retry-After", "120")],
                    Some(Duration::from_secs(120)),
                ),
                (
                    403,
                    vec![("X-RateLimit-Remaining", "1"), ("Retry-After", "120")],
                    Some(Duration::from_secs(120)),
                ),
                (403, vec![("X-RateLimit-Reset", "253402300799")], None),
                (
                    403,
                    vec![
                        ("X-RateLimit-Remaining", "1"),
                        ("X-RateLimit-Reset", "253402300799"),
                    ],
                    None,
                ),
                (403, vec![("Retry-After", "invalid")], None),
                (401, vec![("Retry-After", "120")], None),
                (500, vec![("X-RateLimit-Remaining", "0")], None),
            ] {
                assert_error(&receive_error(code, &headers), code, expected);
            }
        }

        #[test]
        fn real_delay_headers_preserve_long_minima_and_overflow_marker() {
            for code in [429, 403] {
                for (value, expected) in [
                    ("0", Duration::from_secs(60)),
                    ("59", Duration::from_secs(60)),
                    ("86401", Duration::from_secs(86_401)),
                    ("172800", Duration::from_secs(172_800)),
                    ("18446744073709551615", Duration::MAX),
                    ("18446744073709551616", Duration::MAX),
                ] {
                    assert_error(
                        &receive_error(code, &[("Retry-After", value)]),
                        code,
                        Some(expected),
                    );
                }
            }

            // A saturated reset must dominate a shorter valid delay.
            for code in [429, 403] {
                assert_error(
                    &receive_error(
                        code,
                        &[
                            ("X-RateLimit-Remaining", "0"),
                            ("Retry-After", "120"),
                            ("X-RateLimit-Reset", "18446744073709551616"),
                        ],
                    ),
                    code,
                    Some(Duration::MAX),
                );
            }

            // A saturated delay must dominate a shorter valid reset.
            assert_error(
                &receive_error(
                    429,
                    &[
                        ("Retry-After", "18446744073709551616"),
                        ("X-RateLimit-Reset", "253402300799"),
                    ],
                ),
                429,
                Some(Duration::MAX),
            );
        }

        #[test]
        fn real_malformed_duplicate_and_oversized_headers_keep_policy() {
            let oversized = "9".repeat(65);
            for value in [
                "",
                "-1",
                "+120",
                "1.5",
                "120 seconds",
                "120, 240",
                "Sunday, 06-Nov-94 08:49:37 GMT",
                "Mon, 06 Nov 1994 08:49:37 GMT",
                oversized.as_str(),
            ] {
                assert_error(
                    &receive_error(429, &[("Retry-After", value)]),
                    429,
                    Some(Duration::from_secs(3600)),
                );
                assert_error(&receive_error(403, &[("Retry-After", value)]), 403, None);
            }

            let duplicate = [("Retry-After", "120"), ("Retry-After", "240")];
            assert_error(
                &receive_error(429, &duplicate),
                429,
                Some(Duration::from_secs(3600)),
            );
            assert_error(&receive_error(403, &duplicate), 403, None);

            // Independent exhaustion evidence survives an unusable delay.
            assert_error(
                &receive_error(
                    403,
                    &[
                        ("X-RateLimit-Remaining", "0"),
                        ("Retry-After", "invalid"),
                        ("X-RateLimit-Reset", "invalid"),
                    ],
                ),
                403,
                Some(Duration::from_secs(3600)),
            );

            // An ambiguous reset cannot override a valid long delay.
            assert_error(
                &receive_error(
                    429,
                    &[
                        ("Retry-After", "172800"),
                        ("X-RateLimit-Reset", "253402300799"),
                        ("X-RateLimit-Reset", "253402300798"),
                    ],
                ),
                429,
                Some(Duration::from_secs(172_800)),
            );

            // Duplicate exhaustion evidence alone does not classify 403.
            assert_error(
                &receive_error(
                    403,
                    &[
                        ("X-RateLimit-Remaining", "0"),
                        ("X-RateLimit-Remaining", "0"),
                    ],
                ),
                403,
                None,
            );
        }

        #[test]
        fn real_absolute_headers_preserve_later_minimum_beyond_24_hours() {
            // Fri, 31 Dec 9999 23:59:59 GMT, independently specified as
            // both an IMF date and Unix seconds. No parser-derived oracle.
            const TARGET: u64 = 253_402_300_799;

            for (code, headers, relative_minimum) in [
                (
                    429,
                    vec![
                        ("Retry-After", "120"),
                        ("X-RateLimit-Reset", "253402300799"),
                    ],
                    Duration::from_secs(120),
                ),
                (
                    403,
                    vec![
                        ("X-RateLimit-Remaining", "0"),
                        ("Retry-After", "172800"),
                        ("X-RateLimit-Reset", "253402300799"),
                    ],
                    Duration::from_secs(172_800),
                ),
                (
                    403,
                    vec![("Retry-After", "Fri, 31 Dec 9999 23:59:59 GMT")],
                    Duration::ZERO,
                ),
                (
                    429,
                    vec![
                        ("Retry-After", "Fri, 31 Dec 9999 23:59:59 GMT"),
                        ("X-RateLimit-Reset", "4102444800"),
                    ],
                    Duration::ZERO,
                ),
            ] {
                // receive samples SystemTime internally. Bracket that sample,
                // retaining subsecond precision; no sleeps or timing tolerance.
                let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                let error = receive_error(code, &headers);
                let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

                let earliest = before.min(after);
                let latest = before.max(after);
                let target = Duration::from_secs(TARGET);
                let lower = target.saturating_sub(latest).max(relative_minimum);
                let upper = target.saturating_sub(earliest).max(relative_minimum);
                assert!(
                    lower > Duration::from_secs(86_400),
                    "Fixture requires a wall clock before the distant target",
                );

                match error {
                    DownloadError::RateLimited {
                        code: actual,
                        retry_after,
                    } => {
                        assert_eq!(actual, StatusCode::from_u16(code).unwrap());
                        assert!(
                            retry_after >= lower && retry_after <= upper,
                            "Expected {:?}..={:?}, got {:?}",
                            lower,
                            upper,
                            retry_after,
                        );
                    }
                    other => panic!("Absolute rate metadata lost: {:?}", other),
                }
            }
        }

        // Test-local boxed boundary using a real metadata response and `?`.
        // This does not invoke github_releases::check or change its URL policy.
        async fn boxed_metadata(url: &str) -> Result<Vec<u8>, Boxed> {
            Ok(metadata(url).await?)
        }

        #[test]
        fn real_metadata_errors_survive_returned_boxed_boundary() {
            for (code, headers, expected) in [
                (429, vec![], Some(Duration::from_secs(3600))),
                (
                    403,
                    vec![("Retry-After", "172800")],
                    Some(Duration::from_secs(172_800)),
                ),
                (
                    429,
                    vec![("Retry-After", "18446744073709551616")],
                    Some(Duration::MAX),
                ),
                (403, vec![], None),
            ] {
                let url = server(response(code, &headers));
                let error = async_std::task::block_on(boxed_metadata(&url)).unwrap_err();
                let typed = error
                    .downcast_ref::<DownloadError>()
                    .expect("The returned box lost its concrete DownloadError");
                assert_error(typed, code, expected);
            }
        }

        #[test]
        fn real_rate_errors_leave_addon_destination_and_existing_files_untouched() {
            async_std::task::block_on(async {
                let temp = tempfile::tempdir().unwrap();
                let sentinel = temp.path().join("keep");
                let destination = temp.path().join("addon.zip");
                fs::write(&sentinel, b"existing fixture bytes\n").unwrap();

                for (code, headers, expected) in [
                    (
                        429,
                        vec![("Retry-After", "172800")],
                        Some(Duration::from_secs(172_800)),
                    ),
                    (
                        403,
                        vec![("X-RateLimit-Remaining", "0")],
                        Some(Duration::from_secs(3600)),
                    ),
                    (
                        429,
                        vec![("X-RateLimit-Reset", "18446744073709551616")],
                        Some(Duration::MAX),
                    ),
                    (403, vec![], None),
                ] {
                    let url = server(response(code, &headers));
                    let error = download_verified(&url, &destination, None, None)
                        .await
                        .unwrap_err();
                    assert_error(&error, code, expected);
                    assert!(!destination.exists());
                    assert_eq!(fs::read(&sentinel).unwrap(), b"existing fixture bytes\n");
                    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
                }
            });
        }
    }
}
