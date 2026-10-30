use std::{
    io::{self, Write},
    path::Path,
    thread,
    time::Duration,
};

use tempfile::NamedTempFile;
use uuid::Uuid;

const PERSIST_ATTEMPTS: usize = 4;
const PERSIST_RETRY_DELAY: Duration = Duration::from_millis(10);
const PERSIST_RETRY_BUDGET: Duration = Duration::from_millis(70);

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

pub struct PersistBudget {
    remaining: Duration,
}

impl Default for PersistBudget {
    fn default() -> Self {
        Self { remaining: PERSIST_RETRY_BUDGET }
    }
}

impl PersistBudget {
    pub fn persist(&mut self, temporary: NamedTempFile, destination: &Path) -> io::Result<()> {
        self.persist_with_retry(temporary, destination, thread::sleep)
    }

    fn persist_with_retry(
        &mut self,
        mut temporary: NamedTempFile,
        destination: &Path,
        mut wait_retry: impl FnMut(Duration),
    ) -> io::Result<()> {
        for attempt in 0..PERSIST_ATTEMPTS {
            match temporary.persist(destination) {
                Ok(file) => {
                    drop(file);
                    return Ok(());
                }
                Err(failure) => {
                    if failure.error.kind() != io::ErrorKind::PermissionDenied
                        || attempt + 1 == PERSIST_ATTEMPTS
                        || self.remaining.is_zero()
                    {
                        return Err(failure.error);
                    }
                    temporary = failure.file;
                    let delay = (PERSIST_RETRY_DELAY * (1 << attempt)).min(self.remaining);
                    self.remaining -= delay;
                    wait_retry(delay);
                }
            }
        }
        unreachable!("persistence returns on its final attempt")
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
    PersistBudget::default().persist(temporary, path)
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

    #[cfg(windows)]
    #[test]
    fn replacement_survives_transient_lock() {
        use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};

        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        fs::write(&path, b"original").expect("create destination");
        let mut held = Some(OpenOptions::new().read(true).share_mode(0).open(&path).expect("lock destination"));
        let mut temporary = NamedTempFile::new_in(directory.path()).expect("stage replacement");
        temporary.write_all(b"replacement").expect("write replacement");

        PersistBudget::default()
            .persist_with_retry(temporary, &path, |_| drop(held.take()))
            .expect("replace after the conflicting handle closes");

        assert_eq!(fs::read(&path).expect("read replacement"), b"replacement");
        assert_eq!(fs::read_dir(directory.path()).expect("read directory").count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn locked_destination_keeps_contents() {
        use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};

        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        fs::write(&path, b"original").expect("create destination");
        let held = OpenOptions::new().read(true).share_mode(0).open(&path).expect("lock destination");

        let error = write_atomic(&path, b"replacement").expect_err("a persistent sharing conflict must fail");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        drop(held);
        assert_eq!(fs::read(&path).expect("read original"), b"original");
        assert_eq!(fs::read_dir(directory.path()).expect("read directory").count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn retry_budget_is_shared() {
        use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};

        let directory = tempdir().expect("temporary directory");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        fs::write(&first, b"original").expect("create first destination");
        fs::write(&second, b"original").expect("create second destination");
        let mut held = Some(OpenOptions::new().read(true).share_mode(0).open(&first).expect("lock first destination"));
        let mut temporary = NamedTempFile::new_in(directory.path()).expect("stage first replacement");
        temporary.write_all(b"replacement").expect("write replacement");
        let mut budget = PersistBudget::default();
        let mut waited = Duration::ZERO;
        budget
            .persist_with_retry(temporary, &first, |delay| {
                waited += delay;
                if waited == PERSIST_RETRY_BUDGET {
                    drop(held.take());
                }
            })
            .expect("replace first file after consuming the budget");
        assert_eq!(waited, PERSIST_RETRY_BUDGET);
        assert_eq!(fs::read(&first).expect("read first replacement"), b"replacement");

        let held = OpenOptions::new().read(true).share_mode(0).open(&second).expect("lock second destination");
        let temporary = NamedTempFile::new_in(directory.path()).expect("stage second replacement");
        let error = budget
            .persist_with_retry(temporary, &second, |_| panic!("the shared retry budget is exhausted"))
            .expect_err("second failure cannot wait again");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        drop(held);
        assert_eq!(fs::read(&second).expect("read second original"), b"original");
        assert_eq!(fs::read_dir(directory.path()).expect("read directory").count(), 2);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "manual Windows replacement stress; run with --run-ignored only"]
    fn atomic_replacement_stress() {
        for _ in 0..10_000 {
            atomic_write_replaces_contents();
        }
    }
}
