use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use tokio::io::{AsyncBufReadExt, BufReader};

mod mcp_engine;

const DATA_ROOT: &str = "https://data.geo.admin.ch";
const STAC_ROOT: &str = "https://data.geo.admin.ch/api/stac/v0.9";
const POLLEN_COLLECTION_ID: &str = "ch.meteoschweiz.ogd-pollen";
const LOCAL_FORECAST_COLLECTION_ID: &str = "ch.meteoschweiz.ogd-local-forecasting";
const LOCAL_FORECAST_POINT_METADATA_URL: &str =
    "https://data.geo.admin.ch/ch.meteoschweiz.ogd-local-forecasting/ogd-local-forecasting_meta_point.csv";
const CAMS_WMS_URL: &str = "https://eccharts.ecmwf.int/wms/";
const UV_INDEX_UNAVAILABLE: &str =
    "Copernicus CAMS UV lookup is unavailable for this point or time window.";
const AIR_QUALITY_AQI_UNAVAILABLE: &str =
    "No verified official AQI feed has been integrated. The checked NABEL source exposes station-network metadata, not a ready-made AQI forecast or measurement layer.";

struct CurrentLayerSpec {
    dataset_path: &'static str,
    missing_value: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
struct ForecastPoint {
    point_id: String,
    station_abbr: Option<String>,
    postal_code: Option<String>,
    point_name: String,
    point_type_en: String,
    point_height_masl: Option<f64>,
    point_coordinates_lv95_east: Option<f64>,
    point_coordinates_lv95_north: Option<f64>,
    point_coordinates_wgs84_lat: Option<f64>,
    point_coordinates_wgs84_lon: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectedForecastAsset {
    issue_timestamp: String,
    href: String,
}

struct ForecastBundleParameter {
    code: &'static str,
    field_name: &'static str,
    label: &'static str,
    unit: &'static str,
}

#[derive(Debug, PartialEq)]
struct CamsFeatureInfo {
    layer_name: String,
    title: String,
    value: f64,
    input_latitude: f64,
    input_longitude: f64,
    distance_km: f64,
    grid_point_latitude: f64,
    grid_point_longitude: f64,
}

const HOURLY_FORECAST_BUNDLE: &[ForecastBundleParameter] = &[
    ForecastBundleParameter {
        code: "tre200h0",
        field_name: "temperature_2m_c",
        label: "Air temperature 2 m above ground; hourly mean",
        unit: "C",
    },
    ForecastBundleParameter {
        code: "rre150h0",
        field_name: "precipitation_hourly_mm",
        label: "Precipitation; hourly total",
        unit: "mm",
    },
    ForecastBundleParameter {
        code: "rp0003i0",
        field_name: "precipitation_probability_3h_percent",
        label: "Probability of precipitation during 3 hours",
        unit: "%",
    },
    ForecastBundleParameter {
        code: "dkl010h0",
        field_name: "wind_direction_degrees",
        label: "Wind direction; hourly mean",
        unit: "degrees",
    },
    ForecastBundleParameter {
        code: "fu3010h0",
        field_name: "wind_speed_kmh",
        label: "Wind speed scalar; hourly mean",
        unit: "km/h",
    },
    ForecastBundleParameter {
        code: "fu3010h1",
        field_name: "wind_gust_kmh",
        label: "Gust peak (one second); hourly maximum",
        unit: "km/h",
    },
    ForecastBundleParameter {
        code: "jww003i0",
        field_name: "weather_icon_code",
        label: "MeteoSwiss weather icon code",
        unit: "code",
    },
    ForecastBundleParameter {
        code: "nprolohs",
        field_name: "low_cloud_cover",
        label: "Low cloud cover",
        unit: "fraction",
    },
    ForecastBundleParameter {
        code: "npromths",
        field_name: "medium_cloud_cover",
        label: "Medium cloud cover",
        unit: "fraction",
    },
    ForecastBundleParameter {
        code: "nprohihs",
        field_name: "high_cloud_cover",
        label: "High cloud cover",
        unit: "fraction",
    },
    ForecastBundleParameter {
        code: "sre000h0",
        field_name: "sunshine_duration_minutes",
        label: "Sunshine duration; hourly total",
        unit: "min",
    },
    ForecastBundleParameter {
        code: "gre000h0",
        field_name: "global_radiation_w_m2",
        label: "Global radiation; hourly mean",
        unit: "W/m2",
    },
    ForecastBundleParameter {
        code: "ods000h0",
        field_name: "diffuse_radiation_w_m2",
        label: "Diffuse radiation; hourly mean",
        unit: "W/m2",
    },
];

const DAILY_FORECAST_BUNDLE: &[ForecastBundleParameter] = &[
    ForecastBundleParameter {
        code: "tre200dn",
        field_name: "temperature_min_c",
        label: "Air temperature 2 m above ground; daily minimum",
        unit: "C",
    },
    ForecastBundleParameter {
        code: "tre200dx",
        field_name: "temperature_max_c",
        label: "Air temperature 2 m above ground; daily maximum",
        unit: "C",
    },
    ForecastBundleParameter {
        code: "rka150p0",
        field_name: "precipitation_total_mm",
        label: "Precipitation; daily total 00:00 - 24:00 local time",
        unit: "mm",
    },
    ForecastBundleParameter {
        code: "rreq10p0",
        field_name: "precipitation_total_10_percentile_mm",
        label: "Precipitation; daily total 10% quantile",
        unit: "mm",
    },
    ForecastBundleParameter {
        code: "rreq90p0",
        field_name: "precipitation_total_90_percentile_mm",
        label: "Precipitation; daily total 90% quantile",
        unit: "mm",
    },
    ForecastBundleParameter {
        code: "jp2000d0",
        field_name: "weather_icon_code",
        label: "MeteoSwiss pictogram number; daily value",
        unit: "code",
    },
];

// ── Handlers (async, calls external APIs) ───────────────────────────────────

async fn handle_request(request: mcp_engine::McpRequest) -> Option<mcp_engine::McpResponse> {
    let id = request.id.clone().unwrap_or(serde_json::json!(null));
    match request.method.as_str() {
        "initialize" => Some(mcp_engine::initialize_response(id)),
        "notifications/initialized" => None,
        "tools/list" => Some(mcp_engine::tools_list_response(id)),
        "tools/call" => {
            let name = match request.params.get("name") {
                Some(Value::String(s)) => s,
                _ => {
                    return Some(mcp_engine::McpResponse::err(
                        id.clone(),
                        -32602,
                        "Missing argument 'name'",
                    ));
                }
            };
            let args = request.params.get("arguments").cloned().unwrap_or_default();

            let call_result = match name.as_str() {
                "list_weather_stations" => run_json_tool(list_weather_stations().await),
                "get_precipitation" => {
                    let interval = args.get("interval").and_then(Value::as_str);
                    let station_id = args.get("station_id").and_then(Value::as_str);
                    run_json_tool(get_precipitation(interval, station_id).await)
                }
                "get_wind_speed_10min" => {
                    let station_id = args.get("station_id").and_then(Value::as_str);
                    run_json_tool(get_wind_speed_10min(station_id).await)
                }
                "get_wind_gusts_10min" => {
                    let station_id = args.get("station_id").and_then(Value::as_str);
                    run_json_tool(get_wind_gusts_10min(station_id).await)
                }
                "get_sunshine" => {
                    let interval = args.get("interval").and_then(Value::as_str);
                    let station_id = args.get("station_id").and_then(Value::as_str);
                    run_json_tool(get_sunshine(interval, station_id).await)
                }
                "get_global_radiation" => {
                    let interval = args.get("interval").and_then(Value::as_str);
                    let station_id = args.get("station_id").and_then(Value::as_str);
                    run_json_tool(get_global_radiation(interval, station_id).await)
                }
                "list_pollen_stations" => run_json_tool(list_pollen_stations().await),
                "get_pollen_measurement" => {
                    let species = match args.get("species").and_then(Value::as_str) {
                        Some(species) => species,
                        None => {
                            return Some(mcp_engine::McpResponse::err(
                                id.clone(),
                                -32602,
                                "Missing argument 'species'",
                            ));
                        }
                    };
                    let station_id = args.get("station_id").and_then(Value::as_str);
                    run_json_tool(get_pollen_measurement(species, station_id).await)
                }
                "get_local_forecast" => {
                    let point_query = match args.get("point_query").and_then(Value::as_str) {
                        Some(point_query) => point_query,
                        None => {
                            return Some(mcp_engine::McpResponse::err(
                                id.clone(),
                                -32602,
                                "Missing argument 'point_query'",
                            ));
                        }
                    };
                    let hours = args.get("hours").and_then(Value::as_u64);
                    run_json_tool(get_local_forecast(point_query, hours).await)
                }
                _ => {
                    return Some(mcp_engine::McpResponse::err(
                        id.clone(),
                        -32601,
                        &format!("Unknown tool: {}", name),
                    ));
                }
            };

            Some(mcp_engine::McpResponse::ok(
                id,
                serde_json::to_value(&call_result).ok()?,
            ))
        }
        _ => Some(mcp_engine::McpResponse::err(
            id,
            -32601,
            &format!("Unsupported method: {}", request.method),
        )),
    }
}

fn run_json_tool(result: Result<Value>) -> mcp_engine::CallResult {
    match result {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(json) => mcp_engine::CallResult::from_json_str(&json),
            Err(error) => mcp_engine::CallResult::error(&error.to_string()),
        },
        Err(error) => mcp_engine::CallResult::error(&error.to_string()),
    }
}

// ── Network helpers ─────────────────────────────────────────────────────────

async fn fetch_json<T>(url: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("Failed to request {}", url))?
        .error_for_status()
        .with_context(|| format!("Non-success response from {}", url))?;

