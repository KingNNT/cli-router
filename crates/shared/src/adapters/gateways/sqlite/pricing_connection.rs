use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::adapters::errors::AdapterError;
use crate::adapters::gateways::sqlite::connection;

pub fn default_pricing_db_path() -> PathBuf {
    let home = connection::home_dir_public();
    home.join(".local/share/cli-router/pricing.db")
}

pub fn open_writable(path: &Path) -> Result<Connection, AdapterError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AdapterError::DataMapping(format!("cannot create {}: {}", parent.display(), e))
        })?;
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(AdapterError::from)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_fresh_file_and_parent_dir() {
        let tmp = std::env::temp_dir().join("cli-router-test-pricing");
        let _ = std::fs::remove_dir_all(&tmp);
        let path = tmp.join("nested/pricing.db");
        let conn = open_writable(&path).unwrap();
        drop(conn);
        assert!(path.exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
