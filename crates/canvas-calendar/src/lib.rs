//! Host-embedded agenda. The host supplies authenticated owner observations and
//! its Coherence egui style; rendering never performs network or credential I/O.
use chrono::{DateTime, FixedOffset, NaiveDate};
use egui::{Color32, RichText, ScrollArea};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub connection: String,
    pub calendar: String,
    pub event: String,
}
#[derive(Clone, Debug)]
pub enum Period {
    Timed {
        start: DateTime<FixedOffset>,
        end: DateTime<FixedOffset>,
        zone: Option<String>,
    },
    /// Google date-only ends are exclusive; never convert these to UTC instants.
    AllDay {
        start: NaiveDate,
        end_exclusive: NaiveDate,
    },
}
#[derive(Clone, Debug)]
pub struct Event {
    pub id: Identity,
    pub account: String,
    pub title: String,
    pub period: Period,
    pub status: String,
    pub location: String,
}
#[derive(Default)]
pub struct Snapshot {
    pub events: Vec<Event>,
    pub notices: Vec<String>,
    pub fresh: bool,
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or_else(|| format!("Missing or invalid {key}"))
}
fn period(v: &Value) -> Result<Period, String> {
    let start = &v["start"];
    let end = &v["end"];
    if let Some(date) = start["date"].as_str() {
        if start.get("dateTime").is_some() || end.get("dateTime").is_some() {
            return Err("Mixed event date types".into());
        }
        let start =
            NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| "Invalid all-day start")?;
        let end_exclusive = NaiveDate::parse_from_str(field(end, "date")?, "%Y-%m-%d")
            .map_err(|_| "Invalid all-day end")?;
        if end_exclusive <= start {
            return Err("Invalid all-day interval".into());
        }
        Ok(Period::AllDay {
            start,
            end_exclusive,
        })
    } else {
        if end.get("date").is_some() {
            return Err("Mixed event date types".into());
        }
        let start_time = DateTime::parse_from_rfc3339(field(start, "dateTime")?)
            .map_err(|_| "Invalid event start")?;
        let end_time = DateTime::parse_from_rfc3339(field(end, "dateTime")?)
            .map_err(|_| "Invalid event end")?;
        if end_time <= start_time {
            return Err("Invalid event interval".into());
        }
        Ok(Period::Timed {
            start: start_time,
            end: end_time,
            zone: start["timeZone"].as_str().map(str::to_owned),
        })
    }
}

impl Snapshot {
    /// Replace one exact calendar observation. Hosts compose snapshots explicitly;
    /// connection/calendar/event identity stays intact across a unified agenda.
    pub fn from_bridge(reply: &Value, fresh: bool) -> Result<Self, String> {
        if reply["schema_version"] != 1 || reply["operation"] != "events" {
            return Err("Unsupported Calendar observation".into());
        }
        let connection = field(&reply["account"], "connection_id")?;
        let account = field(&reply["account"], "email")?;
        let calendar = field(reply, "calendar_id")?;
        let rows = reply["data"]["items"]
            .as_array()
            .ok_or("Missing calendar event list")?;
        if rows.len() > 4096 {
            return Err("Calendar observation exceeds event bound".into());
        }
        let mut snapshot = Self {
            fresh,
            ..Self::default()
        };
        let mut ids = HashSet::new();
        for row in rows {
            let id = field(row, "id")?;
            if !ids.insert(id) {
                return Err("Duplicate calendar event identity".into());
            }
            if row["status"] == "cancelled" {
                continue;
            }
            snapshot.events.push(Event {
                id: Identity {
                    connection: connection.into(),
                    calendar: calendar.into(),
                    event: id.into(),
                },
                account: account.into(),
                title: row["summary"].as_str().unwrap_or("Untitled event").into(),
                period: period(row)?,
                status: row["status"].as_str().unwrap_or("unknown").into(),
                location: row["location"].as_str().unwrap_or("").into(),
            });
        }
        if reply["data"]["nextPageToken"].is_string() {
            snapshot
                .notices
                .push("More events available; this calendar is incomplete".into());
        }
        Ok(snapshot)
    }
}

