//! Job schedules: a plan picked by hand (how often, at what time, which
//! days), written as cron (pg_cron) or as Oracle's calendar string
//! (DBMS_SCHEDULER's repeat_interval), read back from them when it fits.

use chrono::{Datelike, Duration, NaiveDateTime, Timelike};
use egui::RichText;

use crate::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freq {
    Minutes,
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl Freq {
    pub const ALL: [Freq; 5] = [Freq::Minutes, Freq::Hourly, Freq::Daily, Freq::Weekly, Freq::Monthly];

    pub fn label(self) -> &'static str {
        match self {
            Freq::Minutes => t("Каждые N минут", "Every N minutes"),
            Freq::Hourly => t("Каждый час", "Hourly"),
            Freq::Daily => t("Каждый день", "Daily"),
            Freq::Weekly => t("По дням недели", "Weekly"),
            Freq::Monthly => t("По числам месяца", "Monthly"),
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Freq::Minutes => t("мин.", "min"),
            Freq::Hourly => t("ч.", "h"),
            Freq::Daily => t("дн.", "days"),
            Freq::Weekly => t("нед.", "weeks"),
            Freq::Monthly => t("мес.", "months"),
        }
    }
}

/// Monday first.
const DAYS: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];

fn day_label(i: usize) -> &'static str {
    [t("Пн", "Mo"), t("Вт", "Tu"), t("Ср", "We"), t("Чт", "Th"), t("Пт", "Fr"), t("Сб", "Sa"), t("Вс", "Su")][i]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub freq: Freq,
    /// Every that many minutes / hours / days / weeks / months.
    pub every: u32,
    /// At these minutes of the hour (empty: when it started).
    pub minutes: Vec<u32>,
    /// At these hours (daily and longer; empty: when it started).
    pub hours: Vec<u32>,
    /// Weekly: Monday … Sunday.
    pub days: [bool; 7],
    /// Monthly: these days of the month (-1: the last).
    pub mdays: Vec<i32>,
}

impl Default for Plan {
    fn default() -> Self {
        Self { freq: Freq::Daily, every: 1, minutes: vec![0], hours: vec![3], days: [true, true, true, true, true, false, false], mdays: vec![1] }
    }
}

fn list(v: &[u32]) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

