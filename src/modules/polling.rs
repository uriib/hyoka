use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use derive_more::From;
use futures::channel::mpsc::Receiver;

use crate::modules::{
    battery::{self, Battery},
    clock::Clock,
};

#[derive(From, Debug)]
pub enum Event {
    Clock(Clock),
    Battery(battery::Info),
}

pub enum Signal {
    Battery(Rc<Battery>),
    BatteryStop,
}

pub async fn run(signals: &mut Receiver<Signal>, mut dispatch: impl AsyncFnMut(Event)) {
    let interval = Duration::from_secs(1);
    let mut timer = compio::time::interval_at(Instant::now() + interval, interval);
    let mut battery = None;
    loop {
        timer.tick().await;
        dispatch(Clock::now().into()).await;

        if let Ok(x) = signals.try_next() {
            match x.unwrap() {
                Signal::Battery(x) => battery = Some(x),
                Signal::BatteryStop => battery = None,
            }
        }

        if let Some(battery) = battery.as_ref() {
            dispatch(battery.info().into()).await;
        }
    }
}
