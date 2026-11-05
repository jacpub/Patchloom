//! Native HTTP decompression compatibility, 2026-11-05; original GPLv3 applies.
use isahc::{
    config::{Configurable, RedirectPolicy},
    HttpClient,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

#[test]
fn native_http_gzip_decodes_fixture_and_rejects_corruption() {
    const GZIP: &[u8] = &[
        31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 75, 203, 172, 40, 41, 45, 74, 85, 200, 75, 44, 201, 44,
        75, 85, 72, 175, 202, 44, 80, 40, 74, 45, 46, 200, 207, 43, 78, 5, 0, 7, 177, 34, 153, 28,
        0, 0, 0,
    ];
    for valid in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("Fixture accept failed: {}", e),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut part = [0; 512];
                let n = stream.read(&mut part).unwrap();
                assert!(n > 0 && request.len() + n <= 8192);
                request.extend_from_slice(&part[..n]);
            }
            let body = if valid { GZIP } else { b"not gzip" };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(body).unwrap();
        });
        let client = HttpClient::builder()
            .proxy(None)
            .automatic_decompression(true)
            .redirect_policy(RedirectPolicy::None)
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let response = client.get(format!("http://{address}/fixture"));
        server.join().unwrap();
        let mut bytes = Vec::new();
        let result = match response {
            Ok(mut response) => {
                assert_eq!(response.status(), 200);
                response
                    .body_mut()
                    .take(4096)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        };
        if valid {
            result.unwrap();
            assert_eq!(bytes, b"fixture native gzip response");
        } else {
            assert!(
                result.is_err(),
                "Corrupt gzip must not be returned as success"
            );
        }
    }
}