/// "1,5,10-12" (numbers only, in range) → sorted, without repeats.
fn nums(s: &str, lo: u32, hi: u32) -> Option<Vec<u32>> {
    let mut v = Vec::new();
    for p in s.split(',') {
        let p = p.trim();
        if let Some((a, b)) = p.split_once('-') {
            let (a, b): (u32, u32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            if a > b || a < lo || b > hi {
                return None;
            }
            v.extend(a..=b);
        } else {
            let n: u32 = p.parse().ok()?;
            if n < lo || n > hi {
                return None;
            }
            v.push(n);
        }
    }
    v.sort_unstable();
    v.dedup();
    (!v.is_empty()).then_some(v)
}

/// "*" or "*/n" → its step.
fn step(s: &str) -> Option<u32> {
    if s == "*" {
        return Some(1);
    }
    s.strip_prefix("*/")?.parse().ok().filter(|&n| n >= 1)
}

impl Plan {
    /// As cron's five fields. (Cron can't skip weeks: weekly is every week.)
    pub fn cron(&self) -> String {
        let m = if self.minutes.is_empty() { "0".to_string() } else { list(&self.minutes) };
        let h = if self.hours.is_empty() { "0".to_string() } else { list(&self.hours) };
        let every = |n: u32| if n <= 1 { "*".to_string() } else { format!("*/{n}") };
        match self.freq {
            Freq::Minutes => format!("{} * * * *", every(self.every)),
            Freq::Hourly => format!("{m} {} * * *", every(self.every)),
            Freq::Daily => format!("{m} {h} {} * *", every(self.every)),
            Freq::Weekly => {
                // cron: 0 is Sunday.
                let d: Vec<String> = (0..7).filter(|&i| self.days[i]).map(|i| ((i + 1) % 7).to_string()).collect();
                format!("{m} {h} * * {}", if d.is_empty() { "*".into() } else { d.join(",") })
            }
            Freq::Monthly => {
                let d: Vec<String> = self.mdays.iter().map(|&d| if d < 0 { "$".to_string() } else { d.to_string() }).collect();
                format!("{m} {h} {} {} *", if d.is_empty() { "1".into() } else { d.join(",") }, every(self.every))
            }
        }
    }

    /// From cron, when it's one of the shapes [`Plan::cron`] gives.
    pub fn from_cron(s: &str) -> Option<Plan> {
        let f: Vec<&str> = s.split_whitespace().collect();
        let [mi, ho, dm, mo, dw] = f.as_slice() else { return None };
        let mut p = Plan { every: 1, ..Plan::default() };
        if let (Some(n), "*", "*", "*", "*") = (step(mi), *ho, *dm, *mo, *dw) {
            p.freq = Freq::Minutes;
            p.every = n;
            p.minutes.clear();
            return Some(p);
        }
        p.minutes = nums(mi, 0, 59)?;
        if let (Some(n), "*", "*", "*") = (step(ho), *dm, *mo, *dw) {
            p.freq = Freq::Hourly;
            p.every = n;
            p.hours.clear();
            return Some(p);
        }
        p.hours = nums(ho, 0, 23)?;
        match (*dm, *mo, *dw) {
            (d, "*", "*") if step(d).is_some() => {
                p.freq = Freq::Daily;
                p.every = step(d)?;
            }
            ("*", "*", w) => {
                p.freq = Freq::Weekly;
                let mut days = [false; 7];
                for n in nums(w, 0, 7)? {
                    days[((n + 6) % 7) as usize] = true;
                }
                p.days = days;
            }
            (d, m, "*") if step(m).is_some() => {
                p.freq = Freq::Monthly;
                p.every = step(m)?;
                p.mdays = if d == "$" { vec![-1] } else { nums(d, 1, 31)?.into_iter().map(|x| x as i32).collect() };
            }
            _ => return None,
        }
        Some(p)
    }

    /// As Oracle's calendar string.
    pub fn oracle(&self) -> String {
        let freq = match self.freq {
            Freq::Minutes => "MINUTELY",
            Freq::Hourly => "HOURLY",
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
        };
        let mut s = format!("FREQ={freq}");
        if self.every > 1 {
            s += &format!(";INTERVAL={}", self.every);
        }
        match self.freq {
            Freq::Weekly => {
                let d: Vec<&str> = (0..7).filter(|&i| self.days[i]).map(|i| DAYS[i]).collect();
                if !d.is_empty() {
                    s += &format!(";BYDAY={}", d.join(","));
                }
            }
            Freq::Monthly if !self.mdays.is_empty() => {
                s += &format!(";BYMONTHDAY={}", self.mdays.iter().map(i32::to_string).collect::<Vec<_>>().join(","));
            }
            _ => {}
        }
        if !matches!(self.freq, Freq::Minutes | Freq::Hourly) && !self.hours.is_empty() {
            s += &format!(";BYHOUR={}", list(&self.hours));
        }
        if self.freq != Freq::Minutes && !self.minutes.is_empty() {
            s += &format!(";BYMINUTE={};BYSECOND=0", list(&self.minutes));
        }
        s
    }

    /// From Oracle's calendar string, when it holds only what a plan has.
    pub fn from_oracle(s: &str) -> Option<Plan> {
        let mut p = Plan { every: 1, minutes: Vec::new(), hours: Vec::new(), mdays: Vec::new(), ..Plan::default() };
        let mut freq = None;
        for part in s.split(';').map(str::trim).filter(|x| !x.is_empty()) {
            let (k, v) = part.split_once('=')?;
            let v = v.trim();
            match k.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match v.to_ascii_uppercase().as_str() {
                        "MINUTELY" => Freq::Minutes,
                        "HOURLY" => Freq::Hourly,
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        _ => return None,
                    })
                }
                "INTERVAL" => p.every = v.parse().ok().filter(|&n| (1..=999).contains(&n))?,
                "BYMINUTE" => p.minutes = nums(v, 0, 59)?,
                "BYHOUR" => p.hours = nums(v, 0, 23)?,
                "BYSECOND" if v == "0" => {}
                "BYDAY" => {
                    let mut days = [false; 7];
                    for d in v.split(',') {
                        days[DAYS.iter().position(|x| x.eq_ignore_ascii_case(d.trim()))?] = true;
                    }
                    p.days = days;
                }
                "BYMONTHDAY" => {
                    p.mdays = v.split(',').map(|d| d.trim().parse::<i32>().ok().filter(|&n| n == -1 || (1..=31).contains(&n))).collect::<Option<Vec<_>>>()?;
                }
                _ => return None,
            }
        }
        p.freq = freq?;
        if p.freq == Freq::Weekly && !s.to_ascii_uppercase().contains("BYDAY") {
            p.days = [true; 7];
        }
        Some(p)
    }

    /// What it means, in words.
    pub fn describe(&self) -> String {
        let at = |p: &Plan| {
            let m = if p.minutes.is_empty() { vec![0] } else { p.minutes.clone() };
            if p.hours.is_empty() {
                return format!("{} :{}", t("в минуты", "at minutes"), list(&m));
            }
            let times: Vec<String> = p.hours.iter().flat_map(|h| m.iter().map(move |m| format!("{h:02}:{m:02}"))).collect();
            format!("{} {}", t("в", "at"), times.join(", "))
        };
        let n = self.every;
        match self.freq {
            Freq::Minutes => format!("{} {n} {}", t("каждые", "every"), t("мин.", "min")),
            Freq::Hourly => {
                let m = if self.minutes.is_empty() { "0".into() } else { list(&self.minutes) };
                if n > 1 { format!("{} {n} {}, {} :{m}", t("каждые", "every"), t("ч.", "h"), t("в минуты", "at minutes")) } else { format!("{}, {} :{m}", t("каждый час", "every hour"), t("в минуты", "at minutes")) }
            }
            Freq::Daily => {
                if n > 1 { format!("{} {n} {}, {}", t("каждые", "every"), t("дн.", "days"), at(self)) } else { format!("{}, {}", t("каждый день", "every day"), at(self)) }
            }
            Freq::Weekly => {
                let d: Vec<&str> = (0..7).filter(|&i| self.days[i]).map(day_label).collect();
                let w = if n > 1 { format!("{} {n} {}: ", t("раз в", "every"), t("нед.", "weeks")) } else { String::new() };
                format!("{w}{}, {}", d.join(", "), at(self))
            }
            Freq::Monthly => {
                let d: Vec<String> = self.mdays.iter().map(|&d| if d < 0 { t("последнее", "last").to_string() } else { d.to_string() }).collect();
                let w = if n > 1 { format!("{} {n} {}: ", t("раз в", "every"), t("мес.", "months")) } else { String::new() };
                format!("{w}{} {}, {}", t("числа", "days"), d.join(", "), at(self))
            }
        }
    }

    /// The next runs after `from` (cron's way of counting; Oracle counts
    /// INTERVAL from the job's start: there it's close, not exact).
    pub fn next(&self, from: NaiveDateTime, n: usize) -> Vec<NaiveDateTime> {
        let mut out = Vec::new();
        let mins: Vec<u32> = match self.freq {
            Freq::Minutes => (0..60).step_by(self.every.max(1) as usize).collect(),
            _ if self.minutes.is_empty() => vec![0],
            _ => self.minutes.clone(),
        };
        let hours: Vec<u32> = match self.freq {
            Freq::Minutes => (0..24).collect(),
            Freq::Hourly => (0..24).step_by(self.every.max(1) as usize).collect(),
            _ if self.hours.is_empty() => vec![0],
            _ => self.hours.clone(),
        };
        let start = from.date();
        for k in 0..800 {
            let day = start + Duration::days(k);
            let ok = match self.freq {
                Freq::Minutes | Freq::Hourly => true,
                Freq::Daily => self.every <= 1 || (day.day() - 1) % self.every == 0,
                Freq::Weekly => self.days[day.weekday().num_days_from_monday() as usize],
                Freq::Monthly => {
                    let last = (day + Duration::days(1)).month() != day.month();
                    (self.every <= 1 || (day.month() - 1) % self.every == 0) && self.mdays.iter().any(|&d| d == day.day() as i32 || (d < 0 && last))
                }
            };
            if !ok {
                continue;
            }
            for &h in &hours {
                for &m in &mins {
                    let Some(at) = day.and_hms_opt(h, m, 0) else { continue };
                    if at > from {
                        out.push(at);
                        if out.len() >= n {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }
}

/// "3, 15" / "3-5" typed into a field.
fn edit_nums(ui: &mut egui::Ui, id: egui::Id, v: &mut Vec<u32>, lo: u32, hi: u32, width: f32) -> bool {
    let shown = list(v);
    let mut s = ui.ctx().data(|d| d.get_temp::<(String, String)>(id)).filter(|(was, _)| *was == shown).map_or(shown.clone(), |(_, s)| s);
    let r = ui.add(egui::TextEdit::singleline(&mut s).desired_width(width));
    let good = nums(&s, lo, hi);
    if s.trim().is_empty() || good.is_some() {
        ui.ctx().data_mut(|d| d.remove::<(String, String)>(id));
    } else {
        ui.ctx().data_mut(|d| d.insert_temp(id, (shown, s.clone())));
        ui.painter().rect_stroke(r.rect, 4.0, egui::Stroke::new(1.0, crate::ui::RED), egui::StrokeKind::Inside);
    }
    match good {
        Some(n) if r.changed() && n != *v => {
            *v = n;
            true
        }
        _ => false,
    }
}

/// The plan's controls; true when it changed.
pub fn plan_ui(ui: &mut egui::Ui, p: &mut Plan, cron: bool) -> bool {
    let before = p.clone();
    let id = ui.id().with("plan");
    egui::Grid::new(id.with("grid")).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(t("Как часто", "How often"));
        ui.horizontal(|ui| {
            for f in Freq::ALL {
                if ui.selectable_label(p.freq == f, f.label()).clicked() && p.freq != f {
                    p.freq = f;
                    p.every = if f == Freq::Minutes { 15 } else { 1 };
                    if f != Freq::Minutes && p.minutes.is_empty() {
                        p.minutes = vec![0];
                    }
                    if !matches!(f, Freq::Minutes | Freq::Hourly) && p.hours.is_empty() {
                        p.hours = vec![3];
                    }
                    if f == Freq::Monthly && p.mdays.is_empty() {
                        p.mdays = vec![1];
                    }
                }
            }
        });
        ui.end_row();
        // (Cron can't skip weeks.)
        if !(cron && p.freq == Freq::Weekly) {
            ui.label(t("Интервал", "Interval"));
            ui.horizontal(|ui| {
                ui.label(t("каждые", "every"));
                ui.add(egui::DragValue::new(&mut p.every).range(1..=if p.freq == Freq::Minutes { 59 } else { 365 }));
                ui.label(p.freq.unit());
            });
            ui.end_row();
        }
        if p.freq == Freq::Weekly {
            ui.label(t("Дни недели", "Days"));
            ui.horizontal(|ui| {
                for i in 0..7 {
                    let on = p.days[i];
                    if ui.selectable_label(on, day_label(i)).clicked() {
                        p.days[i] = !on;
                        if !p.days.iter().any(|&d| d) {
                            p.days[i] = true;
                        }
                    }
                }
                ui.add_space(8.0);
                if ui.small_button(t("будни", "weekdays")).clicked() {
                    p.days = [true, true, true, true, true, false, false];
                }
                if ui.small_button(t("все", "all")).clicked() {
                    p.days = [true; 7];
                }
            });
            ui.end_row();
        }
        if p.freq == Freq::Monthly {
            ui.label(t("Числа", "Days"));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                for row in 0..5 {
                    ui.horizontal(|ui| {
                        for col in 0..7 {
                            let d = row * 7 + col + 1;
                            if d > 31 {
                                break;
                            }
                            let on = p.mdays.contains(&d);
                            if ui.add_sized([28.0, 22.0], egui::Button::selectable(on, d.to_string())).clicked() {
                                if on {
                                    p.mdays.retain(|&x| x != d);
                                } else {
                                    p.mdays.push(d);
                                    p.mdays.sort_unstable();
                                }
                            }
                        }
                        if row == 4 {
                            let on = p.mdays.contains(&-1);
                            if ui.add(egui::Button::selectable(on, t("последнее", "last"))).clicked() {
                                if on {
                                    p.mdays.retain(|&x| x != -1);
                                } else {
                                    p.mdays.push(-1);
                                }
                            }
                        }
                    });
                }
                if p.mdays.is_empty() {
                    p.mdays.push(1);
                }
            });
            ui.end_row();
        }
        if p.freq != Freq::Minutes {
            ui.label(t("Время", "Time"));
            ui.horizontal(|ui| {
                if p.freq != Freq::Hourly {
                    ui.label(t("часы", "hours"));
                    if p.hours.len() == 1 {
                        let mut h = p.hours[0];
                        if ui.add(egui::DragValue::new(&mut h).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32))).changed() {
                            p.hours = vec![h];
                        }
                    } else {
                        edit_nums(ui, id.with("hours"), &mut p.hours, 0, 23, 90.0);
                    }
                }
                ui.label(t("минуты", "minutes"));
                if p.minutes.len() <= 1 {
                    let mut m = p.minutes.first().copied().unwrap_or(0);
                    if ui.add(egui::DragValue::new(&mut m).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32))).changed() || p.minutes.is_empty() {
                        p.minutes = vec![m];
                    }
                } else {
                    edit_nums(ui, id.with("mins"), &mut p.minutes, 0, 59, 90.0);
                }
                ui.menu_button("…", |ui| {
                    ui.label(RichText::new(t("Несколько значений через запятую или диапазоном: 9,13,18 или 9-18", "Several, by commas or a range: 9,13,18 or 9-18")).weak());
                    if p.freq != Freq::Hourly {
                        ui.horizontal(|ui| {
                            ui.label(t("часы", "hours"));
                            edit_nums(ui, id.with("hours-many"), &mut p.hours, 0, 23, 140.0);
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label(t("минуты", "minutes"));
                        edit_nums(ui, id.with("mins-many"), &mut p.minutes, 0, 59, 140.0);
                    });
                });
            });
            ui.end_row();
        }
    });
    *p != before
}