    response
        .json::<T>()
        .await
        .with_context(|| format!("Failed to decode JSON from {}", url))
}

async fn fetch_text(url: &str) -> Result<String> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("Failed to request {}", url))?
        .error_for_status()
        .with_context(|| format!("Non-success response from {}", url))?;

    response
        .text()
        .await
        .with_context(|| format!("Failed to decode text from {}", url))
}

// ── Current weather ─────────────────────────────────────────────────────────

async fn list_weather_stations() -> Result<Value> {
    let spec = precipitation_layer_spec(Some("10min"))?;
    let url = current_layer_file_url(spec.dataset_path);
    let dataset: mcp_engine::GeoJsonDataset = fetch_json(&url).await?;

    let mut stations = dataset
        .features
        .iter()
        .map(build_station_summary)
        .collect::<Vec<_>>();

    stations.sort_by(|left, right| {
        station_sort_key(left)
            .cmp(&station_sort_key(right))
            .then_with(|| left["id"].as_str().cmp(&right["id"].as_str()))
    });

    Ok(json!({
        "source_url": url,
        "updated_at": dataset.creation_time,
        "station_count": stations.len(),
        "stations": stations,
    }))
}

async fn get_precipitation(interval: Option<&str>, station_id: Option<&str>) -> Result<Value> {
    let spec = precipitation_layer_spec(interval)?;
    current_layer_measurements(spec, station_id).await
}

async fn get_wind_speed_10min(station_id: Option<&str>) -> Result<Value> {
    current_layer_measurements(
        CurrentLayerSpec {
            dataset_path: "ch.meteoschweiz.messwerte-windgeschwindigkeit-kmh-10min",
            missing_value: None,
        },
        station_id,
    )
    .await
}

async fn get_wind_gusts_10min(station_id: Option<&str>) -> Result<Value> {
    current_layer_measurements(
        CurrentLayerSpec {
            dataset_path: "ch.meteoschweiz.messwerte-wind-boeenspitze-kmh-10min",
            missing_value: None,
        },
        station_id,
    )
    .await
}

async fn get_sunshine(interval: Option<&str>, station_id: Option<&str>) -> Result<Value> {
    let spec = sunshine_layer_spec(interval)?;
    current_layer_measurements(spec, station_id).await
}

async fn get_global_radiation(interval: Option<&str>, station_id: Option<&str>) -> Result<Value> {
    let spec = global_radiation_layer_spec(interval)?;
    current_layer_measurements(spec, station_id).await
}

fn precipitation_layer_spec(interval: Option<&str>) -> Result<CurrentLayerSpec> {
    let normalized = interval.unwrap_or("10min").trim().to_ascii_lowercase();
    let dataset_path = match normalized.as_str() {
        "10min" => "ch.meteoschweiz.messwerte-niederschlag-10min",
        "1h" => "ch.meteoschweiz.messwerte-niederschlag-1h",
        "24h" => "ch.meteoschweiz.messwerte-niederschlag-24h",
        "48h" => "ch.meteoschweiz.messwerte-niederschlag-48h",
        other => bail!("Unsupported precipitation interval: {}", other),
    };

    Ok(CurrentLayerSpec {
        dataset_path,
        missing_value: None,
    })
}

fn sunshine_layer_spec(interval: Option<&str>) -> Result<CurrentLayerSpec> {
    let normalized = interval.unwrap_or("10min").trim().to_ascii_lowercase();
    let dataset_path = match normalized.as_str() {
        "10min" => "ch.meteoschweiz.messwerte-sonnenscheindauer-10min",
        "1d" => "ch.meteoschweiz.messwerte-sonnenscheindauer-relativ-1d",
        other => bail!("Unsupported sunshine interval: {}", other),
    };

    Ok(CurrentLayerSpec {
        dataset_path,
        missing_value: None,
    })
}

