use super::registry::Tool;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::process::{Command, Stdio};

pub struct WebFetchTool;

const MAX_BODY_BYTES: usize = 1_000_000;
const MAX_STDERR_BYTES: usize = 16_384;
const MAX_URL_BYTES: usize = 8_192;
const MAX_REDIRECTS: usize = 5;
const CURL_METADATA_MARKER: &str = "CAIRN_WEB_FETCH_META:";

struct FetchTarget {
    url: String,
    host: String,
    port: u16,
    address: IpAddr,
    needs_resolve: bool,
}

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }
    fn description(&self) -> &str {
        "Fetch content from a URL and extract text"
    }
    fn needs_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}"#.into()
    }

    fn execute(&self, input: &str) -> Result<String, String> {
        let val = crate::json::parse(input).map_err(|e| format!("invalid input: {e}"))?;
        let obj = val.as_object().ok_or("expected object")?;
        let url = obj
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or("url required")?;

        let mut url = url.to_string();
        for redirects in 0..=MAX_REDIRECTS {
            let target = validate_target(&url)?;
            let (status, redirect, body) = fetch_once(&target)?;
            if (300..400).contains(&status) && !redirect.is_empty() {
                if redirects == MAX_REDIRECTS {
                    return Err(format!(
                        "web_fetch: too many redirects (maximum {MAX_REDIRECTS})"
                    ));
                }
                url = redirect;
                continue;
            }
            return Ok(html_to_text(&String::from_utf8_lossy(&body)));
        }
        unreachable!()
    }
}

fn validate_target(url: &str) -> Result<FetchTarget, String> {
    validate_target_with(url, |host, port| {
        (host, port)
            .to_socket_addrs()
            .map_err(|e| format!("web_fetch: could not resolve host: {e}"))
            .map(|addresses| addresses.map(|address| address.ip()).collect())
    })
}

fn validate_target_with<F>(url: &str, resolve: F) -> Result<FetchTarget, String>
where
    F: FnOnce(&str, u16) -> Result<Vec<IpAddr>, String>,
{
    if url.len() > MAX_URL_BYTES {
        return Err(format!("web_fetch: URL exceeds {MAX_URL_BYTES} bytes"));
    }
    if url
        .chars()
        .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
    {
        return Err("web_fetch: URL contains whitespace or control characters".into());
    }

    let (scheme, rest) = url
        .split_once("://")
        .ok_or("web_fetch: URL must use http or https")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err("web_fetch: only http and https URLs are allowed".into());
    }

    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let suffix = &rest[authority_end..];
    if authority.is_empty() || authority.contains('@') {
        return Err("web_fetch: URL must have a host and cannot contain credentials".into());
    }

    let default_port = if scheme == "https" { 443 } else { 80 };
    let (host, authority_host, port, literal_ip) =
        if let Some(after_open) = authority.strip_prefix('[') {
            let close = after_open
                .find(']')
                .ok_or("web_fetch: malformed IPv6 host")?;
            let host = &after_open[..close];
            let remainder = &after_open[close + 1..];
            let port = parse_port(remainder, default_port)?;
            let ip = host
                .parse::<Ipv6Addr>()
                .map_err(|_| "web_fetch: malformed IPv6 host")?;
            (
                ip.to_string(),
                format!("[{ip}]"),
                port,
                Some(IpAddr::V6(ip)),
            )
        } else {
            if authority.matches(':').count() > 1 {
                return Err("web_fetch: IPv6 hosts must be enclosed in brackets".into());
            }
            let (raw_host, port) = match authority.rsplit_once(':') {
                Some((host, port)) => (host, parse_port(&format!(":{port}"), default_port)?),
                None => (authority, default_port),
            };
            if raw_host.is_empty() {
                return Err("web_fetch: URL must have a host".into());
            }
            if let Ok(ip) = raw_host.parse::<Ipv4Addr>() {
                (ip.to_string(), ip.to_string(), port, Some(IpAddr::V4(ip)))
            } else {
                let host = validate_dns_name(raw_host)?;
                (host.clone(), host, port, None)
            }
        };

    let authority = if port == default_port {
        authority_host
    } else {
        format!("{authority_host}:{port}")
    };
    let canonical_url = format!("{scheme}://{authority}{suffix}");

    let addresses = if let Some(ip) = literal_ip {
        vec![ip]
    } else {
        resolve(&host, port)?
    };
    if addresses.is_empty() {
        return Err("web_fetch: host did not resolve to an address".into());
    }
    if addresses.iter().any(|address| !is_public_address(*address)) {
        return Err("web_fetch: local, private, and special-use addresses are not allowed".into());
    }

    Ok(FetchTarget {
        url: canonical_url,
        host,
        port,
        address: addresses[0],
        needs_resolve: literal_ip.is_none(),
    })
}

