//! The controller's side of the Radio screen: the filters, what they keep,
//! and starting a station.

use std::rc::Rc;

use slint::ComponentHandle;

use super::{list_place, Application};
use crate::player::Player;
use crate::radio::{self, Chip, Filters, Station};
use crate::text;
use crate::user_interface::{self, RadioChip, StationRow};

/// The Radio screen's filters — both multi-select, an empty one meaning
/// "all" — and the stations they keep, so a click finds its station.
#[derive(Default)]
pub(super) struct RadioScreen {
    filters: Filters,
    shown: Vec<Station>,
}

impl Application {
    pub(super) fn connect_radio(self: &Rc<Self>) {
        let view = self.window.global::<user_interface::Radio>();
        view.set_countries(self.radio_countries.clone().into());
        view.set_genres(self.radio_genres.clone().into());
        view.set_stations(self.radio_stations.clone().into());

        view.on_country_toggled(self.handler(|application, key: slint::SharedString| {
            application
                .radio_screen
                .borrow_mut()
                .filters
                .toggle_country(&key);
            application.refresh_radio();
        }));
        view.on_genre_toggled(self.handler(|application, key: slint::SharedString| {
            application
                .radio_screen
                .borrow_mut()
                .filters
                .toggle_genre(&key);
            application.refresh_radio();
        }));
        view.on_favourites_toggled(self.action(|application| {
            application
                .radio_screen
                .borrow_mut()
                .filters
                .toggle_favourites();
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
            let station = list_place(index)
                .and_then(|place| application.radio_screen.borrow().shown.get(place).cloned());
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
            .set_favourites_only(screen.filters.favourites_only);
        let chip = |chip: Chip| RadioChip {
            key: chip.key.into(),
            label: chip.label.into(),
            active: chip.active,
        };
        self.radio_countries.set_vec(
            self.catalog
                .country_chips(&screen.filters, &translations)
                .into_iter()
                .map(chip)
                .collect::<Vec<_>>(),
        );
        self.radio_genres.set_vec(
            self.catalog
                .genre_chips(&screen.filters, &translations)
                .into_iter()
                .map(chip)
                .collect::<Vec<_>>(),
        );

        screen.shown = self
            .catalog
            .filtered(
                &screen.filters.countries,
                &screen.filters.genres,
                screen
                    .filters
                    .favourites_only
                    .then(|| favourites.station_ids()),
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
                    detail: radio::station_detail(station, &translations).into(),
                    favourite: favourites.contains(&station.id),
                    gradient_index: (station.gradient_index % text::ART_GRADIENT_COUNT) as i32,
                })
                .collect::<Vec<_>>(),
        );
    }
}