fn global_radiation_layer_spec(interval: Option<&str>) -> Result<CurrentLayerSpec> {
    let normalized = interval.unwrap_or("10min").trim().to_ascii_lowercase();
    let dataset_path = match normalized.as_str() {
        "10min" => "ch.meteoschweiz.messwerte-globalstrahlung-10min",
        "1d" => "ch.meteoschweiz.messwerte-globalstrahlung-1d",
        other => bail!("Unsupported global radiation interval: {}", other),
    };

    Ok(CurrentLayerSpec {
        dataset_path,
        missing_value: None,
    })
}

async fn current_layer_measurements(
    spec: CurrentLayerSpec,
    station_id: Option<&str>,
) -> Result<Value> {
    let url = current_layer_file_url(spec.dataset_path);
    let dataset: mcp_engine::GeoJsonDataset = fetch_json(&url).await?;

    let mut records = dataset
        .features
        .iter()
        .filter(|feature| station_matches(feature, station_id))
        .map(|feature| build_measurement_record(feature, spec.missing_value))
        .collect::<Vec<_>>();

    if let Some(query) = station_id {
        if records.is_empty() {
            bail!("No station matched '{}'", query);
        }
    }

    records.sort_by(|left, right| {
        station_sort_key(left)
            .cmp(&station_sort_key(right))
            .then_with(|| {
                left["station_id"]
                    .as_str()
                    .cmp(&right["station_id"].as_str())
            })
    });

    Ok(json!({
        "source_url": url,
        "layer": dataset.mapname,
        "title": dataset.map_long_name,
        "updated_at": dataset.creation_time,
        "record_count": records.len(),
        "records": records,
    }))
}

fn current_layer_file_url(dataset_path: &str) -> String {
    format!("{}/{}/{}_en.json", DATA_ROOT, dataset_path, dataset_path)
}

fn station_matches(feature: &mcp_engine::GeoJsonFeature, station_id: Option<&str>) -> bool {
    let Some(query) = station_id else {
        return true;
    };

    let normalized_query = query.trim().to_ascii_lowercase();
    let station_name = feature.properties.station_name.to_ascii_lowercase();
    feature.id.to_ascii_lowercase() == normalized_query
        || station_name == normalized_query
        || station_name.contains(&normalized_query)
}

fn build_station_summary(feature: &mcp_engine::GeoJsonFeature) -> Value {
    json!({
        "id": feature.id,
        "station_name": feature.properties.station_name,
        "coordinates_lv95": build_lv95_coordinates(&feature.geometry.coordinates),
    })
}

fn build_measurement_record(
    feature: &mcp_engine::GeoJsonFeature,
    missing_value: Option<f64>,
) -> Value {
    let mut record = Map::new();
    record.insert("station_id".to_string(), json!(feature.id));
    record.insert(
        "station_name".to_string(),
        json!(feature.properties.station_name),
    );
    record.insert(
        "value".to_string(),
        json!(parse_measurement_value(
            &feature.properties.value,
            missing_value
        )),
    );
    record.insert("unit".to_string(), json!(feature.properties.unit));
    record.insert(
        "reference_timestamp".to_string(),
        json!(normalize_reference_timestamp(
            &feature.properties.reference_ts
        )),
    );
    record.insert("altitude".to_string(), json!(feature.properties.altitude));
    record.insert(
        "measurement_height".to_string(),
        json!(feature.properties.measurement_height),
    );
    record.insert(
        "coordinates_lv95".to_string(),
        build_lv95_coordinates(&feature.geometry.coordinates),
    );
    record.insert(
        "wind_direction_degrees".to_string(),
        json!(feature.properties.wind_direction),
    );
    record.insert(
        "wind_direction_radian".to_string(),
        json!(feature.properties.wind_direction_radian),
    );

    if let Some(value_rel) = feature.properties.value_rel {
        record.insert("value_relative".to_string(), json!(value_rel));
    }

    Value::Object(record)
}

fn parse_measurement_value(value: &Value, missing_value: Option<f64>) -> Option<f64> {
    let parsed = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(string) => string.parse::<f64>().ok(),
        _ => None,
    };

    match (parsed, missing_value) {
        (Some(parsed), Some(missing)) if (parsed - missing).abs() < f64::EPSILON => None,
        _ => parsed,
    }
}

fn normalize_reference_timestamp(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        None
    } else {
        Some(trimmed)
    }
}

fn build_lv95_coordinates(coordinates: &[f64]) -> Value {
    if coordinates.len() >= 2 {
        json!({
            "east": coordinates[0],
            "north": coordinates[1],
        })
    } else {
        Value::Null
    }
}

