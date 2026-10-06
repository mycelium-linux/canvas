//! Renderer-neutral chart documents and bounded data ingestion.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, error, fmt};

pub const MAX_CHART_SERIES: usize = 8;
pub const MAX_CHART_POINTS: usize = 10_000;
const MAX_TEXT: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ChartKind {
    Line,
    Bar,
    Scatter,
}

impl ChartKind {
    pub fn parse(value: &str) -> Result<Self, ChartError> {
        match value {
            "line" => Ok(Self::Line),
            "bar" => Ok(Self::Bar),
            "scatter" => Ok(Self::Scatter),
            _ => Err(ChartError(format!(
                "chart type must be line, bar, or scatter; received {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ChartPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ChartAxis {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ChartSeries {
    pub name: String,
    pub points: Vec<ChartPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ChartDocument {
    pub title: String,
    pub kind: ChartKind,
    pub x_axis: ChartAxis,
    pub y_axis: ChartAxis,
    pub series: Vec<ChartSeries>,
}

impl ChartDocument {
    pub fn validate(&self) -> Result<(), ChartError> {
        validate_text("chart title", &self.title)?;
        validate_axis("x axis", &self.x_axis)?;
        validate_axis("y axis", &self.y_axis)?;
        if self.series.is_empty() || self.series.len() > MAX_CHART_SERIES {
            return Err(ChartError(format!(
                "chart must contain 1–{MAX_CHART_SERIES} series"
            )));
        }
        let mut names = BTreeSet::new();
        let mut point_count = 0usize;
        for series in &self.series {
            validate_text("series name", &series.name)?;
            if !names.insert(series.name.as_str()) {
                return Err(ChartError(format!(
                    "chart series name {:?} is duplicated",
                    series.name
                )));
            }
            if series.points.is_empty() {
                return Err(ChartError(format!(
                    "chart series {:?} has no points",
                    series.name
                )));
            }
            point_count = point_count.saturating_add(series.points.len());
            if point_count > MAX_CHART_POINTS {
                return Err(ChartError(format!(
                    "chart exceeds {MAX_CHART_POINTS} total points"
                )));
            }
            if series
                .points
                .iter()
                .any(|point| !point.x.is_finite() || !point.y.is_finite())
            {
                return Err(ChartError("chart points must be finite".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartError(pub String);

impl fmt::Display for ChartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl error::Error for ChartError {}

pub fn parse_delimited(
    content: &str,
    delimiter: u8,
    title: &str,
    kind: ChartKind,
) -> Result<ChartDocument, ChartError> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .trim(csv::Trim::All)
        .from_reader(content.as_bytes());
    let rows = reader
        .records()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ChartError(format!("invalid chart data: {error}")))?;
    let first = rows
        .first()
        .ok_or_else(|| ChartError("chart data is empty".into()))?;
    if first.len() < 2 {
        return Err(ChartError(
            "chart data needs an x column and at least one value column".into(),
        ));
    }
    if first.len() - 1 > MAX_CHART_SERIES {
        return Err(ChartError(format!(
            "chart data exceeds {MAX_CHART_SERIES} value columns"
        )));
    }

    let has_header = first.iter().any(|value| value.parse::<f32>().is_err());
    let (headers, data_rows) = if has_header {
        (
            first.iter().map(str::to_owned).collect::<Vec<_>>(),
            &rows[1..],
        )
    } else {
        let mut headers = vec!["x".to_owned()];
        headers.extend((1..first.len()).map(|index| format!("Series {index}")));
        (headers, rows.as_slice())
    };
    if data_rows.is_empty() {
        return Err(ChartError("chart data has headers but no points".into()));
    }

    let mut series = headers[1..]
        .iter()
        .map(|name| ChartSeries {
            name: split_axis(name).0,
            points: Vec::with_capacity(data_rows.len()),
        })
        .collect::<Vec<_>>();
    for (row_index, row) in data_rows.iter().enumerate() {
        if row.len() != headers.len() {
            return Err(ChartError(format!(
                "chart row {} has {} columns; expected {}",
                row_index + usize::from(has_header) + 1,
                row.len(),
                headers.len()
            )));
        }
        let x = number(row.get(0).unwrap_or_default(), row_index, "x")?;
        for (index, value) in row.iter().skip(1).enumerate() {
            series[index].points.push(ChartPoint {
                x,
                y: number(value, row_index, &headers[index + 1])?,
            });
        }
    }
    let (x_label, x_unit) = split_axis(&headers[0]);
    let common_y_unit = headers[1..]
        .iter()
        .map(|header| split_axis(header).1)
        .reduce(|left, right| if left == right { left } else { None })
        .flatten();
    let document = ChartDocument {
        title: title.to_owned(),
        kind,
        x_axis: ChartAxis {
            label: x_label,
            unit: x_unit,
        },
        y_axis: ChartAxis {
            label: "Value".into(),
            unit: common_y_unit,
        },
        series,
    };
    document.validate()?;
    Ok(document)
}

pub fn parse_json(
    content: &str,
    fallback_title: &str,
    fallback_kind: ChartKind,
) -> Result<ChartDocument, ChartError> {
    let value: serde_json::Value =
        serde_json::from_str(content).map_err(|error| ChartError(error.to_string()))?;
    if value.is_object() {
        let document: ChartDocument =
            serde_json::from_value(value).map_err(|error| ChartError(error.to_string()))?;
        document.validate()?;
        return Ok(document);
    }
    let rows = value
        .as_array()
        .ok_or_else(|| ChartError("chart JSON must be a chart document or x/y array".into()))?;
    let mut points = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let x = row
            .get("x")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| ChartError(format!("chart row {} needs numeric x", index + 1)))?
            as f32;
        let y = row
            .get("y")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| ChartError(format!("chart row {} needs numeric y", index + 1)))?
            as f32;
        points.push(ChartPoint { x, y });
    }
    let document = ChartDocument {
        title: fallback_title.to_owned(),
        kind: fallback_kind,
        x_axis: ChartAxis {
            label: "x".into(),
            unit: None,
        },
        y_axis: ChartAxis {
            label: "Value".into(),
            unit: None,
        },
        series: vec![ChartSeries {
            name: "Series 1".into(),
            points,
        }],
    };
    document.validate()?;
    Ok(document)
}

fn number(value: &str, row_index: usize, column: &str) -> Result<f32, ChartError> {
    let number = value.parse::<f32>().map_err(|_| {
        ChartError(format!(
            "chart row {} column {column:?} is not numeric",
            row_index + 1
        ))
    })?;
    if number.is_finite() {
        Ok(number)
    } else {
        Err(ChartError(format!(
            "chart row {} column {column:?} is not finite",
            row_index + 1
        )))
    }
}

fn split_axis(value: &str) -> (String, Option<String>) {
    let value = value.trim();
    if let Some(open) = value.rfind(" (")
        && value.ends_with(')')
    {
        let label = value[..open].trim();
        let unit = value[open + 2..value.len() - 1].trim();
        if !label.is_empty() && !unit.is_empty() {
            return (label.to_owned(), Some(unit.to_owned()));
        }
    }
    (value.to_owned(), None)
}

fn validate_axis(name: &str, axis: &ChartAxis) -> Result<(), ChartError> {
    validate_text(name, &axis.label)?;
    if let Some(unit) = &axis.unit {
        validate_text("axis unit", unit)?;
    }
    Ok(())
}

fn validate_text(name: &str, value: &str) -> Result<(), ChartError> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_TEXT
        || value.chars().any(char::is_control)
    {
        return Err(ChartError(format!(
            "{name} must be printable and 1–{MAX_TEXT} characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimited_headers_create_named_series_axes_and_units() {
        let chart = parse_delimited(
            "Week,Revenue ($),Cost ($)\n1,100,60\n2,140,75\n",
            b',',
            "Margin",
            ChartKind::Line,
        )
        .unwrap();
        assert_eq!(chart.x_axis.label, "Week");
        assert_eq!(chart.y_axis.unit.as_deref(), Some("$"));
        assert_eq!(chart.series[0].name, "Revenue");
        assert_eq!(chart.series[1].points[1].y, 75.0);
    }

    #[test]
    fn quoted_csv_uses_library_parser_and_legacy_numeric_rows_still_work() {
        let quoted = parse_delimited(
            "Time,\"Gross, USD\"\n1,12\n",
            b',',
            "Quoted",
            ChartKind::Bar,
        )
        .unwrap();
        assert_eq!(quoted.series[0].name, "Gross, USD");

        let legacy = parse_delimited("1,2\n3,4\n", b',', "Legacy", ChartKind::Line).unwrap();
        assert_eq!(legacy.series[0].points.len(), 2);
    }

    #[test]
    fn structured_json_round_trips_and_invalid_series_fail_loudly() {
        let json = r#"{
          "title":"Latency", "kind":"line",
          "x_axis":{"label":"Time","unit":"s"},
          "y_axis":{"label":"Latency","unit":"ms"},
          "series":[{"name":"API","points":[{"x":1.0,"y":20.0}]}]
        }"#;
        let chart = parse_json(json, "ignored", ChartKind::Bar).unwrap();
        assert_eq!(chart.title, "Latency");
        assert_eq!(chart.y_axis.unit.as_deref(), Some("ms"));

        let mut invalid = chart;
        invalid.series.push(invalid.series[0].clone());
        assert!(invalid.validate().unwrap_err().0.contains("duplicated"));
    }
}