fn time_label(period: &Period) -> String {
    match period {
        Period::AllDay {
            start,
            end_exclusive,
        } => format!("{start} · All day · ends {end_exclusive} (exclusive)"),
        Period::Timed { start, end, zone } => format!(
            "{} – {} {}",
            start.format("%b %d %H:%M %:z"),
            end.format("%b %d %H:%M %:z"),
            zone.as_deref().unwrap_or("")
        ),
    }
}
#[derive(Default)]
pub struct Agenda {
    pub selected: Option<Identity>,
    query: String,
}
pub struct Report {
    pub matched: usize,
    pub drawn: usize,
}
impl Agenda {
    /// Account colors are supplied by the host's shared appearance. Color is
    /// supplementary: the full account address is always shown beside the event.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        account_colors: &[Color32],
    ) -> Report {
        ui.heading("Calendar");
        if !snapshot.fresh {
            ui.colored_label(ui.visuals().warn_fg_color, "Calendar observation is stale");
        }
        for notice in &snapshot.notices {
            ui.label(notice);
        }
        ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Filter events or accounts"));
        let query = self.query.to_lowercase();
        let events: Vec<_> = snapshot
            .events
            .iter()
            .filter(|e| {
                query.is_empty()
                    || e.title.to_lowercase().contains(&query)
                    || e.account.to_lowercase().contains(&query)
            })
            .collect();
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !snapshot.events.iter().any(|e| &e.id == id))
        {
            self.selected = None;
        }
        let mut drawn = 0;
        let height = ui.text_style_height(&egui::TextStyle::Body) + 10.0;
        ScrollArea::vertical()
            .max_height((ui.available_height() - 130.0).max(height * 3.0))
            .show_rows(ui, height, events.len(), |ui, range| {
                for index in range {
                    let event = events[index];
                    drawn += 1;
                    let hash = event
                        .account
                        .bytes()
                        .fold(0usize, |n, b| n.wrapping_mul(31).wrapping_add(b as usize));
                    let color = account_colors
                        .get(hash % account_colors.len().max(1))
                        .copied()
                        .unwrap_or(ui.visuals().text_color());
                    ui.push_id(
                        (&event.id.connection, &event.id.calendar, &event.id.event),
                        |ui| {
                            let wide = ui.available_width() >= 720.0;
                            let time_width = if wide { 170.0 } else { 100.0 };
                            let account_width = if wide { 200.0 } else { 140.0 };
                            let status_width = if wide { 90.0 } else { 0.0 };
                            let gaps = ui.spacing().item_spacing.x * if wide { 3.0 } else { 2.0 };
                            let title_width = (ui.available_width()
                                - time_width
                                - account_width
                                - status_width
                                - gaps)
                                .max(40.0);
                            let when = match &event.period {
                                Period::AllDay { start, .. } => {
                                    format!("{} · All day", start.format("%b %d"))
                                }
                                Period::Timed { start, .. } => {
                                    start.format("%b %d · %H:%M %:z").to_string()
                                }
                            };
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    [time_width, height],
                                    egui::Label::new(when).truncate(),
                                )
                                .on_hover_text(time_label(&event.period));
                                if ui
                                    .add_sized(
                                        [title_width, height],
                                        egui::Button::selectable(
                                            self.selected.as_ref() == Some(&event.id),
                                            &event.title,
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&event.title)
                                    .clicked()
                                {
                                    self.selected = Some(event.id.clone());
                                }
                                ui.add_sized(
                                    [account_width, height],
                                    egui::Label::new(RichText::new(&event.account).color(color))
                                        .truncate(),
                                )
                                .on_hover_text(&event.account);
                                if wide {
                                    ui.add_sized(
                                        [status_width, height],
                                        egui::Label::new(&event.status).truncate(),
                                    );
                                }
                            });
                        },
                    );
                }
            });
        if events.is_empty() {
            ui.label("No events in this observation match the filter");
        }
        if let Some(event) = snapshot
            .events
            .iter()
            .find(|e| Some(&e.id) == self.selected.as_ref())
        {
            ui.separator();
            ui.label(RichText::new(&event.title).strong());
            ui.label(time_label(&event.period));
            ui.label(format!("{} · {}", event.account, event.id.calendar));
            ui.label(&event.location);
        }
        Report {
            matched: events.len(),
            drawn,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn reply(items: Value) -> Value {
        json!({"schema_version":1,"operation":"events","account":{
        "connection_id":"google-one","email":"avery@fpl.dev"},"calendar_id":"primary","data":{"items":items}})
    }
    fn timed() -> Value {
        json!({"id":"same","summary":"Design review","status":"confirmed",
        "start":{"dateTime":"2026-11-01T01:30:00-07:00","timeZone":"America/Los_Angeles"},
        "end":{"dateTime":"2026-11-01T01:30:00-08:00","timeZone":"America/Los_Angeles"}})
    }
    #[test]
    fn dst_and_date_only_events_preserve_owner_semantics() {
        let s = Snapshot::from_bridge(
            &reply(json!([timed(),{"id":"day","start":{"date":"2026-11-01"},
            "end":{"date":"2026-11-03"}}])),
            true,
        )
        .unwrap();
        match &s.events[0].period {
            Period::Timed { start, end, .. } => assert_eq!((*end - *start).num_minutes(), 60),
            _ => panic!(),
        }
        match &s.events[1].period {
            Period::AllDay {
                start,
                end_exclusive,
            } => assert_eq!((*end_exclusive - *start).num_days(), 2),
            _ => panic!(),
        }
        let mut other = reply(json!([timed()]));
        other["account"]["connection_id"] = json!("google-two");
        assert_ne!(
            s.events[0].id,
            Snapshot::from_bridge(&other, true).unwrap().events[0].id
        );
    }
    #[test]
    fn cancellations_pagination_and_bad_observations_are_explicit() {
        let mut r = reply(json!([{"id":"removed","status":"cancelled"}]));
        r["data"]["nextPageToken"] = json!("next");
        let s = Snapshot::from_bridge(&r, false).unwrap();
        assert!(s.events.is_empty());
        assert!(!s.fresh);
        assert_eq!(s.notices.len(), 1);
        assert!(Snapshot::from_bridge(&reply(json!([timed(), timed()])), true).is_err());
        assert!(Snapshot::from_bridge(&reply(json!([{"id":"broken"}])), true).is_err());
    }
    #[test]
    fn large_agenda_virtualizes_and_keeps_selected_context() {
        let mut items = Vec::new();
        for n in 0..10_000 {
            let mut item = timed();
            item["id"] = json!(n.to_string());
            items.push(item);
        }
        let mut s = Snapshot::default();
        for chunk in items.chunks(100) {
            s.events.extend(
                Snapshot::from_bridge(&reply(json!(chunk)), true)
                    .unwrap()
                    .events,
            );
        }
        s.fresh = true;
        let context = egui::Context::default();
        let mut agenda = Agenda {
            selected: Some(s.events[42].id.clone()),
            ..Default::default()
        };
        let mut report = Report {
            matched: 0,
            drawn: 0,
        };
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    report = agenda.show(ui, &s, &[Color32::LIGHT_BLUE]);
                });
            },
        );
        assert_eq!(report.matched, 10_000);
        assert!(report.drawn > 0 && report.drawn < 100);
        s.events.reverse();
        assert_eq!(agenda.selected, Some(s.events[9957].id.clone()));
    }
}
