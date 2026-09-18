use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::OnceCell;

use crate::domain::media::MediaResult;

type Outcome = Arc<OnceCell<MediaResult<PathBuf>>>;

struct Flight {
    outcome: Outcome,
    members: usize,
}

#[derive(Default)]
pub(super) struct SingleFlight {
    flights: StdMutex<HashMap<String, Flight>>,
}

impl SingleFlight {
    pub(super) fn join<'a>(&'a self, key: &'a str) -> Seat<'a> {
        let outcome = match self.flights.lock() {
            Ok(mut flights) => {
                let flight = flights.entry(key.to_owned()).or_insert_with(|| Flight {
                    outcome: Outcome::default(),
                    members: 0,
                });
                flight.members = flight.members.saturating_add(1);
                Arc::clone(&flight.outcome)
            }
            Err(_) => Outcome::default(),
        };
        Seat {
            single_flight: self,
            key,
            outcome,
        }
    }

    pub(super) fn clear(&self) {
        if let Ok(mut flights) = self.flights.lock() {
            flights.clear();
        }
    }

    fn leave(&self, key: &str, outcome: &Outcome) {
        let Ok(mut flights) = self.flights.lock() else {
            return;
        };
        let Some(flight) = flights
            .get_mut(key)
            .filter(|flight| Arc::ptr_eq(&flight.outcome, outcome))
        else {
            return;
        };
        flight.members = flight.members.saturating_sub(1);
        if flight.members == 0 || outcome.initialized() {
            flights.remove(key);
        }
    }
}

pub(super) struct Seat<'a> {
    single_flight: &'a SingleFlight,
    key: &'a str,
    outcome: Outcome,
}

impl Seat<'_> {
    pub(super) async fn outcome<F, Fut>(&self, fetch: F) -> MediaResult<PathBuf>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = MediaResult<PathBuf>>,
    {
        self.outcome.get_or_init(fetch).await.clone()
    }
}

impl Drop for Seat<'_> {
    fn drop(&mut self) {
        self.single_flight.leave(self.key, &self.outcome);
    }
}
