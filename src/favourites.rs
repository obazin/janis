//! The stations the listener starred.
//!
//! A station is a favourite or it is not, by its catalog id; none is until the
//! listener says so. A toggle is saved at once, so the choice survives a
//! restart.

use std::collections::HashSet;

use crate::effects::Effect;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FavouriteStations {
    station_ids: HashSet<String>,
}

impl FavouriteStations {
    /// Boot hydration from the saved ids.
    pub fn restore(&mut self, station_ids: Vec<String>) {
        self.station_ids = station_ids.into_iter().collect();
    }

    pub fn contains(&self, station_id: &str) -> bool {
        self.station_ids.contains(station_id)
    }

    /// The ids, for the Radio screen's favourites filter.
    pub fn station_ids(&self) -> &HashSet<String> {
        &self.station_ids
    }

    /// Stars a station, or takes its star away.
    pub fn toggle(&mut self, station_id: &str) -> Vec<Effect> {
        let favourite = !self.station_ids.remove(station_id);
        if favourite {
            self.station_ids.insert(station_id.to_owned());
        }
        vec![Effect::PersistFavouriteStation {
            station_id: station_id.to_owned(),
            favourite,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_station_is_a_favourite_until_the_listener_stars_it() {
        let favourites = FavouriteStations::default();
        assert!(!favourites.contains("jazz"));
        assert!(favourites.station_ids().is_empty());
    }

    #[test]
    fn toggling_stars_a_station_then_takes_the_star_away_and_saves_each_change() {
        let mut favourites = FavouriteStations::default();

        let effects = favourites.toggle("jazz");
        assert!(favourites.contains("jazz"));
        assert_eq!(
            effects,
            [Effect::PersistFavouriteStation {
                station_id: "jazz".to_owned(),
                favourite: true,
            }]
        );

        let effects = favourites.toggle("jazz");
        assert!(!favourites.contains("jazz"));
        assert_eq!(
            effects,
            [Effect::PersistFavouriteStation {
                station_id: "jazz".to_owned(),
                favourite: false,
            }]
        );
    }

    #[test]
    fn starring_one_station_leaves_the_others_alone() {
        let mut favourites = FavouriteStations::default();
        favourites.restore(vec!["jazz".to_owned(), "rock".to_owned()]);

        favourites.toggle("rock");

        assert!(favourites.contains("jazz"));
        assert!(!favourites.contains("rock"));
    }
}
