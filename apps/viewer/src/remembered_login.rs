use std::fmt::{self, Display, Formatter};

use network::Grid;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string};

const SERVICE: &str = "radia.login";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CredentialStoreError {
    Open(String),
    Read(String),
    InvalidSavedData(String),
    Encode(String),
    Save(String),
    Remove(String),
}

impl Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(detail) => write!(formatter, "Could not open the system credential store: {detail}"),
            Self::Read(detail) => write!(formatter, "Could not read saved login: {detail}"),
            Self::InvalidSavedData(detail) => write!(formatter, "Saved login data is invalid: {detail}"),
            Self::Encode(detail) => write!(formatter, "Could not encode saved login: {detail}"),
            Self::Save(detail) => write!(formatter, "Could not save login to the system credential store: {detail}"),
            Self::Remove(detail) => write!(formatter, "Could not remove saved login: {detail}"),
        }
    }
}

impl std::error::Error for CredentialStoreError {}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct RememberedLogin {
    pub(crate) username: String,
    pub(crate) password: String,
    #[serde(default)]
    pub(crate) mfa_hash: String,
}

pub(crate) fn load(grid: Grid) -> Result<Option<RememberedLogin>, CredentialStoreError> {
    let entry = keyring::Entry::new(SERVICE, grid_key(grid)).map_err(|error| CredentialStoreError::Open(error.to_string()))?;

    match entry.get_password() {
        Ok(value) => from_str(&value).map(Some).map_err(|error| CredentialStoreError::InvalidSavedData(error.to_string())),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(CredentialStoreError::Read(error.to_string())),
    }
}

pub(crate) fn save(grid: Grid, login: &RememberedLogin) -> Result<(), CredentialStoreError> {
    let entry = keyring::Entry::new(SERVICE, grid_key(grid)).map_err(|error| CredentialStoreError::Open(error.to_string()))?;
    let value = to_string(login).map_err(|error| CredentialStoreError::Encode(error.to_string()))?;
    entry.set_password(&value).map_err(|error| CredentialStoreError::Save(error.to_string()))
}

pub(crate) fn clear(grid: Grid) -> Result<(), CredentialStoreError> {
    let entry = keyring::Entry::new(SERVICE, grid_key(grid)).map_err(|error| CredentialStoreError::Open(error.to_string()))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(CredentialStoreError::Remove(error.to_string())),
    }
}

fn grid_key(grid: Grid) -> &'static str {
    match grid {
        Grid::Agni => "agni",
        Grid::Aditi => "aditi",
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use network::Grid;

    use super::{CredentialStoreError, load};

    struct FailingCredentialBuilder;

    impl keyring::credential::CredentialBuilderApi for FailingCredentialBuilder {
        fn build(&self, _: Option<&str>, _: &str, _: &str) -> keyring::Result<Box<keyring::Credential>> {
            Err(keyring::Error::Invalid("keyring".into(), "unavailable".into()))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn keyring_failure_is_typed() {
        keyring::set_default_credential_builder(Box::new(FailingCredentialBuilder));

        let error = match load(Grid::Agni) {
            Err(error) => error,
            Ok(_) => panic!("keyring access failure must be reported"),
        };

        std::assert_matches!(&error, CredentialStoreError::Open(_));
        assert!(error.to_string().starts_with("Could not open the system credential store:"));
    }
}