fn station_sort_key(value: &Value) -> String {
    value["station_name"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

// ── Pollen ──────────────────────────────────────────────────────────────────

async fn list_pollen_stations() -> Result<Value> {
    let url = format!("{}/collections/{}/items", STAC_ROOT, POLLEN_COLLECTION_ID);
    let collection: mcp_engine::StacItemCollection = fetch_json(&url).await?;

    let mut stations = collection
        .features
        .iter()
        .map(|item| {
            let title = item
                .properties
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(item.id.as_str());

            mcp_engine::StationInfo {
                id: item.id.to_ascii_uppercase(),
                station_name: title.to_string(),
            }
        })
        .collect::<Vec<_>>();

    stations.sort_by(|left, right| {
        left.station_name
            .to_ascii_lowercase()
            .cmp(&right.station_name.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(json!({
        "source_url": url,
        "station_count": stations.len(),
        "stations": stations,
    }))
}

async fn get_pollen_measurement(species: &str, station_id: Option<&str>) -> Result<Value> {
    let dataset_path = pollen_dataset_path(species)?;
    current_layer_measurements(
        CurrentLayerSpec {
            dataset_path,
            missing_value: Some(99_999.0),
        },
        station_id,
    )
    .await
}

fn pollen_dataset_path(species: &str) -> Result<&'static str> {
    match species.trim().to_ascii_lowercase().as_str() {
        "alder" => Ok("ch.meteoschweiz.messwerte-pollen-erle-1h"),
        "ash" => Ok("ch.meteoschweiz.messwerte-pollen-esche-1h"),
        "beech" => Ok("ch.meteoschweiz.messwerte-pollen-buche-1h"),
        "birch" => Ok("ch.meteoschweiz.messwerte-pollen-birke-1h"),
        "grass" => Ok("ch.meteoschweiz.messwerte-pollen-graeser-1h"),
        "hazel" => Ok("ch.meteoschweiz.messwerte-pollen-hasel-1h"),
        "oak" => Ok("ch.meteoschweiz.messwerte-pollen-eiche-1h"),
        other => bail!("Unsupported pollen species: {}", other),
    }
}

// ── Local forecast ──────────────────────────────────────────────────────────

async fn get_local_forecast(point_query: &str, hours: Option<u64>) -> Result<Value> {
    let limit = hours.unwrap_or(24).clamp(1, 96) as usize;
    let points = load_local_forecast_points().await?;
    let point = resolve_forecast_point(&points, point_query)?;
    let selected_assets = latest_local_forecast_assets().await?;
    let issued_at = selected_assets
        .values()
        .map(|asset| asset.issue_timestamp.as_str())
        .max()
        .unwrap_or_default()
        .to_string();

    let hourly_breakdown = build_forecast_section(
        &point,
        &selected_assets,
        HOURLY_FORECAST_BUNDLE,
        "forecast_time",
        limit,
    )
    .await?;
    let daily_summary = build_forecast_section(
        &point,
        &selected_assets,
        DAILY_FORECAST_BUNDLE,
        "forecast_date",
        8,
    )
    .await?;

    if hourly_breakdown.is_empty() {
        bail!("No forecast rows matched point '{}'", point_query);
    }

    let uv_index = fetch_cams_uv_index_summary(&point, &daily_summary).await;
    let air_quality = unavailable_metric(AIR_QUALITY_AQI_UNAVAILABLE);
    let summary = build_forecast_summary(&hourly_breakdown, uv_index.clone(), air_quality.clone());
    let unsupported = build_unsupported(&uv_index, &air_quality);

    Ok(json!({
        "source_collection": LOCAL_FORECAST_COLLECTION_ID,
        "issued_at": issued_at,
        "point": build_forecast_point_json(&point),
        "daily_parameters": DAILY_FORECAST_BUNDLE.iter().map(build_parameter_metadata).collect::<Vec<_>>(),
        "hourly_parameters": HOURLY_FORECAST_BUNDLE.iter().map(build_parameter_metadata).collect::<Vec<_>>(),
        "daily_summary_count": daily_summary.len(),
        "hourly_breakdown_count": hourly_breakdown.len(),
        "daily_summary": daily_summary,
        "hourly_breakdown": hourly_breakdown,
        "summary": summary,
        "unsupported": unsupported,
    }))
}

fn build_forecast_summary(
    hourly_breakdown: &[Value],
    uv_index: Value,
    air_quality: Value,
) -> Value {
    json!({
        "temperature_2m_c": numeric_summary(hourly_breakdown, "temperature_2m_c"),
        "precipitation_hourly_mm": accumulation_summary(hourly_breakdown, "precipitation_hourly_mm"),
        "precipitation_probability_3h_percent": numeric_summary(hourly_breakdown, "precipitation_probability_3h_percent"),
        "wind_speed_kmh": numeric_summary(hourly_breakdown, "wind_speed_kmh"),
        "wind_gust_kmh": numeric_summary(hourly_breakdown, "wind_gust_kmh"),
        "sunshine_duration_minutes": accumulation_summary(hourly_breakdown, "sunshine_duration_minutes"),
        "global_radiation_w_m2": numeric_summary(hourly_breakdown, "global_radiation_w_m2"),
        "diffuse_radiation_w_m2": numeric_summary(hourly_breakdown, "diffuse_radiation_w_m2"),
        "cloud_cover_fraction": {
            "low": numeric_summary(hourly_breakdown, "low_cloud_cover"),
            "medium": numeric_summary(hourly_breakdown, "medium_cloud_cover"),
            "high": numeric_summary(hourly_breakdown, "high_cloud_cover"),
        },
        "uv_index": uv_index,
        "air_quality_aqi": air_quality,
    })
}

fn unavailable_metric(reason: &str) -> Value {
    json!({
        "available": false,
        "reason": reason,
    })
}

fn build_unsupported(uv_index: &Value, air_quality: &Value) -> Value {
    let mut unsupported = Map::new();

    if !uv_index
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        unsupported.insert(
            "uv_index".to_string(),
            uv_index
                .get("reason")
                .cloned()
                .unwrap_or_else(|| json!(UV_INDEX_UNAVAILABLE)),
        );
    }

    if !air_quality
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        unsupported.insert(
            "air_quality_aqi".to_string(),
            air_quality
                .get("reason")
                .cloned()
                .unwrap_or_else(|| json!(AIR_QUALITY_AQI_UNAVAILABLE)),
        );
    }

    Value::Object(unsupported)
}

fn numeric_summary(rows: &[Value], field_name: &str) -> Value {
    let values = rows
        .iter()
        .filter_map(|row| row.get(field_name).and_then(Value::as_f64))
        .collect::<Vec<_>>();

    if values.is_empty() {
        return Value::Null;
    }

    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sum = values.iter().sum::<f64>();
    let mean = sum / values.len() as f64;

    json!({
        "min": round_to_precision(min, 1),
        "max": round_to_precision(max, 1),
        "mean": round_to_precision(mean, 1),
    })
}

fn accumulation_summary(rows: &[Value], field_name: &str) -> Value {
    let values = rows
        .iter()
        .filter_map(|row| row.get(field_name).and_then(Value::as_f64))
        .collect::<Vec<_>>();

    if values.is_empty() {
        return Value::Null;
    }

    let total = values.iter().sum::<f64>();
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    json!({
        "total": round_to_precision(total, 1),
        "max": round_to_precision(max, 1),
    })
}

fn round_to_precision(value: f64, decimals: u32) -> f64 {
    let factor = 10_f64.powi(decimals as i32);
    (value * factor).round() / factor
}

async fn fetch_cams_uv_index_summary(point: &ForecastPoint, daily_summary: &[Value]) -> Value {
    match try_fetch_cams_uv_index_summary(point, daily_summary).await {
        Ok(summary) => summary,
        Err(error) => unavailable_metric(&format!("{} {}", UV_INDEX_UNAVAILABLE, error)),
    }
}

async fn try_fetch_cams_uv_index_summary(
    point: &ForecastPoint,
    daily_summary: &[Value],
) -> Result<Value> {
    let latitude = point
        .point_coordinates_wgs84_lat
        .ok_or_else(|| anyhow!("Point metadata does not include WGS84 latitude"))?;
    let longitude = point
        .point_coordinates_wgs84_lon
        .ok_or_else(|| anyhow!("Point metadata does not include WGS84 longitude"))?;

    let mut daily_max_forecast = Vec::new();
    let mut daily_max_rows = Vec::new();
    let mut last_error = None;

    for forecast_date in daily_summary
        .iter()
        .filter_map(|row| row.get("forecast_date").and_then(Value::as_str))
        .take(3)
    {
        let time_utc = compact_date_to_cams_day_time(forecast_date)?;

        match fetch_cams_wms_feature_info(
            "composition_uvindex_daily_max",
            &time_utc,
            latitude,
            longitude,
        )
        .await
        {
            Ok(feature_info) => {
                let rounded_value = round_to_precision(feature_info.value, 1);
                daily_max_rows.push(json!({ "value": rounded_value }));
                daily_max_forecast.push(json!({
                    "forecast_date": forecast_date,
                    "time_utc": time_utc,
                    "value": rounded_value,
                    "nearest_grid_point": {
                        "latitude": round_to_precision(feature_info.grid_point_latitude, 1),
                        "longitude": round_to_precision(feature_info.grid_point_longitude, 1),
                        "distance_km": round_to_precision(feature_info.distance_km, 1),
                    }
                }));
            }
            Err(error) => last_error = Some(error),
        }
    }

    if daily_max_forecast.is_empty() {
        if let Some(error) = last_error {
            return Err(error);
        }
        bail!("No CAMS UV daily-max values were returned");
    }

    Ok(json!({
        "available": true,
        "source": {
            "provider": "Copernicus CAMS",
            "service": "ECMWF public WMS",
            "layer": "composition_uvindex_daily_max",
            "url": CAMS_WMS_URL,
        },
        "daily_max_summary": numeric_summary(&daily_max_rows, "value"),
        "daily_max_forecast": daily_max_forecast,
    }))
}

async fn fetch_cams_wms_feature_info(
    layer_name: &str,
    time_utc: &str,
    latitude: f64,
    longitude: f64,
) -> Result<CamsFeatureInfo> {
    let bbox = format!(
        "{},{},{},{}",
        latitude - 0.5,
        longitude - 0.5,
        latitude + 0.5,
        longitude + 0.5
    );

    let response_text = reqwest::Client::new()
        .get(CAMS_WMS_URL)
        .query(&[
            ("token", "public"),
            ("SERVICE", "WMS"),
            ("VERSION", "1.3.0"),
            ("REQUEST", "GetFeatureInfo"),
            ("CRS", "EPSG:4326"),
            ("BBOX", bbox.as_str()),
            ("WIDTH", "101"),
            ("HEIGHT", "101"),
            ("LAYERS", layer_name),
            ("QUERY_LAYERS", layer_name),
            ("INFO_FORMAT", "text/plain"),
            ("TIME", time_utc),
            ("FORMAT", "image/png"),
            ("I", "50"),
            ("J", "50"),
        ])
        .send()
        .await
        .with_context(|| format!("Failed to request CAMS WMS feature info for {}", layer_name))?
        .error_for_status()
        .with_context(|| format!("Non-success response from CAMS WMS for {}", layer_name))?
        .text()
        .await
        .with_context(|| format!("Failed to decode CAMS WMS response for {}", layer_name))?;

    parse_cams_feature_info(&response_text)
}

fn parse_cams_feature_info(text: &str) -> Result<CamsFeatureInfo> {
    if text.contains("ServiceException") {
        bail!("CAMS WMS returned a service exception: {}", text.trim());
    }

    let mut layer_name = None;
    let mut title = None;
    let mut value = None;
    let mut input_latitude = None;
    let mut input_longitude = None;
    let mut distance_km = None;
    let mut grid_point_latitude = None;
    let mut grid_point_longitude = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();

        if let Some(rest) = line.strip_prefix("Name:") {
            layer_name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Title:") {
            title = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Value:") {
            let numeric_token = rest.split_whitespace().next().unwrap_or_default();
            value = Some(
                numeric_token
                    .parse::<f64>()
                    .with_context(|| format!("Invalid CAMS value '{}'", numeric_token))?,
            );
        } else if let Some(rest) = line.strip_prefix("Input latitude:") {
            input_latitude = Some(rest.trim().parse::<f64>()?);
        } else if let Some(rest) = line.strip_prefix("Input longitude:") {
            input_longitude = Some(rest.trim().parse::<f64>()?);
        } else if let Some(rest) = line.strip_prefix("Distance:") {
            let numeric_token = rest.split_whitespace().next().unwrap_or_default();
            distance_km = Some(numeric_token.parse::<f64>()?);
        } else if let Some(rest) = line.strip_prefix("Grid point latitude:") {
            grid_point_latitude = Some(rest.trim().parse::<f64>()?);
        } else if let Some(rest) = line.strip_prefix("Grid point longitude:") {
            grid_point_longitude = Some(rest.trim().parse::<f64>()?);
        }
    }

    Ok(CamsFeatureInfo {
        layer_name: layer_name.ok_or_else(|| anyhow!("Missing CAMS layer name"))?,
        title: title.ok_or_else(|| anyhow!("Missing CAMS title"))?,
        value: value.ok_or_else(|| anyhow!("Missing CAMS value"))?,
        input_latitude: input_latitude.ok_or_else(|| anyhow!("Missing CAMS input latitude"))?,
        input_longitude: input_longitude.ok_or_else(|| anyhow!("Missing CAMS input longitude"))?,
        distance_km: distance_km.ok_or_else(|| anyhow!("Missing CAMS distance"))?,
        grid_point_latitude: grid_point_latitude
            .ok_or_else(|| anyhow!("Missing CAMS grid latitude"))?,
        grid_point_longitude: grid_point_longitude
            .ok_or_else(|| anyhow!("Missing CAMS grid longitude"))?,
    })
}

fn compact_date_to_cams_day_time(compact_date: &str) -> Result<String> {
    let day_portion = match compact_date.len() {
        8 => compact_date,
        12 => &compact_date[..8],
        _ => bail!("Invalid compact date '{}'", compact_date),
    };

    if !day_portion.chars().all(|ch| ch.is_ascii_digit()) {
        bail!("Invalid compact date '{}'", compact_date);
    }

    Ok(format!(
        "{}-{}-{}T00:00:00Z",
        &day_portion[0..4],
        &day_portion[4..6],
        &day_portion[6..8]
    ))
}

async fn build_forecast_section(
    point: &ForecastPoint,
    selected_assets: &HashMap<&'static str, SelectedForecastAsset>,
    parameters: &[ForecastBundleParameter],
    timestamp_field_name: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    let mut timeline: BTreeMap<String, Map<String, Value>> = BTreeMap::new();

    for parameter in parameters {
        let asset = selected_assets
            .get(parameter.code)
            .ok_or_else(|| anyhow!("Missing latest asset for {}", parameter.code))?;
        let csv_text = fetch_text(&asset.href).await?;
        let series = extract_forecast_series(
            &csv_text,
            &point.point_id,
            parameter.code,
            &asset.issue_timestamp,
        )?;

        for (forecast_time, value) in series {
            let entry = timeline.entry(forecast_time.clone()).or_insert_with(|| {
                let mut map = Map::new();
                map.insert(timestamp_field_name.to_string(), json!(forecast_time));
                map
            });
            entry.insert(parameter.field_name.to_string(), value);
        }
    }

    Ok(timeline
        .into_values()
        .take(limit.max(1))
        .map(Value::Object)
        .collect::<Vec<_>>())
}

fn build_parameter_metadata(parameter: &ForecastBundleParameter) -> Value {
    json!({
        "parameter_code": parameter.code,
        "field_name": parameter.field_name,
        "label": parameter.label,
        "unit": parameter.unit,
    })
}

async fn load_local_forecast_points() -> Result<Vec<ForecastPoint>> {
    let csv_text = fetch_text(LOCAL_FORECAST_POINT_METADATA_URL).await?;
    parse_local_forecast_points(&csv_text)
}

fn parse_local_forecast_points(csv_text: &str) -> Result<Vec<ForecastPoint>> {
    let rows = mcp_engine::parse_csv_response(csv_text)?;
    rows.into_iter()
        .map(|row| {
            Ok(ForecastPoint {
                point_id: required_csv_field(&row, "point_id")?,
                station_abbr: optional_non_empty(&row, "station_abbr"),
                postal_code: optional_non_empty(&row, "postal_code"),
                point_name: required_csv_field(&row, "point_name")?,
                point_type_en: required_csv_field(&row, "point_type_en")?,
                point_height_masl: optional_non_empty(&row, "point_height_masl")
                    .and_then(|value| value.parse::<f64>().ok()),
                point_coordinates_lv95_east: optional_non_empty(
                    &row,
                    "point_coordinates_lv95_east",
                )
                .and_then(|value| value.parse::<f64>().ok()),
                point_coordinates_lv95_north: optional_non_empty(
                    &row,
                    "point_coordinates_lv95_north",
                )
                .and_then(|value| value.parse::<f64>().ok()),
                point_coordinates_wgs84_lat: optional_non_empty(
                    &row,
                    "point_coordinates_wgs84_lat",
                )
                .and_then(|value| value.parse::<f64>().ok()),
                point_coordinates_wgs84_lon: optional_non_empty(
                    &row,
                    "point_coordinates_wgs84_lon",
                )
                .and_then(|value| value.parse::<f64>().ok()),
            })
        })
        .collect()
}

fn resolve_forecast_point(points: &[ForecastPoint], query: &str) -> Result<ForecastPoint> {
    let normalized_query = query.trim().to_ascii_lowercase();
    if normalized_query.is_empty() {
        bail!("Point query cannot be empty");
    }

    if let Some(point) = select_single_point(points, |point| {
        point.point_id.to_ascii_lowercase() == normalized_query
    })? {
        return Ok(point);
    }
    if let Some(point) = select_single_point(points, |point| {
        point
            .station_abbr
            .as_ref()
            .map(|value| value.to_ascii_lowercase() == normalized_query)
            .unwrap_or(false)
    })? {
        return Ok(point);
    }
    if let Some(point) = select_single_point(points, |point| {
        point
            .postal_code
            .as_ref()
            .map(|value| value.to_ascii_lowercase() == normalized_query)
            .unwrap_or(false)
    })? {
        return Ok(point);
    }
    if let Some(point) = select_single_point(points, |point| {
        point.point_name.to_ascii_lowercase() == normalized_query
    })? {
        return Ok(point);
    }

    let partial_matches = points
        .iter()
        .filter(|point| forecast_point_contains(point, &normalized_query))
        .cloned()
        .collect::<Vec<_>>();

    match partial_matches.as_slice() {
        [] => bail!("No local forecast point matched '{}'", query),
        [point] => Ok(point.clone()),
        matches => bail!(
            "Point query '{}' is ambiguous. Matches: {}",
            query,
            format_point_candidates(matches).join(", ")
        ),
    }
}

fn select_single_point<F>(points: &[ForecastPoint], predicate: F) -> Result<Option<ForecastPoint>>
where
    F: Fn(&ForecastPoint) -> bool,
{
    let matches = points
        .iter()
        .filter(|point| predicate(point))
        .cloned()
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => Ok(None),
        [point] => Ok(Some(point.clone())),
        many => bail!(
            "Point query is ambiguous. Matches: {}",
            format_point_candidates(many).join(", ")
        ),
    }
}

