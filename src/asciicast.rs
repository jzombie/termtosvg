use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AsciiCastError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsciiCastV2Theme {
    pub fg: String,
    pub bg: String,
    pub palette: String,
}

impl AsciiCastV2Theme {
    pub fn new(fg: &str, bg: &str, palette: &str) -> Result<Self, AsciiCastError> {
        if !Self::is_color(fg) {
            return Err(AsciiCastError::Message(format!(
                "Invalid foreground color: {}",
                fg
            )));
        }
        if !Self::is_color(bg) {
            return Err(AsciiCastError::Message(format!(
                "Invalid background color: {}",
                bg
            )));
        }
        let parts: Vec<&str> = palette.split(':').collect();
        if !(parts.len() >= 8 && parts.len() <= 256) {
            return Err(AsciiCastError::Message(
                "Invalid palette: expecting at least 8 colors".into(),
            ));
        }
        let palette_valid = if parts.len() >= 16 {
            parts.iter().take(16).all(|p| Self::is_color(p))
        } else {
            parts.iter().take(8).all(|p| Self::is_color(p))
        };
        if !palette_valid {
            return Err(AsciiCastError::Message(
                "Invalid palette: the first 8 or 16 colors must be valid".into(),
            ));
        }

        Ok(Self {
            fg: fg.to_string(),
            bg: bg.to_string(),
            palette: if parts.len() >= 16 {
                parts.iter().take(16).cloned().collect::<Vec<_>>().join(":")
            } else {
                parts.iter().take(8).cloned().collect::<Vec<_>>().join(":")
            },
        })
    }

