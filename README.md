# swiss-weather-mcp

`swiss-weather-mcp` is a JSON-RPC 2.0 MCP server over stdio that exposes Swiss weather, pollen, radiation, UV, and air-quality data.

The server reads one JSON-RPC request per line from `stdin` and writes one JSON-RPC response per line to `stdout`.

Repository and homepage:

- <https://github.com/pi-victor/swiss-weather-mcp>

License:

- Apache-2.0

## What It Provides

This server currently exposes these tools:

- `list_weather_stations`
- `get_precipitation`
- `get_wind_speed_10min`
- `get_wind_gusts_10min`
- `get_sunshine`
- `get_global_radiation`
- `list_pollen_stations`
- `get_pollen_measurement`
- `get_local_forecast`

## Data Sources

The implementation currently pulls data from these upstream sources:

- MeteoSwiss / geo.admin current measurement layers for precipitation, wind, sunshine, and global radiation
- MeteoSwiss OGD local forecasting dataset for point forecast data
- MeteoSwiss OGD pollen dataset for pollen station discovery
- Copernicus CAMS via ECMWF public WMS for UV daily maximum values
- FOEN NABEL current table for current pollutant measurements

## Current Limits

- Transport is stdio only. There is no HTTP server.
- `air_quality_aqi` is not implemented as a verified AQI feed. The server returns current pollutant measurements separately and marks AQI unavailable.
- `air_quality_current` is mapped to the nearest official NABEL station using static station metadata embedded in the code. The pollutant values themselves are live.
- A live `cargo run` end-to-end check for `get_local_forecast` may depend on external upstream latency.

## Build And Run

Requirements:

- Rust stable

Install from crates.io:

```bash
cargo install swiss-weather-mcp
```

Common commands:

```bash
cargo build
cargo run
cargo test
cargo clippy --all-targets --all-features
cargo fmt
```

To run the server manually:

```bash
cargo run
```

Then write JSON-RPC requests, one per line, to `stdin`.

## MCP Protocol

The server supports:

- `initialize`
- `notifications/initialized`
- `tools/list`
- `tools/call`

Protocol version:

- `2025-03-26`

### Example Initialize

Request:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
```

Response shape:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2025-03-26",
    "capabilities": {
      "tools": {
        "listChanged": false
      }
    },
    "serverInfo": {
      "name": "swiss-weather-mcp",
      "version": "0.1.0"
    }
  }
}
```

### Example Tools Call

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "get_local_forecast",
    "arguments": {
      "point_query": "Bettwil",
      "hours": 12
    }
  }
}
```

Tool results are returned as MCP `content` blocks containing pretty-printed JSON text:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\n  ... tool JSON ...\n}"
      }
    ]
  }
}
```

## Tool Reference

### `list_weather_stations`

Lists MeteoSwiss current-weather stations based on the latest 10-minute precipitation layer.

Arguments:

- none

Returns:

- `source_url`: upstream JSON file URL
- `updated_at`: source timestamp
- `station_count`
- `stations`: array of:
  - `id`
  - `station_name`
  - `coordinates_lv95.east`
  - `coordinates_lv95.north`

### `get_precipitation`

Returns precipitation measurements from official MeteoSwiss current layers.

Arguments:

- `interval`: one of `10min`, `1h`, `24h`, `48h`
- `station_id`: optional station filter

Returns:

- `source_url`
- `layer`
- `title`
- `updated_at`
- `record_count`
- `records`

### `get_wind_speed_10min`

Returns the MeteoSwiss 10-minute mean wind speed layer.

Arguments:

- `station_id`: optional station filter

Returns the same top-level shape as `get_precipitation`.

### `get_wind_gusts_10min`

Returns the MeteoSwiss 10-minute maximum 1-second wind gust layer.

Arguments:

- `station_id`: optional station filter

Returns the same top-level shape as `get_precipitation`.

### `get_sunshine`

Returns sunshine duration measurements.

Arguments:

- `interval`: one of `10min`, `1d`
- `station_id`: optional station filter

Returns the same top-level shape as `get_precipitation`.

### `get_global_radiation`

Returns global radiation measurements.

Arguments:

- `interval`: one of `10min`, `1d`
- `station_id`: optional station filter

Returns the same top-level shape as `get_precipitation`.

### `list_pollen_stations`

Lists MeteoSwiss pollen stations from the pollen OGD dataset.

Arguments:

- none

Returns:

- `source_url`
- `station_count`
- `stations`: array of:
  - `id`
  - `station_name`

