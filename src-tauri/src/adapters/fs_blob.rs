use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::kernel::ports::BlobPort;

/// Content-addressed blob store: sha256(content) -> file.
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    pub fn new(root: PathBuf) -> Self {
        let _ = fs::create_dir_all(&root);
        Self { root }
    }

    fn path_for(&self, content_ref: &str) -> Result<PathBuf, String> {
        if content_ref.is_empty()
            || !content_ref.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err("invalid content ref".to_string());
        }
        Ok(self.root.join(format!("{content_ref}.blob")))
    }
}

impl BlobPort for FsBlobStore {
    fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let hash = Sha256::digest(bytes);
        let content_ref = hex::encode(hash);
        let path = self.path_for(&content_ref)?;
        if !path.exists() {
            fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
            fs::write(&path, bytes).map_err(|e| format!("write blob: {e}"))?;
        }
        Ok(content_ref)
    }

    fn get(&self, content_ref: &str) -> Result<Vec<u8>, String> {
        let path = self.path_for(content_ref)?;
        fs::read(path).map_err(|e| format!("read blob: {e}"))
    }
}