/// The next runs, a line each.
pub fn next_ui(ui: &mut egui::Ui, p: &Plan) {
    let now = chrono::Local::now().naive_local().with_second(0).unwrap_or_default();
    let runs = p.next(now, 5);
    ui.label(RichText::new(t("Ближайшие запуски (по местному времени):", "Next runs (local time):")).weak());
    for r in runs {
        let day = day_label(r.weekday().num_days_from_monday() as usize);
        ui.label(RichText::new(format!("{day}  {}", r.format("%d.%m.%Y  %H:%M"))).monospace());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn cron_both_ways() {
        for s in ["*/15 * * * *", "5 * * * *", "0 3 * * *", "30 9,18 * * 1,2,3,4,5", "0 0 1,15 * *", "0 4 $ */3 *", "10 */2 * * *", "0 6 */2 * *"] {
            let p = Plan::from_cron(s).unwrap_or_else(|| panic!("{s}"));
            assert_eq!(p.cron(), s, "{p:?}");
        }
        // Sunday as 7 too.
        assert!(Plan::from_cron("0 3 * * 7").unwrap().days[6]);
        assert!(Plan::from_cron("0 3 * * MON").is_none());
        assert!(Plan::from_cron("@daily").is_none());
    }

    #[test]
    fn oracle_both_ways() {
        for s in ["FREQ=HOURLY", "FREQ=MINUTELY;INTERVAL=10", "FREQ=DAILY;BYHOUR=3;BYMINUTE=0;BYSECOND=0", "FREQ=WEEKLY;BYDAY=MON,FRI;BYHOUR=9;BYMINUTE=30;BYSECOND=0", "FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1,-1;BYHOUR=4;BYMINUTE=0;BYSECOND=0"] {
            let p = Plan::from_oracle(s).unwrap_or_else(|| panic!("{s}"));
            assert_eq!(p.oracle(), s, "{p:?}");
        }
        assert!(Plan::from_oracle("FREQ=DAILY;BYDAY=MON;BYSETPOS=-1").is_none());
        assert!(Plan::from_oracle("SYSTIMESTAMP + 1").is_none());
        // One plan, both ways of writing it.
        let p = Plan::from_oracle("FREQ=DAILY;BYHOUR=3;BYMINUTE=15").unwrap();
        assert_eq!(p.cron(), "15 3 * * *");
    }

    #[test]
    fn next_runs() {
        let p = Plan::from_cron("30 9 * * 1,5").unwrap();
        // 2026-11-12 is a Thursday.
        let n = p.next(at("2026-11-12 10:00"), 3);
        assert_eq!(n, vec![at("2026-11-13 09:30"), at("2026-11-16 09:30"), at("2026-11-20 09:30")]);
        let p = Plan::from_cron("*/20 * * * *").unwrap();
        assert_eq!(p.next(at("2026-11-12 10:05"), 2), vec![at("2026-11-12 10:20"), at("2026-11-12 10:40")]);
        let p = Plan::from_oracle("FREQ=MONTHLY;BYMONTHDAY=-1;BYHOUR=23;BYMINUTE=0").unwrap();
        assert_eq!(p.next(at("2026-11-12 10:00"), 2), vec![at("2026-11-30 23:00"), at("2026-12-31 23:00")]);
    }
}
