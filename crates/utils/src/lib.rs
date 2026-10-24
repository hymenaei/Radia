use std::{
    io::{self, Write},
    path::Path,
};

use tempfile::NamedTempFile;
use uuid::Uuid;

macro_rules! ids {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
            pub struct $name(Uuid);

            impl $name {
                pub const fn new(uuid: Uuid) -> Self {
                    Self(uuid)
                }

                pub const fn uuid(self) -> Uuid {
                    self.0
                }
            }
        )+
    };
}

ids!(AssetId, AvatarId, FolderId, GroupId, ItemId, ObjectId, ParcelId, RegionId);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExperienceId(Uuid);

impl ExperienceId {
    pub fn new(uuid: Uuid) -> Option<Self> {
        (!uuid.is_nil()).then_some(Self(uuid))
    }

    pub const fn uuid(self) -> Uuid {
        self.0
    }
}

pub fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(contents)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn hex_preserves_byte_pairs() {
        for (bytes, expected) in [(&[][..], ""), (&[0, 1, 15, 16, 127, 128, 255][..], "00010f107f80ff"), (b"Radia", "5261646961")] {
            assert_eq!(hex_lower(bytes), expected, "{bytes:?}");
        }
    }

    #[test]
    fn atomic_write_replaces_contents() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        write_atomic(&path, b"original").expect("create file");
        write_atomic(&path, b"replacement").expect("replace file");

        assert_eq!(fs::read(&path).expect("read replacement"), b"replacement");
        assert_eq!(fs::read_dir(directory.path()).expect("read directory").count(), 1, "temporary files must be removed");
    }

    #[test]
    fn failed_write_preserves_destination() {
        let directory = tempdir().expect("temporary directory");
        let destination = directory.path().join("existing");
        fs::create_dir(&destination).expect("create destination directory");
        fs::write(destination.join("original"), b"keep").expect("write original data");

        assert!(write_atomic(&destination, b"replacement").is_err(), "replacing a directory must fail");
        assert_eq!(fs::read(destination.join("original")).expect("read original data"), b"keep");
        assert_eq!(fs::read_dir(directory.path()).expect("read directory").count(), 1, "failed writes must remove temporary files");
    }
}
