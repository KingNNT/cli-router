use chrono::NaiveDate;

use crate::application::ports::Clock;

pub struct SystemClock;

impl Clock for SystemClock {
    fn today(&self) -> NaiveDate {
        chrono::Local::now().date_naive()
    }
}