fn parse_port(remainder: &str, default: u16) -> Result<u16, String> {
    if remainder.is_empty() {
        return Ok(default);
    }
    let value = remainder
        .strip_prefix(':')
        .ok_or("web_fetch: malformed URL authority")?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("web_fetch: invalid port".into());
    }
    let port = value
        .parse::<u16>()
        .map_err(|_| "web_fetch: invalid port")?;
    if port == 0 {
        return Err("web_fetch: invalid port".into());
    }
    Ok(port)
}

fn validate_dns_name(host: &str) -> Result<String, String> {
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || host.starts_with("0x")
        || host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return Err("web_fetch: invalid host".into());
    }
    if !host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
    }) {
        return Err("web_fetch: invalid host".into());
    }
    Ok(host)
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let value = u32::from(ip);
            [
                (0x0000_0000, 8),  // current network
                (0x0a00_0000, 8),  // private
                (0x6440_0000, 10), // shared address space
                (0x7f00_0000, 8),  // loopback
                (0xa9fe_0000, 16), // link-local
                (0xac10_0000, 12), // private
                (0xc000_0000, 24), // IETF protocol assignments
                (0xc000_0200, 24), // documentation
                (0xc058_6300, 24), // 6to4 relay anycast
                (0xc0a8_0000, 16), // private
                (0xc612_0000, 15), // benchmarking
                (0xc633_6400, 24), // documentation
                (0xcb00_7100, 24), // documentation
                (0xe000_0000, 4),  // multicast
                (0xf000_0000, 4),  // reserved and broadcast
            ]
            .iter()
            .all(|(network, prefix)| {
                let mask = u32::MAX << (32 - prefix);
                value & mask != network & mask
            })
        }
        IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4_mapped() {
                return is_public_address(IpAddr::V4(ipv4));
            }
            let segments = ip.segments();
            (segments[0] & 0xe000) == 0x2000
                && !(segments[0] == 0x2001 && (segments[1] & 0xfe00) == 0)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && segments[0] != 0x2002
        }
    }
}