### `get_pollen_measurement`

Returns the latest hourly measurement layer for a specific pollen species.

Arguments:

- `species`: one of `alder`, `ash`, `beech`, `birch`, `grass`, `hazel`, `oak`
- `station_id`: optional station filter

Returns the same top-level shape as `get_precipitation`.

Notes:

- The pollen layers use a missing-value sentinel (`99999`) which is converted to `null`.

### `get_local_forecast`

Returns the latest local forecast for a point, with daily summary rows, hourly rows, computed summary blocks, UV, and current air quality.

Arguments:

- `point_query`: point ID, station abbreviation, postal code, or point name
- `hours`: optional integer from `1` to `96`, default `24`

Top-level return shape:

- `source_collection`
- `issued_at`
- `point`
- `daily_parameters`
- `hourly_parameters`
- `daily_summary_count`
- `hourly_breakdown_count`
- `daily_summary`
- `hourly_breakdown`
- `summary`
- `unsupported`

## Shared Response Shapes

### Measurement Layer Responses

These tools return the same general shape:

- `get_precipitation`
- `get_wind_speed_10min`
- `get_wind_gusts_10min`
- `get_sunshine`
- `get_global_radiation`
- `get_pollen_measurement`

Top-level fields:

- `source_url`
- `layer`
- `title`
- `updated_at`
- `record_count`
- `records`

Each `records` item contains:

- `station_id`
- `station_name`
- `value`
- `unit`
- `reference_timestamp`
- `altitude`
- `measurement_height`
- `coordinates_lv95`
- `wind_direction_degrees`
- `wind_direction_radian`
- optional `value_relative`

Notes:

- Some fields may be `null` depending on the source layer.
- `station_id` matching accepts exact station ID, exact station name, or substring match on station name.

### Forecast Point Resolution

`get_local_forecast` resolves `point_query` in this order:

1. exact `point_id`
2. exact `station_abbr`
3. exact `postal_code`
4. exact `point_name`
5. partial match against `point_name`, `station_abbr`, or `postal_code`

Ambiguous partial matches return an error.

## `get_local_forecast` Response Contract

### `point`

The selected point object contains:

- `point_id`
- `point_name`
- `point_type`
- `station_abbr`
- `postal_code`
- `altitude_masl`
- `coordinates_wgs84.lat`
- `coordinates_wgs84.lon`
- `coordinates_lv95.east`
- `coordinates_lv95.north`

### `hourly_parameters`

Each item contains:

- `parameter_code`
- `field_name`
- `label`
- `unit`

The current hourly parameter set is:

- `temperature_2m_c`
- `precipitation_hourly_mm`
- `precipitation_probability_3h_percent`
- `wind_direction_degrees`
- `wind_speed_kmh`
- `wind_gust_kmh`
- `weather_icon_code`
- `low_cloud_cover`
- `medium_cloud_cover`
- `high_cloud_cover`
- `sunshine_duration_minutes`
- `global_radiation_w_m2`
- `diffuse_radiation_w_m2`

### `daily_parameters`

Each item contains the same metadata fields as `hourly_parameters`.

The current daily parameter set is:

- `temperature_min_c`
- `temperature_max_c`
- `precipitation_total_mm`
- `precipitation_total_10_percentile_mm`
- `precipitation_total_90_percentile_mm`
- `weather_icon_code`

### `hourly_breakdown`

An array of timeline rows. Each row contains:

- `forecast_time`
- zero or more hourly fields from the parameter set above

The server only includes values that were present in the source series for that timestamp.

### `daily_summary`

An array of daily rows. Each row contains:

- `forecast_date`
- zero or more daily fields from the daily parameter set above

### `summary`

`summary` is a derived rollup built from `hourly_breakdown` plus external UV and air-quality lookups.

Fields:

- `temperature_2m_c`
- `precipitation_hourly_mm`
- `precipitation_probability_3h_percent`
- `wind_speed_kmh`
- `wind_gust_kmh`
- `sunshine_duration_minutes`
- `global_radiation_w_m2`
- `diffuse_radiation_w_m2`
- `cloud_cover_fraction`
- `uv_index`
- `air_quality_current`
- `air_quality_aqi`

#### Numeric Summary Objects

These fields use:

```json
{
  "min": 0.0,
  "max": 0.0,
  "mean": 0.0
}
```

Used for:

