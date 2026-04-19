//! Core MCP protocol logic and shared parsing types for swiss-weather-mcp.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── MCP Wire Types ──────────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct McpRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct McpResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    pub result: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
}

impl McpResponse {
    pub fn ok(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            result,
            error: None,
        }
    }

    pub fn err(id: serde_json::Value, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            result: serde_json::json!({}),
            error: Some(McpError {
                code,
                message: message.to_string(),
            }),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct McpError {
    pub code: i32,
    pub message: String,
}

// ── Initialize ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: McpCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct McpCapabilities {
    #[serde(rename = "tools")]
    pub tools: ToolCapabilities,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ToolCapabilities {
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

// ── Tools ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CallResult {
    pub content: Vec<ContentBlock>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

impl ContentBlock {
    pub fn text(text: &str) -> Self {
        Self {
            content_type: "text".to_string(),
            text: text.to_string(),
        }
    }
}

impl CallResult {
    pub fn from_json_str(json: &str) -> Self {
        Self {
            content: vec![ContentBlock::text(json)],
        }
    }

    pub fn error(message: &str) -> Self {
        Self {
            content: vec![ContentBlock::text(&format!("Error: {}", message))],
        }
    }
}

// ── CSV Parsing ─────────────────────────────────────────────────────────────

pub fn parse_csv_response(text: &str) -> Result<Vec<HashMap<String, String>>> {
    let cleaned = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut records = Vec::new();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .from_reader(cleaned.as_bytes());

    for result in reader.deserialize::<HashMap<String, String>>() {
        let record = result.map_err(|e| anyhow::anyhow!(e))?;
        records.push(record);
    }

    Ok(records)
}

// ── MeteoSwiss Source Types ─────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct GeoJsonDataset {
    #[serde(default)]
    pub mapname: String,
    #[serde(default)]
    pub map_long_name: String,
    #[serde(default)]
    pub creation_time: String,
    #[serde(default)]
    pub features: Vec<GeoJsonFeature>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GeoJsonFeature {
    pub id: String,
    pub geometry: GeoJsonPointGeometry,
    pub properties: GeoJsonFeatureProperties,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GeoJsonPointGeometry {
    #[serde(default)]
    pub coordinates: Vec<f64>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GeoJsonFeatureProperties {
    pub station_name: String,
    pub value: serde_json::Value,
    pub unit: String,
    pub reference_ts: String,
    pub altitude: String,
    pub measurement_height: String,
    #[serde(default)]
    pub value_rel: Option<f64>,
    #[serde(default)]
    pub wind_direction: Option<i64>,
    #[serde(default)]
    pub wind_direction_radian: Option<f64>,
}

#[derive(Deserialize, Debug)]
pub struct StacItemCollection {
    #[serde(default)]
    pub features: Vec<StacItem>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct StacItem {
    pub id: String,
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub assets: HashMap<String, StacAsset>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct StacAsset {
    pub href: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StationInfo {
    pub id: String,
    pub station_name: String,
}

// ── Handlers ────────────────────────────────────────────────────────────────

pub const MCP_VERSION: &str = "2025-03-26";

pub fn tools_list_response(id: serde_json::Value) -> McpResponse {
    let tools = vec![
        ToolInfo {
            name: "list_weather_stations".to_string(),
            description: "List MeteoSwiss current-weather station IDs and names from the latest 10-minute precipitation layer."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        },
        ToolInfo {
            name: "get_precipitation".to_string(),
            description: "Get MeteoSwiss precipitation measurements from the official current layers.\n\nParameters:\n- interval: '10min' (default), '1h', '24h', or '48h'\n- station_id: Optional station ID filter such as 'ARO' or 'RAG'."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "interval": {
                        "type": "string",
                        "enum": ["10min", "1h", "24h", "48h"],
                        "description": "Measurement interval. Defaults to '10min'.",
                    },
                    "station_id": {
                        "type": "string",
                        "description": "Optional MeteoSwiss station ID filter.",
                    },
                },
            }),
        },
        ToolInfo {
            name: "get_wind_speed_10min".to_string(),
            description: "Get the MeteoSwiss 10-minute mean wind speed layer. Optionally filter by station ID."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "station_id": {
                        "type": "string",
                        "description": "Optional MeteoSwiss station ID filter.",
                    },
                },
            }),
        },
        ToolInfo {
            name: "get_wind_gusts_10min".to_string(),
            description: "Get the MeteoSwiss 10-minute maximum 1-second wind gust layer. Optionally filter by station ID."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "station_id": {
                        "type": "string",
                        "description": "Optional MeteoSwiss station ID filter.",
                    },
                },
            }),
        },
        ToolInfo {
            name: "get_sunshine".to_string(),
            description: "Get MeteoSwiss sunshine duration measurements.\n\nParameters:\n- interval: '10min' (default) or '1d'\n- station_id: Optional MeteoSwiss station ID filter such as 'ARO' or 'RAG'."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "interval": {
                        "type": "string",
                        "enum": ["10min", "1d"],
                        "description": "Measurement interval. Defaults to '10min'.",
                    },
                    "station_id": {
                        "type": "string",
                        "description": "Optional MeteoSwiss station ID filter.",
                    },
                },
            }),
        },
        ToolInfo {
            name: "get_global_radiation".to_string(),
            description: "Get MeteoSwiss global radiation measurements.\n\nParameters:\n- interval: '10min' (default) or '1d'\n- station_id: Optional MeteoSwiss station ID filter such as 'ARO' or 'RAG'."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "interval": {
                        "type": "string",
                        "enum": ["10min", "1d"],
                        "description": "Measurement interval. Defaults to '10min'.",
                    },
                    "station_id": {
                        "type": "string",
                        "description": "Optional MeteoSwiss station ID filter.",
                    },
                },
            }),
        },
        ToolInfo {
            name: "list_pollen_stations".to_string(),
            description: "List MeteoSwiss pollen monitoring stations from the official pollen network dataset."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        },
        ToolInfo {
            name: "get_pollen_measurement".to_string(),
            description: "Get the latest hourly species-specific pollen measurements.\n\nParameters:\n- species: one of 'alder', 'ash', 'beech', 'birch', 'grass', 'hazel', or 'oak'\n- station_id: Optional pollen station ID filter such as 'PBE' or 'PLO'."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "species": {
                        "type": "string",
                        "enum": ["alder", "ash", "beech", "birch", "grass", "hazel", "oak"],
                        "description": "Pollen species to query.",
                    },
                    "station_id": {
                        "type": "string",
                        "description": "Optional pollen station ID filter.",
                    },
                },
                "required": ["species"],
            }),
        },
        ToolInfo {
            name: "get_local_forecast".to_string(),
            description: "Get the latest MeteoSwiss local forecast for a point with a daily summary and hourly breakdown. The response includes temperature, precipitation totals and probability, wind, gusts, wind direction, cloud cover, sunshine duration, global radiation, diffuse radiation, and weather icon codes.\n\nParameters:\n- point_query: point ID, station abbreviation, postal code, or point name\n- hours: Optional number of hourly forecast timestamps to return (default 24)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "point_query": {
                        "type": "string",
                        "description": "Point ID, station abbreviation, postal code, or point name.",
                    },
                    "hours": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 96,
                        "description": "How many forecast timestamps to return. Defaults to 24.",
                    },
                },
                "required": ["point_query"],
            }),
        },
    ];
    McpResponse::ok(
        id,
        serde_json::json!({
            "tools": tools,
        }),
    )
}

pub fn initialize_response(id: serde_json::Value) -> McpResponse {
    let result = InitializeResult {
        protocol_version: MCP_VERSION.to_string(),
        capabilities: McpCapabilities {
            tools: ToolCapabilities {
                list_changed: false,
            },
        },
        server_info: ServerInfo {
            name: "swiss-weather-mcp".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    };
    McpResponse::ok(
        id,
        serde_json::to_value(&result).unwrap_or_else(|_| serde_json::json!({})),
    )
}
