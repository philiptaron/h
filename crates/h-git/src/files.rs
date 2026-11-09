//! Writing h's own files, such as those it keeps in the store.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// Replace the file at `path` with `bytes`, making its directory if need be. The bytes go beside
/// it first and are renamed over it, so no reader ever sees half of them.
pub fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut temp = path.as_os_str().to_owned().into_vec();
    temp.extend_from_slice(format!(".{}", std::process::id()).as_bytes());
    let temp = PathBuf::from(OsString::from_vec(temp));
    let written = fs::File::create(&temp).and_then(|mut file| file.write_all(bytes));
    match written.and_then(|()| fs::rename(&temp, path)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_files_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/file");
        replace_file(&path, b"one").unwrap();
        replace_file(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(
            fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1,
            "nothing left beside it"
        );
    }
}