- `temperature_2m_c`
- `precipitation_probability_3h_percent`
- `wind_speed_kmh`
- `wind_gust_kmh`
- `global_radiation_w_m2`
- `diffuse_radiation_w_m2`
- `cloud_cover_fraction.low`
- `cloud_cover_fraction.medium`
- `cloud_cover_fraction.high`

#### Accumulation Summary Objects

These fields use:

```json
{
  "total": 0.0,
  "max": 0.0
}
```

Used for:

- `precipitation_hourly_mm`
- `sunshine_duration_minutes`

#### `uv_index`

Current behavior:

- Source: Copernicus CAMS via ECMWF public WMS
- Uses the first up to `3` daily forecast dates

Available shape:

```json
{
  "available": true,
  "source": {
    "provider": "Copernicus CAMS",
    "service": "ECMWF public WMS",
    "layer": "composition_uvindex_daily_max",
    "url": "https://eccharts.ecmwf.int/wms/"
  },
  "daily_max_summary": {
    "min": 0.0,
    "max": 0.0,
    "mean": 0.0
  },
  "daily_max_forecast": [
    {
      "forecast_date": "20260420",
      "time_utc": "2026-04-20T00:00:00Z",
      "value": 4.1,
      "nearest_grid_point": {
        "latitude": 47.2,
        "longitude": 8.4,
        "distance_km": 14.6
      }
    }
  ]
}
```

Unavailable shape:

```json
{
  "available": false,
  "reason": "Copernicus CAMS UV lookup is unavailable for this point or time window. ..."
}
```

#### `air_quality_current`

Current behavior:

- Source: FOEN NABEL current table
- The server chooses the nearest official NABEL station using static station metadata and the forecast point LV95 coordinates

Available shape:

```json
{
  "available": true,
  "reported_at": "Data from: 19.04.2026 23:00",
  "source": {
    "provider": "FOEN NABEL",
    "service": "Current situation table",
    "url": "https://bafu.meteotest.ch/nabel/tables/show/english"
  },
  "nearest_station": {
    "id": "BER",
    "station_name": "Bern-Bollwerk",
    "site_type": "Urban, traffic",
    "distance_km": 42.7,
    "coordinates_lv95": {
      "east": 2600170.0,
      "north": 1199990.0
    }
  },
  "measurements_ug_m3": {
    "o3": 68.0,
    "o3_daily_max": 88.0,
    "no2": 10.0,
    "pm10": 13.0,
    "pm2_5": 6.0
  }
}
```

Unavailable shape:

```json
{
  "available": false,
  "reason": "Official FOEN NABEL current pollutant measurements are unavailable for this point or time window. ..."
}
```

#### `air_quality_aqi`

Current behavior:

- There is no verified AQI feed integrated
- The field is currently returned as unavailable

Shape:

```json
{
  "available": false,
  "reason": "No verified official AQI feed has been integrated. Current official NABEL pollutant measurements are available separately in summary.air_quality_current."
}
```

### `unsupported`

`unsupported` is an object containing only the unavailable derived sub-features.

Possible keys:

- `uv_index`
- `air_quality_current`
- `air_quality_aqi`

Each value is the corresponding reason string.

If all derived features are available, `unsupported` is an empty object.

## Error Behavior

Common failure cases:

- missing required tool argument
- unsupported interval
- unsupported pollen species
- unknown tool name
- unsupported method
- no matching station or point
- ambiguous point query
- upstream request failure
- upstream decode failure

For tool calls, application errors are returned inside the tool content as:

```text
Error: ...
```

JSON-RPC argument and method errors are returned as MCP `error` objects, for example:

- `-32601` for unknown method or unknown tool
- `-32602` for missing required arguments

## Development Notes

- Formatting: `cargo fmt`
- Typecheck/build validation: `cargo check`
- Tests: `cargo test`
- Linting: `cargo clippy --all-targets --all-features`

The current implementation has unit tests for:

- MCP request/response serialization
- CSV parsing
- forecast asset parsing
- forecast summary derivation
- CAMS UV parsing
- compact CAMS date conversion
- NABEL table parsing
- station-name normalization
- ambiguous point resolution

## Example Session

```text
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_local_forecast","arguments":{"point_query":"Bettwil","hours":12}}}
```

## Repository Layout

- `src/main.rs`: request handling, tool implementations, forecast parsing, UV and air-quality integration, tests
- `src/mcp_engine.rs`: MCP protocol types, tool metadata, shared CSV and source types
- `Cargo.toml`: dependencies and crate metadata
- `README.md`: package readme and user-facing project documentation
