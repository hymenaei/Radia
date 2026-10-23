use network::Grid;
use uuid::Uuid;
use viewer::inventory::{FavoriteLocation, favorite_store::FavoriteStore};

#[test]
fn store_scopes_agents_and_reassigns_usernames() {
    let agni_agent = Uuid::from_u128(1);
    let aditi_agent = Uuid::from_u128(2);
    let agni_favorites = vec![FavoriteLocation { name: "Sandbox".into(), region_name: "Agni region".into(), x: 128, y: 64, z: 23 }];
    let aditi_favorites = vec![FavoriteLocation { name: "Home".into(), region_name: "Aditi region".into(), x: 1, y: 2, z: 3 }];
    let mut store = FavoriteStore::default();

    store.replace_favorites(Grid::Agni, agni_agent, "  Example  ", agni_favorites.clone());
    store.replace_favorites(Grid::Aditi, aditi_agent, "Example Resident", aditi_favorites.clone());

    assert_eq!(store.favorites_for(Grid::Agni, agni_agent), agni_favorites);
    assert_eq!(store.favorites_for(Grid::Aditi, aditi_agent), aditi_favorites);
    assert!(store.favorites_for(Grid::Agni, aditi_agent).is_empty());
    assert_eq!(store.agent_id_for_username(Grid::Agni, "EXAMPLE resident"), Some(agni_agent));
    assert_eq!(store.agent_id_for_username(Grid::Aditi, "example"), Some(aditi_agent));

    assert!(store.remember_agent(Grid::Agni, agni_agent, "Renamed Account"));
    assert_eq!(store.favorites_for(Grid::Agni, agni_agent), agni_favorites);
    assert_eq!(store.agent_id_for_username(Grid::Agni, "Example"), None);

    let replacement_agent = Uuid::from_u128(3);
    assert!(store.remember_agent(Grid::Agni, replacement_agent, "Renamed Account"));
    assert_eq!(store.agent_id_for_username(Grid::Agni, "Renamed Account"), Some(replacement_agent));
    assert_eq!(store.favorites_for(Grid::Agni, agni_agent), agni_favorites);
    assert!(store.favorites_for(Grid::Agni, replacement_agent).is_empty());
}
