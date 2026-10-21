//! Reading files that come from other people: module files, package
//! manifests, panel art and patches.

use std::io::Read;
use std::path::Path;

/// Reads a text file of at most `max` bytes. Anything that isn't a regular
/// file (a device such as `/dev/zero`, or a pipe, symlinked in) is refused
/// before it's opened, and reading stops at the cap whatever size the file
/// reported, so a file that grows can't get past it either.
pub fn read_text_file(path: &Path, max: u64) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > max {
        return Err(format!("{} bytes is over the {max}-byte limit", meta.len()));
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.take(max + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > max {
        return Err(format!("over the {max}-byte limit"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_and_oversized_files_are_refused() {
        #[cfg(unix)]
        {
            let link = std::env::temp_dir().join(format!("dov-zero-{}", std::process::id()));
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
            assert_eq!(read_text_file(&link, 1024).unwrap_err(), "not a regular file");
            let _ = std::fs::remove_file(&link);
        }
        let file = std::env::temp_dir().join(format!("dov-big-{}", std::process::id()));
        std::fs::write(&file, "x".repeat(100)).unwrap();
        assert!(read_text_file(&file, 99).unwrap_err().contains("limit"));
        assert_eq!(read_text_file(&file, 100).unwrap().len(), 100);
        let _ = std::fs::remove_file(&file);
    }
}
