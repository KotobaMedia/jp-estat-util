use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub async fn unzip_archive(zip_path: &Path) -> Result<PathBuf> {
    let out_dir = zip_path.with_extension("");
    // Remove the output directory if it exists
    if out_dir.exists() {
        tokio::fs::remove_dir_all(&out_dir).await?;
    }
    let output = Command::new("unzip")
        .arg(zip_path)
        .arg("-d")
        .arg(&out_dir)
        .output()
        .await?;

    if !output.status.success() {
        eprintln!(
            "Failed to unzip: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Err(anyhow!("Failed to unzip"));
    }

    Ok(out_dir)
}

/// Finds the first file with the given extension in the specified directory.
/// Returns the path to the file if found.
pub async fn find_file_with_ext(dir: &Path, ext: &str) -> Result<PathBuf> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let entry = entry.path();
        if entry.extension().is_some_and(|e| e == ext) {
            return Ok(entry);
        }
    }
    Err(anyhow!("No .{} file found in the directory", ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_unzip_archive_and_find_shp() {
        let zip_path = PathBuf::from("./test/2000-31.zip");
        let out_dir = unzip_archive(&zip_path).await.unwrap();
        let shape_file = find_file_with_ext(&out_dir, "shp").await.unwrap();
        assert!(shape_file.exists());
        assert_eq!(shape_file.extension().unwrap(), "shp");
        assert_eq!(shape_file.file_stem().unwrap(), "h12ka31");

        tokio::fs::remove_dir_all(out_dir).await.unwrap();
    }
}