fn forecast_point_contains(point: &ForecastPoint, query: &str) -> bool {
    point.point_name.to_ascii_lowercase().contains(query)
        || point
            .station_abbr
            .as_ref()
            .map(|value| value.to_ascii_lowercase().contains(query))
            .unwrap_or(false)
        || point
            .postal_code
            .as_ref()
            .map(|value| value.to_ascii_lowercase().contains(query))
            .unwrap_or(false)
}

fn format_point_candidates(points: &[ForecastPoint]) -> Vec<String> {
    points
        .iter()
        .take(10)
        .map(|point| match &point.station_abbr {
            Some(station_abbr) => format!("{} ({})", point.point_name, station_abbr),
            None => point.point_name.clone(),
        })
        .collect()
}

async fn latest_local_forecast_assets() -> Result<HashMap<&'static str, SelectedForecastAsset>> {
    let url = format!(
        "{}/collections/{}/items?limit=8",
        STAC_ROOT, LOCAL_FORECAST_COLLECTION_ID
    );
    let collection: mcp_engine::StacItemCollection = fetch_json(&url).await?;
    let mut selected = HashMap::new();

    for item in collection.features {
        for (asset_name, asset) in item.assets {
            let Some((issue_timestamp, parameter_code)) =
                parse_local_forecast_asset_name(&asset_name)
            else {
                continue;
            };

            let Some(parameter) = HOURLY_FORECAST_BUNDLE
                .iter()
                .chain(DAILY_FORECAST_BUNDLE.iter())
                .find(|parameter| parameter.code == parameter_code)
            else {
                continue;
            };

            let should_replace = selected
                .get(parameter.code)
                .map(|current: &SelectedForecastAsset| {
                    issue_timestamp > current.issue_timestamp.as_str()
                })
                .unwrap_or(true);

            if should_replace {
                selected.insert(
                    parameter.code,
                    SelectedForecastAsset {
                        issue_timestamp: issue_timestamp.to_string(),
                        href: asset.href,
                    },
                );
            }
        }
    }

    for parameter in HOURLY_FORECAST_BUNDLE
        .iter()
        .chain(DAILY_FORECAST_BUNDLE.iter())
    {
        if !selected.contains_key(parameter.code) {
            bail!("No local forecast asset found for {}", parameter.code);
        }
    }

    Ok(selected)
}