fn curl_args(target: &FetchTarget) -> Vec<String> {
    let mut args = vec![
        "-q".into(),
        "-sS".into(),
        "--globoff".into(),
        "--no-location".into(),
        "--noproxy".into(),
        "*".into(),
        "--proto".into(),
        "=http,https".into(),
        "--proto-redir".into(),
        "=http,https".into(),
        "--connect-timeout".into(),
        "5".into(),
        "--max-time".into(),
        "15".into(),
        "--max-filesize".into(),
        MAX_BODY_BYTES.to_string(),
    ];
    if target.needs_resolve {
        let address = match target.address {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        args.extend([
            "--resolve".into(),
            format!("{}:{}:{address}", target.host, target.port),
        ]);
    }
    args.extend([
        "--write-out".into(),
        format!("%{{stderr}}{CURL_METADATA_MARKER}%{{response_code}}\n%{{redirect_url}}"),
        "--".into(),
        target.url.clone(),
    ]);
    args
}

fn fetch_once(target: &FetchTarget) -> Result<(u16, String, Vec<u8>), String> {
    let mut child = Command::new("curl")
        .args(curl_args(target))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or("curl: could not capture output")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("curl: could not capture errors")?;
    let stderr_reader =
        std::thread::spawn(move || read_bounded_and_drain(&mut stderr, MAX_STDERR_BYTES));
    let mut body = Vec::new();
    let stdout_result = stdout
        .by_ref()
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut body);
    if stdout_result.is_err() || body.len() > MAX_BODY_BYTES {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| format!("curl: {e}"))?;
    let (stderr, stderr_overflow) = stderr_reader
        .join()
        .map_err(|_| "curl: failed to read errors".to_string())?
        .map_err(|e| format!("curl: {e}"))?;
    stdout_result.map_err(|e| format!("curl: {e}"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(format!(
            "web_fetch: response exceeds {MAX_BODY_BYTES} bytes"
        ));
    }
    if stderr_overflow {
        return Err("curl: response metadata exceeds safe limit".into());
    }
    if !status.success() {
        let message = String::from_utf8_lossy(&stderr);
        let message = message
            .split(CURL_METADATA_MARKER)
            .next()
            .unwrap_or("")
            .trim();
        return Err(format!(
            "curl: {}",
            if message.is_empty() {
                "request failed"
            } else {
                message
            }
        ));
    }

    let metadata = String::from_utf8_lossy(&stderr);
    let metadata = metadata
        .rsplit_once(CURL_METADATA_MARKER)
        .map(|(_, metadata)| metadata)
        .ok_or("curl: missing response metadata")?;
    let (status, redirect) = metadata
        .split_once('\n')
        .ok_or("curl: invalid response metadata")?;
    let status = status
        .parse::<u16>()
        .map_err(|_| "curl: invalid HTTP status")?;
    Ok((status, redirect.to_string(), body))
}

fn read_bounded_and_drain(
    reader: &mut impl Read,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::new();
    let mut overflow = false;
    let mut buffer = [0_u8; 8_192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok((retained, overflow));
        }
        let remaining = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
        overflow |= read > remaining;
    }
}

