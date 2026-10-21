use network::Grid;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use serde_json::{from_str, to_string};

#[cfg(target_os = "windows")]
const SERVICE: &str = "radia.login";

#[derive(Clone, Deserialize, Serialize)]
pub struct RememberedLogin {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub mfa_hash: String,
}

#[cfg(target_os = "windows")]
pub fn load(grid: Grid) -> Result<Option<RememberedLogin>, String> {
    let entry =
        keyring::Entry::new(SERVICE, grid_key(grid)).map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;

    match entry.get_password() {
        Ok(value) => from_str(&value).map(Some).map_err(|error| format!("Saved login data is invalid: {error}")),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("Could not read saved login: {error}")),
    }
}

#[cfg(target_os = "windows")]
pub fn save(grid: Grid, login: &RememberedLogin) -> Result<(), String> {
    let entry =
        keyring::Entry::new(SERVICE, grid_key(grid)).map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;
    let value = to_string(login).map_err(|error| format!("Could not encode saved login: {error}"))?;
    entry.set_password(&value).map_err(|error| format!("Could not save login to Windows Credential Manager: {error}"))
}

#[cfg(target_os = "windows")]
pub fn clear(grid: Grid) -> Result<(), String> {
    let entry =
        keyring::Entry::new(SERVICE, grid_key(grid)).map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not remove saved login: {error}")),
    }
}

#[cfg(target_os = "windows")]
fn grid_key(grid: Grid) -> &'static str {
    match grid {
        Grid::Agni => "agni",
        Grid::Aditi => "aditi",
    }
}

#[cfg(not(target_os = "windows"))]
pub fn load(_: Grid) -> Result<Option<RememberedLogin>, String> {
    Ok(None)
}

#[cfg(not(target_os = "windows"))]
pub fn save(_: Grid, _: &RememberedLogin) -> Result<(), String> {
    Err("Secure login storage is available on Windows only.".into())
}

#[cfg(not(target_os = "windows"))]
pub fn clear(_: Grid) -> Result<(), String> {
    Ok(())
}