fn parse_local_forecast_asset_name(asset_name: &str) -> Option<(&str, &str)> {
    let mut parts = asset_name.split('.');
    let _prefix = parts.next()?;
    let _source = parts.next()?;
    let issue_timestamp = parts.next()?;
    let parameter_code = parts.next()?;
    let extension = parts.next()?;

    if extension == "csv" {
        Some((issue_timestamp, parameter_code))
    } else {
        None
    }
}

fn extract_forecast_series(
    csv_text: &str,
    point_id: &str,
    parameter_code: &str,
    issue_timestamp: &str,
) -> Result<Vec<(String, Value)>> {
    let rows = mcp_engine::parse_csv_response(csv_text)?;
    let mut series = rows
        .into_iter()
        .filter(|row| {
            row.get("point_id")
                .map(|value| value == point_id)
                .unwrap_or(false)
        })
        .filter_map(|row| {
            let forecast_time = row.get("Date")?.to_string();
            if forecast_time.as_str() < issue_timestamp {
                return None;
            }

            let value = row
                .get(parameter_code)
                .and_then(|raw| parse_forecast_cell(raw.as_str()))?;
            Some((forecast_time, value))
        })
        .collect::<Vec<_>>();

    series.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(series)
}

fn parse_forecast_cell(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return None;
    }

    if let Ok(integer) = trimmed.parse::<i64>() {
        return Some(json!(integer));
    }
    if let Ok(float) = trimmed.parse::<f64>() {
        return Some(json!(float));
    }

    Some(json!(trimmed))
}