pub(crate) fn html_to_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_name = String::new();

    for c in html.chars() {
        if in_script || in_style {
            if c == '<' {
                tag_name.clear();
            } else if c == '>' {
                if tag_name == "/script" {
                    in_script = false;
                }
                if tag_name == "/style" {
                    in_style = false;
                }
            } else if tag_name.len() < 10 {
                tag_name.push(c);
            }
            continue;
        }

        match c {
            '<' => {
                in_tag = true;
                tag_name.clear();
            }
            '>' => {
                in_tag = false;
                let tn = tag_name.trim().to_lowercase();
                if tn == "script" || tn.starts_with("script ") {
                    in_script = true;
                }
                if tn == "style" || tn.starts_with("style ") {
                    in_style = true;
                }
                if tn == "br"
                    || tn == "br/"
                    || tn == "/p"
                    || tn == "/div"
                    || tn == "/tr"
                    || tn == "/li"
                    || tn == "/h1"
                    || tn == "/h2"
                    || tn == "/h3"
                    || tn == "/h4"
                {
                    text.push('\n');
                }
            }
            _ => {
                if !in_tag {
                    text.push(c);
                } else {
                    tag_name.push(c);
                }
            }
        }
    }

    // Collapse whitespace: preserve single spaces, fold consecutive runs, strip leading/trailing per line
    let mut result = String::new();
    let mut prev_was_space = false;
    let mut at_line_start = true;
    for c in text.chars() {
        if c == ' ' || c == '\t' {
            if !prev_was_space && !at_line_start {
                result.push(' ');
                prev_was_space = true;
            }
        } else if c == '\n' {
            result.push('\n');
            prev_was_space = false;
            at_line_start = true;
        } else {
            result.push(c);
            prev_was_space = false;
            at_line_start = false;
        }
    }

    let result = result.trim().to_string();
    if result.len() > 10000 {
        let mut end = 10000;
        while !result.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...\n[truncated at 10000 bytes]", &result[..end])
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_scripts() {
        let html = r#"
            <html><head><style>body{color:red}</style><script>alert(1)</script></head>
            <body><h1>Hello</h1><p>World</p><br/>Line2</body></html>
        "#;
        let text = html_to_text(html);
        assert!(text.contains("Hello"), "{text}");
        assert!(text.contains("World"), "{text}");
        assert!(!text.contains("alert"), "{text}");
        assert!(!text.contains("color:red"), "{text}");
    }

    #[test]
    fn collapses_whitespace() {
        let text = html_to_text("<p>a   b\t\tc</p>");
        assert!(text.contains("a b c") || text.contains("a b"), "{text}");
    }

    #[test]
    fn requires_url() {
        assert!(WebFetchTool.execute("{}").is_err());
        assert!(WebFetchTool.execute("not-json").is_err());
    }

    #[test]
    fn rejects_non_http_and_unsafe_urls() {
        for url in [
            "file:///etc/passwd",
            "gopher://example.com/",
            "example.com",
            "http://user:password@example.com/",
            "http://127.0.0.1/",
            "http://127.1/",
            "http://2130706433/",
            "http://0177.0.0.1/",
            "http://0x7f000001/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://example.com:0/",
            "http://example.com:65536/",
            "http://example.com/\n--output=/tmp/file",
        ] {
            assert!(validate_target(url).is_err(), "accepted unsafe URL: {url}");
        }
    }

    #[test]
    fn blocks_non_public_address_ranges() {
        for address in [
            "0.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.0.1",
            "192.168.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "2001::1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            let address = address.parse().unwrap();
            assert!(
                !is_public_address(address),
                "accepted unsafe address: {address}"
            );
        }
        assert!(is_public_address("93.184.216.34".parse().unwrap()));
        assert!(is_public_address("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn rejects_mixed_dns_answers_and_pins_validated_address() {
        let mixed = validate_target_with("https://example.com/", |host, port| {
            assert_eq!((host, port), ("example.com", 443));
            Ok(vec![
                "93.184.216.34".parse().unwrap(),
                "127.0.0.1".parse().unwrap(),
            ])
        });
        assert!(mixed.is_err());

        let target = validate_target_with("https://EXAMPLE.com:8443/path", |host, port| {
            assert_eq!((host, port), ("example.com", 8443));
            Ok(vec!["93.184.216.34".parse().unwrap()])
        })
        .unwrap();
        let args = curl_args(&target);
        assert!(args
            .windows(2)
            .any(|args| args == ["--resolve", "example.com:8443:93.184.216.34"]));
        assert_eq!(target.url, "https://example.com:8443/path");
    }

    #[test]
    fn hardens_curl_and_places_url_after_option_boundary() {
        let target = validate_target("https://93.184.216.34/path?[x]=1").unwrap();
        let args = curl_args(&target);
        assert_eq!(args.first().map(String::as_str), Some("-q"));
        assert!(args.iter().any(|arg| arg == "--globoff"));
        assert!(args.iter().any(|arg| arg == "--no-location"));
        assert!(args.iter().any(|arg| arg == "--max-time"));
        assert!(args.iter().any(|arg| arg == "--max-filesize"));
        assert!(!args.iter().any(|arg| arg == "-L" || arg == "--location"));
        assert_eq!(
            &args[args.len() - 2..],
            ["--", "https://93.184.216.34/path?[x]=1"]
        );
    }

    #[test]
    fn bounds_diagnostic_output_while_draining_input() {
        let input = vec![b'x'; 100];
        let (retained, overflow) = read_bounded_and_drain(&mut input.as_slice(), 16).unwrap();
        assert_eq!(retained, vec![b'x'; 16]);
        assert!(overflow);
    }

    #[test]
    fn truncates_very_long_text() {
        let long = format!("<p>{}</p>", "x".repeat(12_000));
        let text = html_to_text(&long);
        assert!(text.contains("truncated"), "{text}");
        assert!(text.len() < 12_000);
    }

    #[test]
    fn truncates_unicode_at_a_character_boundary() {
        let text = html_to_text(&format!("{}界🙂", "é".repeat(4_999)));
        assert!(text.contains("truncated"), "{text}");
        assert!(text.starts_with(&"é".repeat(4_999)), "{text}");
        assert!(!text.contains('界'), "{text}");
    }
}
