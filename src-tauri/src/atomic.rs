//! Atomic file writes: write to a temp file in the same directory, flush, then rename over the target.
//! A crash leaves either the old file or the new one, never a half-written one.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Write `bytes` to `target` atomically, creating parent directories. Never logs the content.
pub fn write(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = target
        .parent()
        .ok_or_else(|| "target has no parent directory".to_string())?;
    fs::create_dir_all(dir).map_err(|e| format!("create dir failed: {e}"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| format!("temp file failed: {e}"))?;
    tmp.write_all(bytes).map_err(|e| format!("write failed: {e}"))?;
    tmp.flush().map_err(|e| format!("flush failed: {e}"))?;
    tmp.as_file().sync_all().map_err(|e| format!("sync failed: {e}"))?;
    tmp.persist(target).map_err(|e| format!("rename failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrips_and_creates_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested").join("a.json");
        write(&target, b"hello").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");
        // Overwrite leaves only the new content, no temp strays.
        write(&target, b"world").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"world");
        let strays: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "a.json")
            .collect();
        assert!(strays.is_empty(), "temp files left behind");
    }
}
