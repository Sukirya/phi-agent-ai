use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use phi_kernel::ports::BlobPort;

use phi_kernel::fs_atomic::write_atomic;

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
        if content_ref.is_empty() || !content_ref.chars().all(|c| c.is_ascii_hexdigit()) {
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
        // A zero-length file is a crashed write; replace it instead of trusting
        // the hash-named path forever.
        let intact = fs::metadata(&path)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false);
        if !intact {
            write_atomic(&path, bytes)?;
        }
        Ok(content_ref)
    }

    fn get(&self, content_ref: &str) -> Result<Vec<u8>, String> {
        let path = self.path_for(content_ref)?;
        fs::read(path).map_err(|e| format!("read blob: {e}"))
    }
}
