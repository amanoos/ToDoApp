//! `todo-app healthcheck`: asks the running server whether it is healthy, for container
//! health checks. It uses only the standard library, so the runtime image needs no `curl`.

use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(3);

/// The address to connect to, from the same `HOST` and `PORT` the server reads. A
/// wildcard `HOST` (such as `0.0.0.0` in a container) is reached through loopback.
fn target(host: Option<&str>, port: Option<&str>) -> String {
    let host = match host.map(str::trim) {
        None | Some("" | "0.0.0.0" | "::" | "[::]") => "127.0.0.1",
        Some(host) => host,
    };
    let port = port
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("3000");
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Whether an HTTP response starts with a 200 status line.
fn is_ok(response: &str) -> bool {
    let first = response.lines().next().unwrap_or_default();
    let mut words = first.split_whitespace();
    matches!(
        (words.next(), words.next()),
        (Some(version), Some("200")) if version.starts_with("HTTP/1.")
    )
}

/// Requests `/healthz` from the local server. Returns `true` if it answers 200.
pub fn run() -> bool {
    let address = target(
        std::env::var("HOST").ok().as_deref(),
        std::env::var("PORT").ok().as_deref(),
    );
    let Some(socket) = address.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        eprintln!("healthcheck: cannot resolve {address}");
        return false;
    };
    let result = (|| -> std::io::Result<String> {
        let mut stream = TcpStream::connect_timeout(&socket, TIMEOUT)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream
            .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        // Only the status line matters, so a few hundred bytes are enough.
        let mut buffer = [0u8; 512];
        let read = stream.read(&mut buffer)?;
        Ok(String::from_utf8_lossy(&buffer[..read]).into_owned())
    })();
    match result {
        Ok(response) if is_ok(&response) => true,
        Ok(response) => {
            eprintln!(
                "healthcheck: unhealthy: {}",
                response.lines().next().unwrap_or("no response")
            );
            false
        }
        Err(error) => {
            eprintln!("healthcheck: {address}: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_follows_host_and_port_with_local_defaults() {
        assert_eq!(target(None, None), "127.0.0.1:3000");
        assert_eq!(target(Some("0.0.0.0"), Some("8080")), "127.0.0.1:8080");
        assert_eq!(target(Some("::"), Some("8080")), "127.0.0.1:8080");
        assert_eq!(target(Some(""), Some("")), "127.0.0.1:3000");
        assert_eq!(target(Some("10.1.2.3"), Some("9000")), "10.1.2.3:9000");
        assert_eq!(target(Some("::1"), Some("9000")), "[::1]:9000");
        assert_eq!(target(Some("[::1]"), Some("9000")), "[::1]:9000");
        assert_eq!(
            target(Some(" localhost "), Some(" 3100 ")),
            "localhost:3100"
        );
    }

    #[test]
    fn only_a_200_status_line_counts_as_healthy() {
        assert!(is_ok(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\r\nok"
        ));
        assert!(is_ok("HTTP/1.0 200 OK\r\n"));
        for bad in [
            "",
            "HTTP/1.1 500 Internal Server Error\r\n",
            "HTTP/1.1 307 Temporary Redirect\r\n",
            "HTTP/1.1 404 Not Found\r\n",
            "HTTP/1.1 2000 OK\r\n",
            "200 OK",
            "garbage",
            "\r\nHTTP/1.1 200 OK",
        ] {
            assert!(!is_ok(bad), "{bad:?}");
        }
    }
}
