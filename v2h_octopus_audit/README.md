# v2h_octopus_audit

**Octopus Intelligent Go — smart-charge slot verifier**

Compares every half-hour cheap slot that Octopus Energy promises against
what Home Assistant's `off_peak` binary sensor actually recorded and
whether the Zappi charger drew any current.

## Quick start

```bash
# 1. Create your config (DO NOT commit config.toml — it holds your API key)
cp config.example.toml config.toml
# Edit config.toml and set your octopus.api_key

# 2. Fetch the HA database (mounts CIFS share, safe online backup)
./fetch_db.sh

# 3. Run the audit (re-run as many times as needed without re-fetching the DB)
./audit.sh
```

`fetch_db.sh` reuses the SMB credentials from `../v2h_homeassistant/.deploy_credentials`.
Both scripts create the `.venv` automatically on the first run.

### Manual steps (Windows or no SMB access)

```bat
copy \\192.168.10.21\config\home-assistant_v2.db ha_db.db
python3 -m venv .venv
.venv\Scripts\pip install -r requirements.txt
.venv\Scripts\python audit.py
```

Outputs land in `output/`:
- `slot_detail_YYYYMMDD_HHMM.csv`   — one row per 30-min slot, all 30 days
- `night_summary_YYYYMMDD_HHMM.csv` — one row per 23:30–05:30 night window

## CSV flags

| Flag | Meaning |
|------|---------|
| `MISSED_SLOT` | Octopus API says cheap — HA sensor did not fire |
| `SENSOR_ONLY` | HA sensor fired — no matching API cheap slot |
| `OK_CHARGING` | Cheap window, sensor on, Zappi drawing >1 kW |
| `OK_IDLE` | Cheap window, sensor on, no car / already full |
| `CHARGED_AT_PEAK` | Zappi drew power outside any cheap window |

## Data sources

| Source | What it provides |
|--------|-----------------|
| Octopus GraphQL `completedDispatches` | Historical cheap slots including any smart dispatches (longer history) |
| Octopus REST `/intelligent/dispatches/` | Fallback if GraphQL fails (~2 weeks) |
| Octopus REST `standard-unit-rates` | Rate in p/kWh per slot (reference) |
| HA SQLite `states` table | `off_peak` sensor history, Zappi watts, smartcharge_status |

## Config

Copy `config.example.toml` to `config.toml` (gitignored) and edit it:

```toml
[octopus]
api_key = "sk_live_YOUR_KEY_HERE"

[ha]
db_path    = "ha_db.db"
audit_days = 30
offpeak_entity     = "binary_sensor.octopus_energy_electricity_..."
zappi_entity       = "sensor.myenergi_zappi_..."
smartcharge_entity = "sensor.smartcharge_status"
```

The entity IDs can be found in HA → Developer Tools → States.

## Requirements

Python 3.11+ (no extra dependencies). Python 3.10 needs `tomli` — included in `requirements.txt`.