fn build_forecast_point_json(point: &ForecastPoint) -> Value {
    json!({
        "point_id": point.point_id,
        "point_name": point.point_name,
        "point_type": point.point_type_en,
        "station_abbr": point.station_abbr,
        "postal_code": point.postal_code,
        "altitude_masl": point.point_height_masl,
        "coordinates_wgs84": {
            "lat": point.point_coordinates_wgs84_lat,
            "lon": point.point_coordinates_wgs84_lon,
        },
        "coordinates_lv95": {
            "east": point.point_coordinates_lv95_east,
            "north": point.point_coordinates_lv95_north,
        }
    })
}

fn required_csv_field(row: &HashMap<String, String>, key: &str) -> Result<String> {
    row.get(key)
        .cloned()
        .ok_or_else(|| anyhow!("Missing CSV field '{}'", key))
}

fn optional_non_empty(row: &HashMap<String, String>, key: &str) -> Option<String> {
    row.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        let request: mcp_engine::McpRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("Failed to parse request: {}", error);
                continue;
            }
        };

        if let Some(response) = handle_request(request).await {
            println!("{}", serde_json::to_string(&response)?);
        }
    }

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_engine::*;

    #[test]
    fn test_parse_csv_response() {
        let csv = "a;b;c\n1;2;3\n4;5;6\n";
        let records = parse_csv_response(csv).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["a"], "1");
        assert_eq!(records[1]["c"], "6");
    }

    #[test]
    fn test_mcp_response_ok() {
        let resp = McpResponse::ok(serde_json::json!(0), serde_json::json!({"test": true}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""jsonrpc":"2.0""#));
        assert!(json.contains(r#""id":0"#));
        assert!(json.contains(r#""test":true"#));
        assert!(json.contains(r#""result":{"test":true}"#));
        assert!(!json.contains(r#""error""#));
    }

    #[test]
    fn test_mcp_response_err() {
        let resp = McpResponse::err(serde_json::json!(0), -32601, "Method not found");
        let back: McpResponse =
            serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
        assert_eq!(back.jsonrpc, "2.0");
        assert_eq!(back.id, Some(serde_json::json!(0)));
    }

    #[test]
    fn test_tools_list_response() {
        let resp = tools_list_response(serde_json::json!(1));
        let tools = resp
            .result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap();
        assert_eq!(tools.len(), 9);
        assert_eq!(tools[0]["name"], "list_weather_stations");
        assert_eq!(tools[4]["name"], "get_sunshine");
        assert_eq!(tools[6]["name"], "list_pollen_stations");
        assert_eq!(tools[8]["name"], "get_local_forecast");
        assert_eq!(tools[7]["inputSchema"]["required"][0], "species");
    }

    #[test]
    fn test_parse_initialize_request() {
        let json = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.0.1"}}}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, Some(serde_json::json!(1)));
    }

    #[test]
    fn test_parse_tools_list_request() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/list","id":2}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tools/list");
        assert!(req.id.is_some());
    }

    #[test]
    fn test_parse_tools_call_request() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"get_local_forecast","arguments":{"point_query":"ARO","hours":12}}}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.params["name"], "get_local_forecast");
        assert_eq!(req.params["arguments"]["point_query"], "ARO");
    }

    #[test]
    fn test_parse_tools_call_without_arguments() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"list_weather_stations"}}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.params["name"], "list_weather_stations");
        assert!(req.params.get("arguments").is_none());
    }

    #[tokio::test]
    async fn test_tools_call_missing_name_returns_error() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/call","id":3}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        let resp = handle_request(req).await.unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("Missing argument 'name'"));
    }

    #[tokio::test]
    async fn test_tools_call_unknown_tool_returns_error() {
        let json =
            r#"{"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"nonexistent"}}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        let resp = handle_request(req).await.unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, -32601);
    }

    #[tokio::test]
    async fn test_get_pollen_measurement_missing_species_returns_error() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"get_pollen_measurement","arguments":{}}}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        let resp = handle_request(req).await.unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("Missing argument 'species'"));
    }

    #[tokio::test]
    async fn test_get_local_forecast_missing_point_query_returns_error() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"get_local_forecast","arguments":{}}}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        let resp = handle_request(req).await.unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("Missing argument 'point_query'"));
    }

    #[tokio::test]
    async fn test_unsupported_method_returns_error() {
        let json = r#"{"jsonrpc":"2.0","method":"unknown","id":3}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        let resp = handle_request(req).await.unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, -32601);
    }

    #[tokio::test]
    async fn test_notification_initialized_returns_none() {
        let json = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req: McpRequest = serde_json::from_str(json).unwrap();
        let resp = handle_request(req).await;
        assert!(resp.is_none());
    }

    #[test]
    fn test_mcp_error_serialization() {
        let err = McpError {
            code: -32601,
            message: "Method not found".to_string(),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains(r#""code":-32601"#));
        assert!(json.contains(r#""message":"Method not found""#));
    }

    #[test]
    fn test_station_info_serialization() {
        let info = StationInfo {
            id: "PBE".to_string(),
            station_name: "Bern".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(r#""id":"PBE""#));
        assert!(json.contains(r#""station_name":"Bern""#));
    }

    #[test]
    fn test_parse_local_forecast_asset_name() {
        let parsed = parse_local_forecast_asset_name("vnut12.lssw.202604170000.fu3010h0.csv");
        assert_eq!(parsed, Some(("202604170000", "fu3010h0")));
    }

    #[test]
    fn test_parse_measurement_value_treats_missing_sentinel_as_none() {
        let value = json!(99999);
        assert_eq!(parse_measurement_value(&value, Some(99_999.0)), None);
    }

    #[test]
    fn test_extract_forecast_series_filters_issue_timestamp_and_parses_values() {
        let csv = "point_id;point_type_id;Date;fu3010h0\n1;1;202604162200;5.4\n1;1;202604170000;2.3\n1;1;202604170100;3.3\n";
        let series = extract_forecast_series(csv, "1", "fu3010h0", "202604170000").unwrap();
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].0, "202604170000");
        assert_eq!(series[0].1, json!(2.3));
    }

    #[test]
    fn test_build_forecast_summary_includes_cloud_cover_and_availability() {
        let hourly_breakdown = vec![
            json!({
                "temperature_2m_c": 10.0,
                "precipitation_hourly_mm": 0.0,
                "precipitation_probability_3h_percent": 10,
                "wind_speed_kmh": 5.0,
                "wind_gust_kmh": 11.0,
                "sunshine_duration_minutes": 12,
                "low_cloud_cover": 0.2,
                "medium_cloud_cover": 0.4,
                "high_cloud_cover": 0.6
            }),
            json!({
                "temperature_2m_c": 14.0,
                "precipitation_hourly_mm": 1.2,
                "precipitation_probability_3h_percent": 40,
                "wind_speed_kmh": 9.0,
                "wind_gust_kmh": 18.0,
                "sunshine_duration_minutes": 3,
                "low_cloud_cover": 0.5,
                "medium_cloud_cover": 0.1,
                "high_cloud_cover": 0.3
            }),
        ];

        let uv_index = json!({
            "available": true,
            "daily_max_summary": {
                "min": 3.5,
                "max": 5.2,
                "mean": 4.4
            }
        });
        let air_quality = unavailable_metric(AIR_QUALITY_AQI_UNAVAILABLE);
        let summary = build_forecast_summary(&hourly_breakdown, uv_index, air_quality);
        assert_eq!(summary["temperature_2m_c"]["min"], json!(10.0));
        assert_eq!(summary["temperature_2m_c"]["max"], json!(14.0));
        assert_eq!(summary["precipitation_hourly_mm"]["total"], json!(1.2));
        assert_eq!(summary["sunshine_duration_minutes"]["total"], json!(15.0));
        assert_eq!(summary["cloud_cover_fraction"]["low"]["max"], json!(0.5));
        assert_eq!(
            summary["cloud_cover_fraction"]["medium"]["mean"],
            json!(0.3)
        );
        assert_eq!(summary["uv_index"]["available"], json!(true));
        assert_eq!(summary["uv_index"]["daily_max_summary"]["max"], json!(5.2));
        assert_eq!(
            summary["air_quality_aqi"]["reason"],
            json!(AIR_QUALITY_AQI_UNAVAILABLE)
        );
    }

    #[test]
    fn test_parse_cams_feature_info_parses_numeric_fields() {
        let feature_info = parse_cams_feature_info(
            r#"
Name: composition_uvindex_daily_max
Title: Total sky UV index (provided by CAMS)
Value: 4.10311 default
Input latitude:  47.254950495049506
Input longitude: 8.225049504950496
Grid point: nearest
Distance: 14.553 km
Grid point latitude: 47.2
Grid point longitude:  8.4
"#,
        )
        .unwrap();

        assert_eq!(feature_info.layer_name, "composition_uvindex_daily_max");
        assert_eq!(feature_info.title, "Total sky UV index (provided by CAMS)");
        assert_eq!(feature_info.value, 4.10311);
        assert_eq!(feature_info.distance_km, 14.553);
        assert_eq!(feature_info.grid_point_latitude, 47.2);
        assert_eq!(feature_info.grid_point_longitude, 8.4);
    }

    #[test]
    fn test_compact_date_to_cams_day_time_formats_iso_date() {
        let formatted = compact_date_to_cams_day_time("20260420").unwrap();
        assert_eq!(formatted, "2026-04-20T00:00:00Z");
    }

    #[test]
    fn test_compact_date_to_cams_day_time_accepts_meteoswiss_daily_timestamp() {
        let formatted = compact_date_to_cams_day_time("202604200000").unwrap();
        assert_eq!(formatted, "2026-04-20T00:00:00Z");
    }

    #[test]
    fn test_resolve_forecast_point_by_station_abbr() {
        let points = vec![
            ForecastPoint {
                point_id: "1".to_string(),
                station_abbr: Some("ARO".to_string()),
                postal_code: None,
                point_name: "Arosa".to_string(),
                point_type_en: "Station".to_string(),
                point_height_masl: Some(1878.0),
                point_coordinates_lv95_east: None,
                point_coordinates_lv95_north: None,
                point_coordinates_wgs84_lat: None,
                point_coordinates_wgs84_lon: None,
            },
            ForecastPoint {
                point_id: "2".to_string(),
                station_abbr: Some("RAG".to_string()),
                postal_code: None,
                point_name: "Bad Ragaz".to_string(),
                point_type_en: "Station".to_string(),
                point_height_masl: Some(497.0),
                point_coordinates_lv95_east: None,
                point_coordinates_lv95_north: None,
                point_coordinates_wgs84_lat: None,
                point_coordinates_wgs84_lon: None,
            },
        ];

        let point = resolve_forecast_point(&points, "rag").unwrap();
        assert_eq!(point.point_name, "Bad Ragaz");
    }

    #[test]
    fn test_resolve_forecast_point_reports_ambiguous_partial_match() {
        let points = vec![
            ForecastPoint {
                point_id: "1".to_string(),
                station_abbr: Some("ARO".to_string()),
                postal_code: None,
                point_name: "Arosa".to_string(),
                point_type_en: "Station".to_string(),
                point_height_masl: None,
                point_coordinates_lv95_east: None,
                point_coordinates_lv95_north: None,
                point_coordinates_wgs84_lat: None,
                point_coordinates_wgs84_lon: None,
            },
            ForecastPoint {
                point_id: "2".to_string(),
                station_abbr: None,
                postal_code: Some("7050".to_string()),
                point_name: "Arosa Dorf".to_string(),
                point_type_en: "Postal code".to_string(),
                point_height_masl: None,
                point_coordinates_lv95_east: None,
                point_coordinates_lv95_north: None,
                point_coordinates_wgs84_lat: None,
                point_coordinates_wgs84_lon: None,
            },
        ];

        let error = resolve_forecast_point(&points, "aros").unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
    }
}
