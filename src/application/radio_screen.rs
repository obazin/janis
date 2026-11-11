//! The controller's side of the Radio screen: the filters, what they keep,
//! and starting a station.

use std::rc::Rc;

use slint::ComponentHandle;

use super::Application;
use crate::player::Player;
use crate::radio::{self, Station, ALL_COUNTRIES_KEY, ALL_GENRES_KEY};
use crate::text;
use crate::user_interface::{self, RadioChip, StationRow};

/// The Radio screen's filters — both multi-select, an empty one meaning
/// "all" — and the stations they keep, so a click finds its station.
#[derive(Default)]
pub(super) struct RadioScreen {
    countries: Vec<String>,
    genres: Vec<String>,
    /// Only the starred stations are shown.
    favourites_only: bool,
    shown: Vec<Station>,
}

impl Application {
    pub(super) fn connect_radio(self: &Rc<Self>) {
        let view = self.window.global::<user_interface::Radio>();
        view.set_countries(self.radio_countries.clone().into());
        view.set_genres(self.radio_genres.clone().into());
        view.set_stations(self.radio_stations.clone().into());

        view.on_country_toggled(self.handler(|application, key: slint::SharedString| {
            {
                let mut screen = application.radio_screen.borrow_mut();
                if key == ALL_COUNTRIES_KEY {
                    screen.countries.clear();
                } else {
                    radio::toggle_filter(&mut screen.countries, &key);
                }
            }
            application.refresh_radio();
        }));
        view.on_genre_toggled(self.handler(|application, key: slint::SharedString| {
            {
                let mut screen = application.radio_screen.borrow_mut();
                if key == ALL_GENRES_KEY {
                    screen.genres.clear();
                } else {
                    radio::toggle_filter(&mut screen.genres, &key);
                }
            }
            application.refresh_radio();
        }));
        view.on_favourites_toggled(self.action(|application| {
            {
                let mut screen = application.radio_screen.borrow_mut();
                screen.favourites_only = !screen.favourites_only;
            }
            application.refresh_radio();
        }));
        view.on_active_station_favourite_toggled(self.action(|application| {
            let station_id = application
                .player
                .borrow()
                .station()
                .map(|station| station.id.clone());
            if let Some(station_id) = station_id {
                let effects = application
                    .favourite_stations
                    .borrow_mut()
                    .toggle(&station_id);
                application.run(effects);
                application.refresh_radio();
            }
        }));
        view.on_station_clicked(self.handler(|application, index: i32| {
            let station = application
                .radio_screen
                .borrow()
                .shown
                .get(usize::try_from(index).unwrap_or(usize::MAX))
                .cloned();
            if let Some(station) = station {
                let effects = application.player.borrow_mut().play_station(station);
                application.run(effects);
            }
        }));
    }

    /// Which station plays or connects, so its card can show it, and whether
    /// it is a favourite.
    pub(super) fn refresh_active_station(&self, player: &Player) {
        let station = player.station();
        let radio = self.window.global::<user_interface::Radio>();
        radio.set_active_station_favourite(
            station.is_some_and(|station| self.favourite_stations.borrow().contains(&station.id)),
        );
        radio.set_active_station_id(
            station
                .map(|station| station.id.as_str())
                .unwrap_or_default()
                .into(),
        );
        radio.set_connecting(player.connecting());
    }

    /// Rebuilds the Radio screen's chips and stations from the filters, the
    /// search and the language.
    pub(super) fn refresh_radio(&self) {
        let translations = self.translations.borrow();
        let search = self.search.borrow();
        let mut screen = self.radio_screen.borrow_mut();
        let favourites = self.favourite_stations.borrow();
        self.window
            .global::<user_interface::Radio>()
            .set_favourites_only(screen.favourites_only);

        let mut countries = vec![RadioChip {
            key: ALL_COUNTRIES_KEY.into(),
            label: translations.text(ALL_COUNTRIES_KEY).into(),
            active: screen.countries.is_empty(),
        }];
        countries.extend(self.catalog.countries().iter().map(|code| RadioChip {
            key: code.as_str().into(),
            label: translations.text(&radio::country_label_key(code)).into(),
            active: screen.countries.contains(code),
        }));
        self.radio_countries.set_vec(countries);

        let mut genres = vec![RadioChip {
            key: ALL_GENRES_KEY.into(),
            label: translations.text(ALL_GENRES_KEY).into(),
            active: screen.genres.is_empty(),
        }];
        genres.extend(
            self.catalog
                .genres_by_label(&translations)
                .into_iter()
                .map(|key| RadioChip {
                    label: translations.text(&key).into(),
                    active: screen.genres.contains(&key),
                    key: key.into(),
                }),
        );
        self.radio_genres.set_vec(genres);

        screen.shown = self
            .catalog
            .filtered(
                &screen.countries,
                &screen.genres,
                screen.favourites_only.then(|| favourites.station_ids()),
                &search,
                &translations,
            )
            .into_iter()
            .cloned()
            .collect();
        self.radio_stations.set_vec(
            screen
                .shown
                .iter()
                .map(|station| StationRow {
                    id: station.id.as_str().into(),
                    name: station.name.as_str().into(),
                    detail: format!(
                        "{} · {}",
                        translations.text(&station.genre_key),
                        translations.translate("radio.kbps", &[("kbps", station.kbps.to_string())])
                    )
                    .into(),
                    favourite: favourites.contains(&station.id),
                    gradient_index: (station.gradient_index % text::ART_GRADIENT_COUNT) as i32,
                })
                .collect::<Vec<_>>(),
        );
    }
}
