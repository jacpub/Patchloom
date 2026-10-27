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
    time::Duration,
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
}