    pub fn is_color(value: &str) -> bool {
        if value.len() != 7 || !value.starts_with('#') {
            return false;
        }
        u32::from_str_radix(&value[1..], 16).is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsciiCastV2Header {
    pub version: u8,
    pub width: u16,
    pub height: u16,
    pub theme: Option<AsciiCastV2Theme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_time_limit: Option<f64>,
}

impl AsciiCastV2Header {
    pub fn new(
        version: u8,
        width: u16,
        height: u16,
        theme: Option<AsciiCastV2Theme>,
        idle_time_limit: Option<f64>,
    ) -> Result<Self, AsciiCastError> {
        if version != 2 {
            return Err(AsciiCastError::Message(
                "Only asciicast v2 format is supported".into(),
            ));
        }
        Ok(Self {
            version,
            width,
            height,
            theme,
            idle_time_limit,
        })
    }

    pub fn to_json_line(&self) -> Result<String, AsciiCastError> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json_line(line: &str) -> Result<Self, AsciiCastError> {
        let value: Value = serde_json::from_str(line)?;
        if !value.is_object() {
            return Err(AsciiCastError::Message("Unknown record type".into()));
        }
        let header: AsciiCastV2Header = serde_json::from_value(value)?;
        if header.version != 2 {
            return Err(AsciiCastError::Message(
                "Only asciicast v2 format is supported".into(),
            ));
        }
        Ok(header)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsciiCastV2Event {
    pub time: f64,
    pub event_type: String,
    pub event_data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

impl AsciiCastV2Event {
    pub fn new(
        time: f64,
        event_type: &str,
        event_data: &str,
        duration: Option<f64>,
    ) -> Result<Self, AsciiCastError> {
        if !(event_type == "o" || event_type == "i") {
            return Err(AsciiCastError::Message(format!(
                "Invalid event_type: {}",
                event_type
            )));
        }
        Ok(Self {
            time,
            event_type: event_type.to_string(),
            event_data: event_data.to_string(),
            duration,
        })
    }

    pub fn to_json_line(&self) -> Result<String, AsciiCastError> {
        let array =
            serde_json::json!([self.time, self.event_type.clone(), self.event_data.clone()]);
        Ok(serde_json::to_string(&array)?)
    }

    pub fn from_json_line(line: &str) -> Result<Self, AsciiCastError> {
        let value: Value = serde_json::from_str(line)?;
        if !value.is_array() {
            return Err(AsciiCastError::Message("Unknown record type".into()));
        }
        let arr = value.as_array().unwrap();
        if arr.len() < 3 {
            return Err(AsciiCastError::Message("Invalid event".into()));
        }
        let time = arr[0]
            .as_f64()
            .or_else(|| arr[0].as_i64().map(|v| v as f64))
            .ok_or_else(|| AsciiCastError::Message("Invalid time".into()))?;
        let event_type = arr[1]
            .as_str()
            .ok_or_else(|| AsciiCastError::Message("Invalid event_type".into()))?;
        let event_data = arr[2]
            .as_str()
            .ok_or_else(|| AsciiCastError::Message("Invalid event_data".into()))?;
        AsciiCastV2Event::new(time, event_type, event_data, None)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AsciiCastV2Record {
    Header(AsciiCastV2Header),
    Event(AsciiCastV2Event),
}

impl AsciiCastV2Record {
    pub fn to_json_line(&self) -> Result<String, AsciiCastError> {
        match self {
            AsciiCastV2Record::Header(h) => h.to_json_line(),
            AsciiCastV2Record::Event(e) => e.to_json_line(),
        }
    }

    pub fn from_json_line(line: &str) -> Result<Self, AsciiCastError> {
        let value: Value = serde_json::from_str(line)?;
        match value {
            Value::Object(_) => Ok(AsciiCastV2Record::Header(
                AsciiCastV2Header::from_json_line(line)?,
            )),
            Value::Array(_) => Ok(AsciiCastV2Record::Event(AsciiCastV2Event::from_json_line(
                line,
            )?)),
            _ => Err(AsciiCastError::Message(format!(
                "Unknown record type: {}",
                line
            ))),
        }
    }
}

pub fn read_records<P: AsRef<Path>>(path: P) -> Result<Vec<AsciiCastV2Record>, AsciiCastError> {
    let file = File::open(path.as_ref())?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();
    let mut records = Vec::new();

    // First try v2 line by line
    while reader.read_line(&mut buf)? != 0 {
        let line = buf.trim_end_matches(['\n', '\r'].as_ref());
        if line.is_empty() {
            buf.clear();
            continue;
        }
        match AsciiCastV2Record::from_json_line(line) {
            Ok(rec) => records.push(rec),
            Err(err) => {
                // Fall back to v1 parsing using full file contents
                return read_v1_records(path).map_err(|_| err);
            }
        }
        buf.clear();
    }
    Ok(records)
}

fn read_v1_records<P: AsRef<Path>>(path: P) -> Result<Vec<AsciiCastV2Record>, AsciiCastError> {
    let content = std::fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&content)?;
    let header_keys = ["version", "width", "height", "stdout"];
    if !header_keys.iter().all(|k| value.get(k).is_some()) {
        return Err(AsciiCastError::Message(
            "Missing attributes in asciicast v1 file".into(),
        ));
    }
    if value["version"].as_i64() != Some(1) {
        return Err(AsciiCastError::Message(
            "This function can only decode asciicast v1 data".into(),
        ));
    }

    let width = value["width"]
        .as_u64()
        .ok_or_else(|| AsciiCastError::Message("Invalid width".into()))? as u16;
    let height = value["height"]
        .as_u64()
        .ok_or_else(|| AsciiCastError::Message("Invalid height".into()))? as u16;

    let mut events = Vec::new();
    events.push(AsciiCastV2Record::Header(AsciiCastV2Header::new(
        2, width, height, None, None,
    )?));

    let stdout_events = value["stdout"]
        .as_array()
        .ok_or_else(|| AsciiCastError::Message("Invalid stdout attribute".into()))?;
    let mut elapsed = 0.0_f64;
    for ev in stdout_events {
        if !ev.is_array() || ev.as_array().unwrap().len() != 2 {
            return Err(AsciiCastError::Message("Invalid event".into()));
        }
        let parts = ev.as_array().unwrap();
        let time_delta = parts[0]
            .as_f64()
            .or_else(|| parts[0].as_i64().map(|v| v as f64))
            .ok_or_else(|| AsciiCastError::Message("Invalid event duration".into()))?;
        let data = parts[1]
            .as_str()
            .ok_or_else(|| AsciiCastError::Message("Invalid event".into()))?;
        elapsed += time_delta;
        events.push(AsciiCastV2Record::Event(AsciiCastV2Event::new(
            elapsed, "o", data, None,
        )?));
    }

    Ok(events)
}
