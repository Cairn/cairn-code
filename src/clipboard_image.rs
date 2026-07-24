//! Read an image from the OS clipboard (platform-specific helpers).
//!
//! Used by the TUI paste bindings:
//! - Windows: Alt+V
//! - Linux: Ctrl+V (when clipboard holds an image)
//! - macOS: Cmd+V (SUPER+V)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::llm::ImageBlock;

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push(T[(n & 63) as usize] as char);
        i += 3;
    }
    match data.len() - i {
        1 => {
            let n = (data[i] as u32) << 16;
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push(T[((n >> 6) & 63) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Maximum raw image bytes accepted from the clipboard (~8 MiB).
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ClipboardImage {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

impl ClipboardImage {
    pub fn into_block(self) -> ImageBlock {
        ImageBlock {
            media_type: self.media_type,
            data_base64: base64_encode(&self.bytes),
        }
    }
}

/// Try to pull a PNG/JPEG/GIF/WebP image from the system clipboard.
pub fn read_clipboard_image() -> Result<ClipboardImage, String> {
    #[cfg(windows)]
    {
        return read_windows();
    }
    #[cfg(target_os = "macos")]
    {
        return read_macos();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return read_linux();
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err("clipboard image paste is not supported on this platform".into())
    }
}

fn temp_clip_path(ext: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "cairn-clip-img-{}-{}.{}",
        std::process::id(),
        nanos,
        ext
    ))
}

fn load_image_file(path: &Path) -> Result<ClipboardImage, String> {
    let meta = fs::metadata(path).map_err(|e| format!("clipboard image stat: {e}"))?;
    if meta.len() == 0 {
        return Err("clipboard image is empty".into());
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "clipboard image is too large ({} bytes; max {MAX_IMAGE_BYTES})",
            meta.len()
        ));
    }
    let bytes = fs::read(path).map_err(|e| format!("read clipboard image: {e}"))?;
    let media_type = sniff_media_type(&bytes)
        .ok_or_else(|| "clipboard data is not a supported image (png/jpeg/gif/webp)".to_string())?
        .to_string();
    Ok(ClipboardImage { media_type, bytes })
}

fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Some("image/jpeg");
    }
    if bytes.len() >= 6
        && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
    {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

#[cfg(windows)]
fn read_windows() -> Result<ClipboardImage, String> {
    let path = temp_clip_path("png");
    let path_str = path.to_string_lossy().replace('\'', "''");
    // System.Windows.Forms.Clipboard.GetImage() returns a bitmap when the user
    // copied a screenshot or image; also try file-drop lists of image paths.
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$out = '{path_str}'
$img = [System.Windows.Forms.Clipboard]::GetImage()
if ($null -ne $img) {{
  $img.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
  $img.Dispose()
  exit 0
}}
$files = [System.Windows.Forms.Clipboard]::GetFileDropList()
if ($null -ne $files) {{
  foreach ($f in $files) {{
    if (Test-Path -LiteralPath $f) {{
      $ext = [System.IO.Path]::GetExtension($f).ToLowerInvariant()
      if ($ext -in @('.png','.jpg','.jpeg','.gif','.webp','.bmp')) {{
        Copy-Item -LiteralPath $f -Destination $out -Force
        exit 0
      }}
    }}
  }}
}}
exit 2
"#
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("powershell clipboard: {e}"))?;
    let result = if output.status.success() {
        load_image_file(&path)
    } else if output.status.code() == Some(2) {
        Err("no image on the clipboard".into())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "clipboard image failed ({}): {}",
            output.status,
            err.trim()
        ))
    };
    let _ = fs::remove_file(&path);
    result
}

#[cfg(target_os = "macos")]
fn read_macos() -> Result<ClipboardImage, String> {
    // Prefer pngpaste when installed; fall back to osascript PNGf class.
    let path = temp_clip_path("png");
    if Command::new("pngpaste")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        let result = load_image_file(&path);
        let _ = fs::remove_file(&path);
        return result;
    }

    let path_str = path.to_string_lossy();
    let script = format!(
        r#"try
  set png_data to the clipboard as «class PNGf»
  set out_path to POSIX file "{path_str}"
  set fref to open for access out_path with write permission
  set eof fref to 0
  write png_data to fref
  close access fref
on error errMsg number errNum
  try
    close access out_path
  end try
  error errMsg number errNum
end try"#
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("osascript clipboard: {e}"))?;
    let result = if output.status.success() {
        load_image_file(&path)
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.to_ascii_lowercase().contains("can't get")
            || err.to_ascii_lowercase().contains("error")
        {
            Err("no image on the clipboard".into())
        } else {
            Err(format!(
                "clipboard image failed: {}",
                err.trim().if_empty("unknown error")
            ))
        }
    };
    let _ = fs::remove_file(&path);
    result
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_linux() -> Result<ClipboardImage, String> {
    // Wayland first, then X11.
    if let Ok(img) = read_via_cmd(
        "wl-paste",
        &["--no-newline", "--type", "image/png"],
        "image/png",
    ) {
        return Ok(img);
    }
    if let Ok(img) = read_via_cmd(
        "wl-paste",
        &["--no-newline", "--type", "image/jpeg"],
        "image/jpeg",
    ) {
        return Ok(img);
    }
    if let Ok(img) = read_via_cmd(
        "xclip",
        &["-selection", "clipboard", "-t", "image/png", "-o"],
        "image/png",
    ) {
        return Ok(img);
    }
    if let Ok(img) = read_via_cmd(
        "xclip",
        &["-selection", "clipboard", "-t", "image/jpeg", "-o"],
        "image/jpeg",
    ) {
        return Ok(img);
    }
    if let Ok(img) = read_via_cmd(
        "xsel",
        &["--clipboard", "--output", "--mime-type", "image/png"],
        "image/png",
    ) {
        return Ok(img);
    }
    Err(
        "no image on the clipboard (install wl-paste or xclip for image paste support)".into(),
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_via_cmd(bin: &str, args: &[&str], media_type: &str) -> Result<ClipboardImage, String> {
    let output = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{bin}: {e}"))?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(format!("{bin}: no image data"));
    }
    if output.stdout.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "clipboard image is too large ({} bytes; max {MAX_IMAGE_BYTES})",
            output.stdout.len()
        ));
    }
    let media = sniff_media_type(&output.stdout)
        .unwrap_or(media_type)
        .to_string();
    Ok(ClipboardImage {
        media_type: media,
        bytes: output.stdout,
    })
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for &str {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_png_jpeg_gif_webp() {
        let png = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0];
        assert_eq!(sniff_media_type(&png), Some("image/png"));
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 0];
        assert_eq!(sniff_media_type(&jpeg), Some("image/jpeg"));
        let gif = b"GIF89a......";
        assert_eq!(sniff_media_type(gif), Some("image/gif"));
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.extend_from_slice(&[0; 4]);
        assert_eq!(sniff_media_type(&webp), Some("image/webp"));
        assert_eq!(sniff_media_type(b"not-an-image"), None);
    }

    #[test]
    fn into_block_base64() {
        let img = ClipboardImage {
            media_type: "image/png".into(),
            bytes: b"hi".to_vec(),
        };
        let block = img.into_block();
        assert_eq!(block.media_type, "image/png");
        assert_eq!(block.data_base64, base64_encode(b"hi"));
    }
}
