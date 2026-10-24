use std::{
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

use network::Grid;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_vec_pretty};
use utils::write_atomic;
use uuid::Uuid;

use super::{FavoriteLocation, favorites::FavoritesError};

const APP_DIRECTORY: &str = "Radia";
const LOGIN_LOCATIONS_FILE: &str = "login_locations.json";
const MAX_FAVORITE_STORE_BYTES: usize = 4 * 1024 * 1024;
const MAX_STORED_AGENTS: usize = 256;
pub(super) const MAX_FAVORITES_PER_AGENT: usize = 512;
const MAX_USERNAME_BYTES: usize = 256;
const MAX_FAVORITE_NAME_BYTES: usize = 512;
const MAX_REGION_NAME_BYTES: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FavoriteStoreError {
    DataDirectoryUnavailable,
    Read { path: PathBuf, detail: String },
    FileTooLarge { path: PathBuf },
    InvalidFile { path: PathBuf, detail: String },
    InvalidStoredValues { path: PathBuf, reason: &'static str },
    InvalidValuesForSave(&'static str),
    InvalidRefreshedValues(&'static str),
    MissingParentDirectory,
    CreateDirectory { path: PathBuf, detail: String },
    Encode(String),
    SerializedDataTooLarge,
    RefreshedStoreTooLarge,
    Save { path: PathBuf, detail: String },
    Favorites(FavoritesError),
}

impl Display for FavoriteStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::DataDirectoryUnavailable => formatter.write_str("Could not find the roaming application-data directory."),
            Self::Read { path, detail } => write!(formatter, "Could not read login locations from {}: {detail}", path.display()),
            Self::FileTooLarge { path } => write!(formatter, "Saved login locations at {} exceed the 4 MiB limit.", path.display()),
            Self::InvalidFile { path, detail } => write!(formatter, "Saved login locations at {} are invalid: {detail}", path.display()),
            Self::InvalidStoredValues { path, reason } => {
                write!(formatter, "Saved login locations at {} are invalid: {reason}.", path.display())
            }
            Self::InvalidValuesForSave(reason) => write!(formatter, "Could not save login locations: {reason}."),
            Self::InvalidRefreshedValues(reason) => {
                write!(formatter, "Second Life returned favorite landmarks that cannot be stored: {reason}.")
            }
            Self::MissingParentDirectory => formatter.write_str("Login locations need a parent directory."),
            Self::CreateDirectory { path, detail } => write!(formatter, "Could not create {}: {detail}", path.display()),
            Self::Encode(detail) => write!(formatter, "Could not encode favorite landmarks: {detail}"),
            Self::SerializedDataTooLarge => formatter.write_str("Login locations exceed the 4 MiB storage limit."),
            Self::RefreshedStoreTooLarge => formatter.write_str("Favorite refresh exceeds the 4 MiB login-locations storage limit."),
            Self::Save { path, detail } => write!(formatter, "Could not save login locations to {}: {detail}", path.display()),
            Self::Favorites(error) => Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for FavoriteStoreError {}

enum StorageError {
    InvalidValues(&'static str),
    Encode(String),
    TooLarge,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FavoriteStore {
    #[serde(default)]
    agents: Vec<AgentFavorites>,
}

impl FavoriteStore {
    fn validate(&self) -> Result<(), &'static str> {
        if self.agents.len() > MAX_STORED_AGENTS {
            return Err("too many saved accounts");
        }

        for agent in &self.agents {
            validate_agent(&agent.username, &agent.favorites)?;
        }

        Ok(())
    }

    fn encode_for_storage(&self) -> Result<Vec<u8>, StorageError> {
        self.validate().map_err(StorageError::InvalidValues)?;
        let contents = to_vec_pretty(self).map_err(|error| StorageError::Encode(error.to_string()))?;
        if contents.len() > MAX_FAVORITE_STORE_BYTES {
            return Err(StorageError::TooLarge);
        }
        Ok(contents)
    }

    pub fn favorites_for(&self, grid: Grid, agent_id: Uuid) -> &[FavoriteLocation] {
        let grid = StoredGrid::from(grid);
        self.agents
            .iter()
            .find(|agent| agent.grid == grid && agent.agent_id == agent_id)
            .map(|agent| agent.favorites.as_slice())
            .unwrap_or_default()
    }

    pub fn agent_id_for_username(&self, grid: Grid, username: &str) -> Option<Uuid> {
        let grid = StoredGrid::from(grid);
        let username = normalize_username(username);
        if username.is_empty() {
            return None;
        }
        self.agents.iter().find(|agent| agent.grid == grid && agent.username == username).map(|agent| agent.agent_id)
    }

    pub fn remember_agent(&mut self, grid: Grid, agent_id: Uuid, username: &str) -> bool {
        let grid_key = StoredGrid::from(grid);
        let username = normalize_username(username);
        if username.is_empty() {
            return false;
        }

        let mut changed = false;
        for agent in &mut self.agents {
            if agent.grid == grid_key && agent.agent_id != agent_id && agent.username == username {
                agent.username.clear();
                changed = true;
            }
        }

        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.grid == grid_key && agent.agent_id == agent_id) {
            if agent.username == username {
                return changed;
            }
            agent.username = username;
            changed = true;
        } else {
            self.agents.push(AgentFavorites { grid: grid_key, agent_id, username, favorites: Vec::new() });
            changed = true;
        }
        changed
    }

    pub fn replace_favorites(&mut self, grid: Grid, agent_id: Uuid, username: &str, favorites: Vec<FavoriteLocation>) {
        let grid_key = StoredGrid::from(grid);
        self.remember_agent(grid, agent_id, username);
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.grid == grid_key && agent.agent_id == agent_id) {
            agent.favorites = favorites;
        }
    }
}

fn validate_agent(username: &str, favorites: &[FavoriteLocation]) -> Result<(), &'static str> {
    if username.len() > MAX_USERNAME_BYTES {
        return Err("a saved username is too long");
    }
    validate_favorites(favorites)
}

fn validate_favorites(favorites: &[FavoriteLocation]) -> Result<(), &'static str> {
    if favorites.len() > MAX_FAVORITES_PER_AGENT {
        return Err("an account has too many saved favorites");
    }
    if favorites.iter().any(|favorite| favorite.name.len() > MAX_FAVORITE_NAME_BYTES || favorite.region_name.len() > MAX_REGION_NAME_BYTES)
    {
        return Err("a saved favorite name is too long");
    }

    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AgentFavorites {
    grid: StoredGrid,
    agent_id: Uuid,
    username: String,
    #[serde(default)]
    favorites: Vec<FavoriteLocation>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum StoredGrid {
    Agni,
    Aditi,
}

impl From<Grid> for StoredGrid {
    fn from(grid: Grid) -> Self {
        match grid {
            Grid::Agni => Self::Agni,
            Grid::Aditi => Self::Aditi,
        }
    }
}

pub(crate) fn apply_favorites_refresh(
    current_favorites: &mut Vec<FavoriteLocation>,
    store: &mut FavoriteStore,
    grid: Grid,
    agent_id: Uuid,
    username: &str,
    refresh: Result<Vec<FavoriteLocation>, FavoritesError>,
) -> Result<(), FavoriteStoreError> {
    let favorites = refresh.map_err(FavoriteStoreError::Favorites)?;
    validate_favorites(&favorites).map_err(FavoriteStoreError::InvalidRefreshedValues)?;
    let mut next_store = store.clone();
    next_store.replace_favorites(grid, agent_id, username, favorites.clone());
    next_store.encode_for_storage().map_err(|error| match error {
        StorageError::InvalidValues(reason) => FavoriteStoreError::InvalidRefreshedValues(reason),
        StorageError::Encode(detail) => FavoriteStoreError::Encode(detail),
        StorageError::TooLarge => FavoriteStoreError::RefreshedStoreTooLarge,
    })?;
    *store = next_store;
    *current_favorites = favorites;
    Ok(())
}

pub(crate) fn load() -> Result<FavoriteStore, FavoriteStoreError> {
    load_from(&login_locations_path()?)
}

pub(crate) fn save(favorites: &FavoriteStore) -> Result<(), FavoriteStoreError> {
    save_to(&login_locations_path()?, favorites)
}

fn login_locations_path() -> Result<PathBuf, FavoriteStoreError> {
    let roaming_dir = dirs::data_dir().ok_or(FavoriteStoreError::DataDirectoryUnavailable)?;
    Ok(login_locations_path_under(&roaming_dir))
}

fn login_locations_path_under(data_dir: &Path) -> PathBuf {
    data_dir.join(APP_DIRECTORY).join(LOGIN_LOCATIONS_FILE)
}

fn normalize_username(username: &str) -> String {
    let mut names = username.split_whitespace();
    let Some(first_name) = names.next() else {
        return String::new();
    };
    let last_name = names.collect::<Vec<_>>().join(" ");
    if last_name.is_empty() { format!("{first_name} resident").to_lowercase() } else { format!("{first_name} {last_name}").to_lowercase() }
}

fn load_from(path: &Path) -> Result<FavoriteStore, FavoriteStoreError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(FavoriteStore::default()),
        Err(error) => return Err(FavoriteStoreError::Read { path: path.to_owned(), detail: error.to_string() }),
    };

    let mut bytes = Vec::new();
    file.take((MAX_FAVORITE_STORE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| FavoriteStoreError::Read { path: path.to_owned(), detail: error.to_string() })?;
    if bytes.len() > MAX_FAVORITE_STORE_BYTES {
        return Err(FavoriteStoreError::FileTooLarge { path: path.to_owned() });
    }

    let contents =
        String::from_utf8(bytes).map_err(|error| FavoriteStoreError::InvalidFile { path: path.to_owned(), detail: error.to_string() })?;
    let favorites: FavoriteStore =
        from_str(&contents).map_err(|error| FavoriteStoreError::InvalidFile { path: path.to_owned(), detail: error.to_string() })?;
    favorites.validate().map_err(|reason| FavoriteStoreError::InvalidStoredValues { path: path.to_owned(), reason })?;
    Ok(favorites)
}

fn save_to(path: &Path, favorites: &FavoriteStore) -> Result<(), FavoriteStoreError> {
    let contents = favorites.encode_for_storage().map_err(|error| match error {
        StorageError::InvalidValues(reason) => FavoriteStoreError::InvalidValuesForSave(reason),
        StorageError::Encode(detail) => FavoriteStoreError::Encode(detail),
        StorageError::TooLarge => FavoriteStoreError::SerializedDataTooLarge,
    })?;
    let directory = path.parent().ok_or(FavoriteStoreError::MissingParentDirectory)?;
    fs::create_dir_all(directory)
        .map_err(|error| FavoriteStoreError::CreateDirectory { path: directory.to_owned(), detail: error.to_string() })?;

    write_atomic(path, &contents).map_err(|error| FavoriteStoreError::Save { path: path.to_owned(), detail: error.to_string() })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use network::Grid;
    use tempfile::tempdir;
    use uuid::Uuid;

    use crate::inventory::{FavoriteLocation, favorites::FavoritesError};

    use super::{
        AgentFavorites, FavoriteStore, FavoriteStoreError, MAX_FAVORITE_STORE_BYTES, MAX_FAVORITES_PER_AGENT, StoredGrid,
        apply_favorites_refresh, load_from, login_locations_path_under, save_to,
    };

    #[test]
    fn store_round_trips_favorites_by_grid_and_agent() {
        let test_directory = tempdir().expect("create temporary login locations directory");
        let path = login_locations_path_under(test_directory.path());
        let mut favorites = FavoriteStore::default();
        let agni_agent_id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let aditi_agent_id = Uuid::from_u128(0xfedc_ba98_7654_3210_fedc_ba98_7654_3210);
        let agni_favorites = vec![FavoriteLocation { name: "Sandbox".into(), region_name: "Test Region".into(), x: 128, y: 64, z: 23 }];
        let aditi_favorites = vec![FavoriteLocation { name: "Home".into(), region_name: "Beta Region".into(), x: 1, y: 2, z: 3 }];

        favorites.replace_favorites(Grid::Agni, agni_agent_id, "Example", agni_favorites.clone());
        favorites.replace_favorites(Grid::Aditi, aditi_agent_id, "Example Resident", aditi_favorites.clone());
        save_to(&path, &favorites).expect("save favorites");

        let reloaded = load_from(&path).expect("reload login locations");
        assert_eq!(reloaded.favorites_for(Grid::Agni, agni_agent_id), agni_favorites);
        assert_eq!(reloaded.favorites_for(Grid::Aditi, aditi_agent_id), aditi_favorites);
    }

    #[test]
    fn failed_favorite_refresh_keeps_memory_and_disk_snapshots() {
        let test_directory = tempdir().expect("create temporary login locations directory");
        let path = login_locations_path_under(test_directory.path());
        let agent_id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let saved = vec![FavoriteLocation { name: "Home".into(), region_name: "Ahern".into(), x: 1, y: 2, z: 3 }];
        let mut store = FavoriteStore::default();
        store.replace_favorites(Grid::Agni, agent_id, "Example Resident", saved.clone());
        save_to(&path, &store).expect("save known-good favorite snapshot");
        let mut current_favorites = saved.clone();

        let result = apply_favorites_refresh(
            &mut current_favorites,
            &mut store,
            Grid::Agni,
            agent_id,
            "Example Resident",
            Err(FavoritesError::RegionLookup("region lookup failed".into())),
        );

        assert!(matches!(result, Err(FavoriteStoreError::Favorites(FavoritesError::RegionLookup(_)))));
        assert_eq!(current_favorites, saved);
        assert_eq!(store.favorites_for(Grid::Agni, agent_id), saved);
        assert_eq!(load_from(&path).expect("reload known-good snapshot").favorites_for(Grid::Agni, agent_id), saved);

        apply_favorites_refresh(&mut current_favorites, &mut store, Grid::Agni, agent_id, "Example Resident", Ok(Vec::new()))
            .expect("an empty successful refresh is authoritative");
        assert!(current_favorites.is_empty());
        assert!(store.favorites_for(Grid::Agni, agent_id).is_empty());
        save_to(&path, &store).expect("persist successful empty snapshot");
        assert!(load_from(&path).expect("reload empty snapshot").favorites_for(Grid::Agni, agent_id).is_empty());
    }

    #[test]
    fn oversized_favorite_refresh_keeps_previous_snapshot() {
        let agent_id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let saved = vec![FavoriteLocation { name: "Home".into(), region_name: "Ahern".into(), x: 1, y: 2, z: 3 }];
        let mut store = FavoriteStore::default();
        store.replace_favorites(Grid::Agni, agent_id, "Example Resident", saved.clone());
        let mut current_favorites = saved.clone();
        let oversized =
            vec![FavoriteLocation { name: "Home".into(), region_name: "Ahern".into(), x: 1, y: 2, z: 3 }; MAX_FAVORITES_PER_AGENT + 1];

        let result = apply_favorites_refresh(&mut current_favorites, &mut store, Grid::Agni, agent_id, "Example Resident", Ok(oversized));

        assert_eq!(result, Err(FavoriteStoreError::InvalidRefreshedValues("an account has too many saved favorites")));
        assert_eq!(current_favorites, saved);
        assert_eq!(store.favorites_for(Grid::Agni, agent_id), saved);
    }

    #[test]
    fn oversized_favorite_name_keeps_snapshot() {
        let test_directory = tempdir().expect("create temporary login locations directory");
        let path = login_locations_path_under(test_directory.path());
        let agent_id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let saved = vec![FavoriteLocation { name: "Home".into(), region_name: "Ahern".into(), x: 1, y: 2, z: 3 }];
        let mut store = FavoriteStore::default();
        store.replace_favorites(Grid::Agni, agent_id, "Example Resident", saved.clone());
        save_to(&path, &store).expect("save known-good favorite snapshot");
        let mut current_favorites = saved.clone();
        let oversized = vec![FavoriteLocation { name: "x".repeat(513), region_name: "Ahern".into(), x: 1, y: 2, z: 3 }];

        let result = apply_favorites_refresh(&mut current_favorites, &mut store, Grid::Agni, agent_id, "Example Resident", Ok(oversized));

        assert_eq!(result, Err(FavoriteStoreError::InvalidRefreshedValues("a saved favorite name is too long")));
        assert_eq!(current_favorites, saved);
        assert_eq!(store.favorites_for(Grid::Agni, agent_id), saved);
        save_to(&path, &store).expect("rejected refresh must leave a savable snapshot");
        assert_eq!(load_from(&path).expect("reload known-good snapshot").favorites_for(Grid::Agni, agent_id), saved);
    }

    #[test]
    fn load_rejects_oversized_store_before_parsing() {
        let test_directory = tempdir().expect("create temporary login locations directory");
        let path = login_locations_path_under(test_directory.path());
        fs::create_dir_all(path.parent().expect("login locations have a parent directory")).expect("create login locations directory");
        fs::write(&path, vec![b' '; MAX_FAVORITE_STORE_BYTES + 1]).expect("write oversized login locations");

        let error = load_from(&path).expect_err("oversized login locations must be rejected");
        assert_eq!(error, FavoriteStoreError::FileTooLarge { path });
    }

    #[test]
    fn load_rejects_corrupted_store_file() {
        let test_directory = tempdir().expect("create temporary login locations directory");
        let path = login_locations_path_under(test_directory.path());
        fs::create_dir_all(path.parent().expect("login locations have a parent directory")).expect("create login locations directory");
        fs::write(&path, b"{ invalid json").expect("write corrupted login locations");

        let error = load_from(&path).expect_err("corrupted login locations must not load as an empty store");

        assert!(matches!(error, FavoriteStoreError::InvalidFile { .. }));
        assert!(error.to_string().contains("Saved login locations at"));
        assert!(error.to_string().contains("are invalid:"));
    }

    #[test]
    fn load_rejects_excessive_favorites() {
        let test_directory = tempdir().expect("create temporary login locations directory");
        let path = login_locations_path_under(test_directory.path());
        fs::create_dir_all(path.parent().expect("login locations have a parent directory")).expect("create login locations directory");
        let oversized = FavoriteStore {
            agents: vec![AgentFavorites {
                grid: StoredGrid::Agni,
                agent_id: Uuid::from_u128(1),
                username: "Example Resident".into(),
                favorites: vec![
                    FavoriteLocation { name: "Home".into(), region_name: "Ahern".into(), x: 1, y: 2, z: 3 };
                    MAX_FAVORITES_PER_AGENT + 1
                ],
            }],
        };
        fs::write(&path, serde_json::to_vec(&oversized).expect("encode oversized favorite list")).expect("write oversized favorite list");

        let error = load_from(&path).expect_err("excessive favorite count must be rejected");
        assert_eq!(error, FavoriteStoreError::InvalidStoredValues { path, reason: "an account has too many saved favorites" });
    }

    #[test]
    fn save_rejects_favorite_names_outside_storage_limits() {
        let test_directory = tempdir().expect("create temporary login locations directory");
        let path = login_locations_path_under(test_directory.path());
        let oversized = FavoriteStore {
            agents: vec![AgentFavorites {
                grid: StoredGrid::Agni,
                agent_id: Uuid::from_u128(1),
                username: "Example Resident".into(),
                favorites: vec![FavoriteLocation {
                    name: "x".repeat(super::MAX_FAVORITE_NAME_BYTES + 1),
                    region_name: "Ahern".into(),
                    x: 1,
                    y: 2,
                    z: 3,
                }],
            }],
        };

        let error = save_to(&path, &oversized).expect_err("out-of-limit favorite must not be persisted");
        assert_eq!(error, FavoriteStoreError::InvalidValuesForSave("a saved favorite name is too long"));
        assert!(!path.exists());
    }
}
