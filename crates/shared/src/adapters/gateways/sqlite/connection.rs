use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::adapters::errors::AdapterError;

pub fn default_db_path() -> PathBuf {
    home_dir().join(".local/share/opencode/opencode.db")
}

pub(crate) fn home_dir_public() -> PathBuf {
    home_dir()
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

pub fn open_readonly(path: &Path) -> Result<Connection, AdapterError> {
    if !path.exists() {
        return Err(AdapterError::DataMapping(format!(
            "Database not found at: {}",
            path.display()
        )));
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_missing_path_errors() {
        let err = open_readonly(Path::new("/nonexistent/xyz.db")).unwrap_err();
        assert!(matches!(err, AdapterError::DataMapping(_)));
    }
}
