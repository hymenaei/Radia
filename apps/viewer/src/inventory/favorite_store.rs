use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use network::Grid;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_vec_pretty};
use utils::write_atomic;
use uuid::Uuid;

use super::FavoriteLocation;

const APP_DIRECTORY: &str = "Radia";
const LOGIN_LOCATIONS_FILE: &str = "login_locations.json";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FavoriteStore {
    #[serde(default)]
    agents: Vec<AgentFavorites>,
}

impl FavoriteStore {
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
    refresh: Result<Vec<FavoriteLocation>, String>,
) -> Result<(), String> {
    let favorites = refresh?;
    store.replace_favorites(grid, agent_id, username, favorites.clone());
    *current_favorites = favorites;
    Ok(())
}

pub fn load() -> Result<FavoriteStore, String> {
    load_from(&login_locations_path()?)
}

pub fn save(favorites: &FavoriteStore) -> Result<(), String> {
    save_to(&login_locations_path()?, favorites)
}

fn login_locations_path() -> Result<PathBuf, String> {
    let roaming_dir = dirs::data_dir().ok_or_else(|| "Could not find the roaming application-data directory.".to_string())?;
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

fn load_from(path: &Path) -> Result<FavoriteStore, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(FavoriteStore::default()),
        Err(error) => return Err(format!("Could not read login locations from {}: {error}", path.display())),
    };

    from_str(&contents).map_err(|error| format!("Saved login locations at {} are invalid: {error}", path.display()))
}

fn save_to(path: &Path, favorites: &FavoriteStore) -> Result<(), String> {
    let directory = path.parent().ok_or_else(|| "Login locations need a parent directory.".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("Could not create {}: {error}", directory.display()))?;

    let contents = to_vec_pretty(favorites).map_err(|error| format!("Could not encode favorite landmarks: {error}"))?;
    write_atomic(path, &contents).map_err(|error| format!("Could not save login locations to {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use network::Grid;
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{FavoriteStore, apply_favorites_refresh, load_from, login_locations_path_under, save_to};
    use crate::inventory::FavoriteLocation;

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
            Err("region lookup failed".into()),
        );

        assert_eq!(result, Err("region lookup failed".into()));
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
}
