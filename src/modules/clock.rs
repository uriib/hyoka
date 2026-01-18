use arrayvec::ArrayVec;
use chrono::{DateTime, Datelike, Local, Timelike};

#[derive(Debug)]
pub struct Clock {
    date_time: DateTime<Local>,
}

impl Default for Clock {
    fn default() -> Self {
        Self::now()
    }
}

impl Clock {
    pub fn now() -> Self {
        Self {
            date_time: Local::now(),
        }
    }
    pub fn year(&self) -> [u8; 4] {
        let year = self.date_time.year();
        [year / 1000, (year / 100) % 10, (year / 10) % 10, year % 10].map(|x| x as u8 + b'0')
    }
    pub fn month(&self) -> &'static str {
        [
            "JAN", "FÉV", "MAR", "AVR", "MAI", "JUN", "JUL", "AOU", "SÉP", "OCT", "NOV", "DÉC",
        ][self.date_time.month0() as usize]
    }
    pub fn day(&self) -> [u8; 2] {
        let day = self.date_time.day();
        [day / 10, day % 10].map(|x| x as u8 + b'0')
    }
    pub fn date(&self) -> ArrayVec<u8, 12> {
        self.year()
            .into_iter()
            .chain([b' '])
            .chain(self.month().bytes())
            .chain([b' '])
            .chain(self.day().into_iter())
            .collect()
    }
    pub fn time(&self) -> [u8; 8] {
        let [h, m, s] = [
            self.date_time.hour(),
            self.date_time.minute(),
            self.date_time.second(),
        ]
        .map(|x| x as u8);
        [
            h / 10 + b'0',
            h % 10 + b'0',
            b':',
            m / 10 + b'0',
            m % 10 + b'0',
            b':',
            s / 10 + b'0',
            s % 10 + b'0',
        ]
    }
    pub fn weekday(&self) -> &'static str {
        ["日", "月", "火", "水", "木", "金", "土"]
            [self.date_time.weekday().number_from_monday() as usize % 7]
    }
}
