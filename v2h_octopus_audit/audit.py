#!/usr/bin/env python3
"""
v2h_octopus_audit
=================
Octopus Intelligent Go — smart-charge slot auditor.

Pulls cheap-period data from two sources:
  1. Octopus Kraken GraphQL / REST API (dispatches + unit rates)
  2. Home Assistant SQLite history (off_peak binary sensor, Zappi CT, smartcharge_status)

Produces two CSVs you can open straight in Excel:
  output/slot_detail_YYYYMMDD_HHMM.csv   — one row per 30-min slot
  output/night_summary_YYYYMMDD_HHMM.csv — one row per 23:30–05:30 window

Usage
-----
1.  Copy config.example.toml → config.toml and fill in your Octopus API key.
2.  Copy HA's home-assistant_v2.db locally first (avoids SQLite lock issues):
      Linux:   cp /mnt/ha_config/home-assistant_v2.db ha_db.db
      Windows: copy \\192.168.10.21\\config\\home-assistant_v2.db ha_db.db
3.  pip install -r requirements.txt
4.  python audit.py

Requires Python 3.10+
"""

import csv
import os
import sqlite3
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from zoneinfo import ZoneInfo

import requests

try:
    import tomllib          # Python 3.11+
except ImportError:
    import tomli as tomllib  # pip install tomli  (Python 3.10)

# ══════════════════════════════════════════════════════════════════════════════
# CONFIG — loaded from config.toml (copy config.example.toml to get started)
# ══════════════════════════════════════════════════════════════════════════════

_CONFIG_PATH = Path(__file__).parent / "config.toml"
if not _CONFIG_PATH.exists():
    sys.exit(
        "\n✗  config.toml not found.\n"
        "   Copy config.example.toml → config.toml and fill in your Octopus API key.\n"
    )

with open(_CONFIG_PATH, "rb") as _f:
    _cfg = tomllib.load(_f)

OCTOPUS_API_KEY    = _cfg["octopus"]["api_key"]
_oct               = _cfg["octopus"]
CACHED_ACCOUNT     = _oct.get("account_number")
CACHED_MPAN        = _oct.get("mpan")
CACHED_SERIAL      = _oct.get("serial")
CACHED_TARIFF      = _oct.get("tariff_code")

HA_DB_PATH         = _cfg["ha"]["db_path"]
AUDIT_DAYS         = _cfg["ha"]["audit_days"]
FROM_DATE          = _cfg["ha"].get("from_date")   # optional "YYYY-MM-DD" override
OFFPEAK_ENTITY     = _cfg["ha"]["offpeak_entity"]
ZAPPI_ENTITY       = _cfg["ha"]["zappi_entity"]
SMARTCHARGE_ENTITY = _cfg["ha"]["smartcharge_entity"]
ZAPPI_ON_W         = _cfg["thresholds"]["zappi_on_w"]
ON_FRACTION_MIN    = _cfg["thresholds"]["on_fraction_min"]
CHEAP_WINDOW_START = (_cfg["cheap_window"]["start_hour"], _cfg["cheap_window"]["start_minute"])
CHEAP_WINDOW_END   = (_cfg["cheap_window"]["end_hour"],   _cfg["cheap_window"]["end_minute"])

OUTPUT_DIR = Path("output")

# ══════════════════════════════════════════════════════════════════════════════

TZ_UK  = ZoneInfo("Europe/London")
TZ_UTC = timezone.utc

OCTOPUS_REST = "https://api.octopus.energy/v1"
GQL_URL      = f"{OCTOPUS_REST}/graphql/"


# ── Octopus API helpers ───────────────────────────────────────────────────────

def rest_session() -> requests.Session:
    s = requests.Session()
    s.auth = (OCTOPUS_API_KEY, "")
    return s


def gql_token() -> str | None:
    """Exchange API key for a Kraken JWT (needed for dispatch GraphQL queries)."""
    mutation = """
    mutation ($key: String!) {
        obtainKrakenToken(input: {APIKey: $key}) { token }
    }
    """
    try:
        r = requests.post(GQL_URL, json={"query": mutation,
                                         "variables": {"key": OCTOPUS_API_KEY}})
        r.raise_for_status()
        return r.json()["data"]["obtainKrakenToken"]["token"]
    except Exception as exc:
        print(f"    ⚠  Could not get GraphQL token: {exc}")
        return None


def get_account_info(session: requests.Session, jwt: str | None) -> dict:
    """Return account_number, mpan, serial, tariff_code, product_code."""
    # Account number via GraphQL viewer query (requires JWT auth)
    accounts = []
    if jwt:
        try:
            r = requests.post(GQL_URL,
                              json={"query": "{ viewer { accounts { number } } }"},
                              headers={"Authorization": f"JWT {jwt}"})
            r.raise_for_status()
            resp = r.json()
            if resp.get("errors"):
                print(f"    ⚠  GraphQL viewer errors: {resp['errors']}")
            accounts = ((resp.get("data") or {}).get("viewer") or {}).get("accounts") or []
        except Exception as exc:
            print(f"    ⚠  GraphQL viewer query failed: {exc}")

    if not accounts:
        sys.exit("✗  No accounts found. Check your API key in config.toml.")
    account_number = accounts[0]["number"]
    print(f"    Account : {account_number}")

    # Meter point + tariff via REST
    r = session.get(f"{OCTOPUS_REST}/accounts/{account_number}/")
    r.raise_for_status()

    now_iso = datetime.now(TZ_UTC).isoformat()
    for prop in r.json().get("properties", []):
        for emp in prop.get("electricity_meter_points", []):
            for meter in emp.get("meters", []):
                # Newest active agreement
                for ag in sorted(emp.get("agreements", []),
                                 key=lambda a: a.get("valid_from", ""), reverse=True):
                    if ag.get("valid_to") is None or ag["valid_to"] > now_iso:
                        tc = ag.get("tariff_code", "")
                        parts = tc.split("-")
                        pc = "-".join(parts[2:-1]) if len(parts) >= 4 else ""
                        print(f"    MPAN    : {emp['mpan']}")
                        print(f"    Serial  : {meter['serial_number']}")
                        print(f"    Tariff  : {tc}")
                        return dict(
                            account_number = account_number,
                            mpan           = emp["mpan"],
                            serial         = meter["serial_number"],
                            tariff_code    = tc,
                            product_code   = pc,
                        )

    sys.exit("✗  Could not extract meter / tariff info from account.")


def get_unit_rates(session: requests.Session,
                   product_code: str, tariff_code: str,
                   from_dt: datetime, to_dt: datetime) -> list[dict]:
    """Fetch standard unit rates for the tariff period (may be half-hourly for Agile/Go)."""
    url = (f"{OCTOPUS_REST}/products/{product_code}"
           f"/electricity-tariffs/{tariff_code}/standard-unit-rates/")
    params = {
        "period_from": from_dt.isoformat(),
        "period_to":   to_dt.isoformat(),
        "page_size":   1500,
    }
    results, next_url = [], url
    while next_url:
        try:
            r = session.get(next_url, params=params)
            r.raise_for_status()
            data    = r.json()
            results.extend(data.get("results", []))
            next_url = data.get("next")
            params   = {}
        except Exception as exc:
            print(f"    ⚠  Unit rates error: {exc}")
            break
    print(f"    Unit rate records : {len(results)}")
    return results


def get_dispatches_gql(account_number: str, jwt: str | None) -> tuple[list, list]:
    """Pull completed + planned dispatches from Kraken GraphQL (longer history)."""
    if not jwt:
        return [], []
    query = """
    query ($acct: String!) {
        completedDispatches(accountNumber: $acct) {
            startDt endDt delta meta { source location }
        }
        plannedDispatches(accountNumber: $acct) {
            startDt endDt delta meta { source location }
        }
    }
    """
    try:
        r = requests.post(GQL_URL,
                          json={"query": query, "variables": {"acct": account_number}},
                          headers={"Authorization": f"JWT {jwt}"})
        r.raise_for_status()
        d = r.json().get("data", {})
        completed = d.get("completedDispatches", [])
        planned   = d.get("plannedDispatches",   [])
        print(f"    GQL dispatches — completed: {len(completed)}  planned: {len(planned)}")
        return completed, planned
    except Exception as exc:
        print(f"    ⚠  GraphQL dispatches failed: {exc}")
        return [], []


def get_dispatches_rest(session: requests.Session,
                        account_number: str) -> tuple[list, list]:
    """Pull dispatches from the REST endpoint (typically ~2 weeks of history)."""
    url = f"{OCTOPUS_REST}/intelligent/dispatches/{account_number}/"
    try:
        r = session.get(url)
        r.raise_for_status()
        d = r.json()
        completed = d.get("completed", [])
        planned   = d.get("planned",   [])
        print(f"    REST dispatches — completed: {len(completed)}  planned: {len(planned)}")
        return completed, planned
    except Exception as exc:
        print(f"    ⚠  REST dispatches failed: {exc}")
        return [], []


# ── Cheap-slot set builder ────────────────────────────────────────────────────

def slot_floor(dt: datetime) -> datetime:
    """Round a UTC datetime DOWN to the nearest 30-minute boundary."""
    return dt.replace(minute=(dt.minute // 30) * 30, second=0, microsecond=0)


def _parse_dt(value: str) -> datetime:
    """Parse ISO-8601 string to UTC-aware datetime, tolerating various formats."""
    dt = datetime.fromisoformat(value)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=TZ_UTC)
    return dt.astimezone(TZ_UTC)


def dispatches_to_slots(dispatches: list, from_dt: datetime, to_dt: datetime,
                        label: str) -> dict[datetime, str]:
    """Convert a list of dispatch records into {slot_utc: label} entries."""
    result = {}
    for d in dispatches:
        # GraphQL uses startDt/endDt; REST uses start/end
        try:
            ds = _parse_dt(d.get("startDt") or d["start"])
            de = _parse_dt(d.get("endDt")   or d["end"])
        except (KeyError, ValueError):
            continue
        source = (d.get("meta") or {}).get("source", label)
        cur = slot_floor(ds)
        while cur < de:
            if from_dt <= cur < to_dt:
                result[cur] = source
            cur += timedelta(minutes=30)
    return result


def build_rate_slots(from_dt: datetime, to_dt: datetime,
                     unit_rates: list) -> dict[datetime, str]:
    """
    Return {slot_utc: "rate_X.XXp"} for every half-hour whose unit rate is
    within 15 % of the tariff minimum (the off-peak band).
    Falls back to the guaranteed 23:30–05:30 window if no rate data is available.
    """
    if unit_rates:
        rates_map: dict[datetime, float] = {}
        for rec in unit_rates:
            vf = _parse_dt(rec["valid_from"])
            vt = (_parse_dt(rec["valid_to"])
                  if rec.get("valid_to") else vf + timedelta(minutes=30))
            cur = slot_floor(vf)
            while cur < vt:
                rates_map[cur] = rec["value_inc_vat"]
                cur += timedelta(minutes=30)
        if rates_map:
            min_rate  = min(rates_map.values())
            threshold = min_rate * 1.15   # within 15 % of minimum → cheap
            result = {slot: f"rate_{rate:.2f}p"
                      for slot, rate in rates_map.items()
                      if from_dt <= slot < to_dt and rate <= threshold}
            print(f"    Rate-based cheap slots : {len(result)}")
            return result

    # Fallback — no rate data
    print("    ⚠  No unit rate data — using guaranteed 23:30–05:30 window only")
    result = {}
    cur = from_dt
    while cur < to_dt:
        loc  = cur.astimezone(TZ_UK)
        h, m = loc.hour, loc.minute
        in_window = (
            (h == CHEAP_WINDOW_START[0] and m >= CHEAP_WINDOW_START[1]) or
            (h < CHEAP_WINDOW_END[0]) or
            (h == CHEAP_WINDOW_END[0] and m <= CHEAP_WINDOW_END[1])
        )
        if in_window:
            result[cur] = "guaranteed"
        cur += timedelta(minutes=30)
    return result


def build_dispatch_slots(from_dt: datetime, to_dt: datetime,
                         completed: list, planned: list) -> dict[datetime, str]:
    """Return {slot_utc: source_label} for every half-hour covered by a dispatch."""
    result: dict[datetime, str] = {}
    result.update(dispatches_to_slots(completed, from_dt, to_dt, "completed"))
    result.update(dispatches_to_slots(planned,   from_dt, to_dt, "planned"))
    print(f"    Dispatch slots         : {len(result)}")
    return result


# ── HA SQLite helpers ─────────────────────────────────────────────────────────

def _has_states_meta(conn: sqlite3.Connection) -> bool:
    tables = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()}
    return "states_meta" in tables


def fetch_ha_states(conn: sqlite3.Connection,
                    entity_id: str, from_ts: float, to_ts: float
                    ) -> list[tuple[datetime, str]]:
    """
    Return [(utc_datetime, state_str)] for an entity, ordered by time.
    Skips unavailable / unknown states.

    Modern HA schema (2023+): entity_id lives in states_meta; states is indexed
    on (metadata_id, last_updated_ts). We look up metadata_id first so the main
    query hits the index instead of scanning the full table.
    """
    skip = {"unavailable", "unknown", "None", ""}

    if _has_states_meta(conn):
        meta = conn.execute(
            "SELECT metadata_id FROM states_meta WHERE entity_id = ?", (entity_id,)
        ).fetchone()
        if not meta:
            return []
        metadata_id = meta[0]
        rows = conn.execute("""
            SELECT COALESCE(last_changed_ts, last_updated_ts), state
            FROM states
            WHERE metadata_id = ?
              AND last_updated_ts BETWEEN ? AND ?
              AND state NOT IN ('unavailable','unknown','None','')
            ORDER BY last_updated_ts
        """, (metadata_id, from_ts, to_ts)).fetchall()
        return [(datetime.fromtimestamp(r[0], tz=TZ_UTC), r[1])
                for r in rows if r[1] not in skip]
    else:
        # Old schema — entity_id column populated, datetime strings
        rows = conn.execute("""
            SELECT last_changed, state FROM states
            WHERE entity_id = ?
              AND last_changed BETWEEN datetime(?,'unixepoch')
                                   AND datetime(?,'unixepoch')
              AND state NOT IN ('unavailable','unknown','None','')
            ORDER BY last_changed
        """, (entity_id, from_ts, to_ts)).fetchall()
        return [(datetime.fromisoformat(r[0]).replace(tzinfo=TZ_UTC), r[1])
                for r in rows if r[1] not in skip]


# ── Per-slot calculations ─────────────────────────────────────────────────────

def on_fraction(states: list[tuple[datetime, str]],
                slot_s: datetime, slot_e: datetime) -> float:
    """
    Fraction of [slot_s, slot_e) that the entity was in state "on" or "1".
    Correctly handles state changes both before and within the slot.
    """
    total = (slot_e - slot_s).total_seconds()
    if total <= 0 or not states:
        return 0.0

    # Walk backwards to find the state active at slot_s
    cur_state = "off"
    for ts, st in states:
        if ts <= slot_s:
            cur_state = st
        else:
            break

    on_secs = 0.0
    prev    = slot_s

    for ts, st in states:
        if ts <= slot_s:
            cur_state = st
            continue
        if ts >= slot_e:
            break
        if cur_state in ("on", "1"):
            on_secs += (ts - prev).total_seconds()
        prev      = ts
        cur_state = st

    # Tail: from last change to slot end
    if cur_state in ("on", "1"):
        on_secs += (slot_e - prev).total_seconds()

    return min(on_secs / total, 1.0)


def peak_watts(states: list[tuple[datetime, str]],
               slot_s: datetime, slot_e: datetime) -> float:
    """
    Maximum numeric value for a sensor within the slot.
    Includes the value active at slot_s (last known before the slot).
    """
    cur_val = 0.0
    for ts, st in states:
        if ts <= slot_s:
            try:
                cur_val = float(st)
            except ValueError:
                pass
        else:
            break

    vals = [cur_val]
    for ts, st in states:
        if slot_s < ts < slot_e:
            try:
                vals.append(float(st))
            except ValueError:
                pass

    return max(vals)


# ── Flag logic ────────────────────────────────────────────────────────────────

def classify(oct_cheap: bool, ha_on: bool, zappi_chg: bool) -> str:
    if oct_cheap and not ha_on:
        return "MISSED_SLOT"         # Octopus said cheap — HA sensor missed it
    if ha_on and not oct_cheap:
        return "SENSOR_ONLY"         # HA fired with no matching API slot
    if oct_cheap and ha_on and zappi_chg:
        return "OK_CHARGING"         # all good, car charged
    if oct_cheap and ha_on:
        return "OK_IDLE"             # cheap window open, car not charging (fine)
    if not oct_cheap and zappi_chg:
        return "CHARGED_AT_PEAK"     # charged outside cheap window — worth checking
    return ""


# ── Night-window helper ───────────────────────────────────────────────────────

def night_label(local_dt: datetime) -> str | None:
    """
    Return the YYYY-MM-DD label for the night that owns this slot
    (night label = the date the 23:30 window STARTS on).
    Returns None if the slot is outside the 23:30–05:30 window.
    """
    h, m = local_dt.hour, local_dt.minute
    in_window = (
        (h == CHEAP_WINDOW_START[0] and m >= CHEAP_WINDOW_START[1]) or
        (h < CHEAP_WINDOW_END[0]) or
        (h == CHEAP_WINDOW_END[0] and m <= CHEAP_WINDOW_END[1])
    )
    if not in_window:
        return None
    if h < 12:   # early morning side → night started yesterday
        return (local_dt - timedelta(days=1)).strftime("%Y-%m-%d")
    return local_dt.strftime("%Y-%m-%d")


# ══════════════════════════════════════════════════════════════════════════════
# MAIN
# ══════════════════════════════════════════════════════════════════════════════

def main() -> None:
    OUTPUT_DIR.mkdir(exist_ok=True)
    stamp  = datetime.now().strftime("%Y%m%d_%H%M")
    to_dt  = datetime.now(TZ_UTC).replace(minute=0, second=0, microsecond=0)
    if FROM_DATE:
        from_dt = datetime.strptime(FROM_DATE, "%Y-%m-%d").replace(tzinfo=TZ_UK).astimezone(TZ_UTC)
    else:
        from_dt = to_dt - timedelta(days=AUDIT_DAYS)

    print()
    print("=" * 62)
    print("  v2h_octopus_audit — Intelligent Go slot verifier")
    print(f"  {from_dt.astimezone(TZ_UK):%d %b %Y}  →  {to_dt.astimezone(TZ_UK):%d %b %Y}"
          f"  ({AUDIT_DAYS} days)")
    print("=" * 62)

    # ── 1. Octopus account + tariff info ──────────────────────────────────────
    print("\n[1/4] Octopus account …")
    session = rest_session()
    jwt     = gql_token()          # needed for dispatch queries

    if all([CACHED_ACCOUNT, CACHED_MPAN, CACHED_SERIAL, CACHED_TARIFF]):
        parts = CACHED_TARIFF.split("-")
        info  = dict(
            account_number = CACHED_ACCOUNT,
            mpan           = CACHED_MPAN,
            serial         = CACHED_SERIAL,
            tariff_code    = CACHED_TARIFF,
            product_code   = "-".join(parts[2:-1]) if len(parts) >= 4 else "",
        )
        print(f"    Account : {CACHED_ACCOUNT} (config)")
        print(f"    MPAN    : {CACHED_MPAN}")
        print(f"    Tariff  : {CACHED_TARIFF}")
    else:
        info = get_account_info(session, jwt)

    # ── 2. Unit rates (for reference / Agile support) ─────────────────────────
    print("\n[2/4] Rates + dispatches …")
    unit_rates: list = []
    if info["product_code"] and info["tariff_code"]:
        unit_rates = get_unit_rates(session, info["product_code"],
                                    info["tariff_code"], from_dt, to_dt)

    gql_done, gql_planned = get_dispatches_gql(info["account_number"], jwt)

    if gql_done or gql_planned:
        completed, planned = gql_done, gql_planned
    else:
        print("    Falling back to REST dispatches …")
        completed, planned = get_dispatches_rest(session, info["account_number"])

    rate_map     = build_rate_slots(from_dt, to_dt, unit_rates)
    dispatch_map = build_dispatch_slots(from_dt, to_dt, completed, planned)
    cheap_map    = rate_map | dispatch_map   # union — dispatch extends rate window
    print(f"    Total cheap slots (rate + dispatch): {len(cheap_map)}")

    # ── 3. HA database ────────────────────────────────────────────────────────
    print("\n[3/4] HA sensor history …")
    if not Path(HA_DB_PATH).exists():
        sys.exit(
            f"\n✗  HA database not found at: {HA_DB_PATH}\n"
            "   Copy it first:\n"
            "     cp /mnt/ha_config/home-assistant_v2.db ha_db.db\n"
            "   Then set HA_DB_PATH in the CONFIG section."
        )

    conn = sqlite3.connect(HA_DB_PATH)
    try:
        conn.execute("PRAGMA integrity_check(1)").fetchone()
    except sqlite3.DatabaseError as e:
        sys.exit(
            f"\n✗  HA database is corrupt: {e}\n"
            "   Re-run run_audit.sh to take a fresh backup using the safe online backup.\n"
            "   (Plain 'cp' of a live SQLite database can produce a corrupt copy.)"
        )
    from_ts, to_ts = from_dt.timestamp(), to_dt.timestamp()

    print(f"    {OFFPEAK_ENTITY}")
    offpeak_states = fetch_ha_states(conn, OFFPEAK_ENTITY,     from_ts, to_ts)
    print(f"      → {len(offpeak_states):,} state changes")

    print(f"    {ZAPPI_ENTITY}")
    zappi_states   = fetch_ha_states(conn, ZAPPI_ENTITY,       from_ts, to_ts)
    print(f"      → {len(zappi_states):,} state changes")

    print(f"    {SMARTCHARGE_ENTITY}")
    sc_states      = fetch_ha_states(conn, SMARTCHARGE_ENTITY, from_ts, to_ts)
    print(f"      → {len(sc_states):,} state changes")

    conn.close()

    # ── 4. Build slot table ───────────────────────────────────────────────────
    print("\n[4/4] Building slot table …")

    detail_rows: list[dict] = []
    nights: dict[str, dict] = {}

    cur = from_dt
    while cur < to_dt:
        slot_s = cur
        slot_e = cur + timedelta(minutes=30)
        local  = slot_s.astimezone(TZ_UK)
        cur   += timedelta(minutes=30)

        oct_cheap    = slot_s in cheap_map
        rate_cheap   = slot_s in rate_map
        dispatched   = slot_s in dispatch_map
        oct_source   = dispatch_map.get(slot_s) or rate_map.get(slot_s, "")

        op_frac    = on_fraction(offpeak_states, slot_s, slot_e)
        ha_op_on   = op_frac >= ON_FRACTION_MIN

        z_max      = peak_watts(zappi_states, slot_s, slot_e)
        zappi_chg  = z_max >= ZAPPI_ON_W

        sc_frac    = on_fraction(sc_states, slot_s, slot_e)
        sc_on      = sc_frac >= ON_FRACTION_MIN

        flag = classify(oct_cheap, ha_op_on, zappi_chg)

        detail_rows.append({
            "date":            local.strftime("%Y-%m-%d"),
            "day":             local.strftime("%a"),
            "slot_local":      local.strftime("%H:%M"),
            "slot_utc":        slot_s.strftime("%Y-%m-%dT%H:%MZ"),
            "rate_cheap":      "Y" if rate_cheap  else "",
            "dispatched":      "Y" if dispatched  else "",
            "oct_cheap":       "Y" if oct_cheap   else "",
            "oct_source":      oct_source,
            "ha_offpeak_%":    f"{op_frac*100:.0f}",
            "ha_offpeak":      "Y" if ha_op_on    else "",
            "zappi_max_w":     f"{z_max:.0f}",
            "zappi_charging":  "Y" if zappi_chg   else "",
            "smartcharge":     "Y" if sc_on        else "",
            "flag":            flag,
        })

        # Night-summary accumulation
        nl = night_label(local)
        if nl:
            n = nights.setdefault(nl, {
                "slots": 0, "oct": 0, "ha": 0,
                "zappi": 0, "missed": 0, "sensor_only": 0, "charged_peak": 0,
            })
            n["slots"] += 1
            if oct_cheap:  n["oct"]    += 1
            if ha_op_on:   n["ha"]     += 1
            if zappi_chg:  n["zappi"]  += 1
            if flag == "MISSED_SLOT":    n["missed"]       += 1
            if flag == "SENSOR_ONLY":    n["sensor_only"]  += 1
            if flag == "CHARGED_AT_PEAK": n["charged_peak"] += 1

    # ── Write detail CSV ──────────────────────────────────────────────────────
    detail_path = OUTPUT_DIR / f"slot_detail_{stamp}.csv"
    fieldnames  = list(detail_rows[0].keys())

    with open(detail_path, "w", newline="", encoding="utf-8-sig") as f:
        w = csv.DictWriter(f, fieldnames=fieldnames)
        w.writeheader()
        w.writerows(detail_rows)

        # Totals footer
        f.write("\n")
        totals: dict[str, str] = {k: "" for k in fieldnames}
        totals.update({
            "date":           "TOTALS",
            "rate_cheap":     str(sum(1 for r in detail_rows if r["rate_cheap"])),
            "dispatched":     str(sum(1 for r in detail_rows if r["dispatched"])),
            "oct_cheap":      str(sum(1 for r in detail_rows if r["oct_cheap"])),
            "ha_offpeak":     str(sum(1 for r in detail_rows if r["ha_offpeak"])),
            "zappi_charging": str(sum(1 for r in detail_rows if r["zappi_charging"])),
            "smartcharge":    str(sum(1 for r in detail_rows if r["smartcharge"])),
            "flag": (f"MISSED={sum(1 for r in detail_rows if r['flag']=='MISSED_SLOT')}  "
                     f"SENSOR_ONLY={sum(1 for r in detail_rows if r['flag']=='SENSOR_ONLY')}  "
                     f"CHARGED_AT_PEAK={sum(1 for r in detail_rows if r['flag']=='CHARGED_AT_PEAK')}"),
        })
        csv.DictWriter(f, fieldnames=fieldnames).writerow(totals)

    # ── Write night summary CSV ───────────────────────────────────────────────
    summary_path  = OUTPUT_DIR / f"night_summary_{stamp}.csv"
    summary_rows  = []
    for nd in sorted(nights):
        n   = nights[nd]
        end = (datetime.strptime(nd, "%Y-%m-%d") + timedelta(days=1)).strftime("%Y-%m-%d")
        pct = f"{n['ha'] / n['slots'] * 100:.0f}%" if n["slots"] else "—"
        if n["missed"] > 0:
            status = f"✗  {n['missed']} MISSED"
        elif n["sensor_only"] > 0:
            status = f"⚠  {n['sensor_only']} SENSOR_ONLY"
        elif n["charged_peak"] > 0:
            status = f"⚡ {n['charged_peak']} CHARGED_AT_PEAK"
        else:
            status = "OK"
        summary_rows.append({
            "night_start":          nd,
            "night_end":            end,
            "window":               "23:30–05:30",
            "expected_slots":       n["slots"],
            "oct_cheap_slots":      n["oct"],
            "ha_detected_slots":    n["ha"],
            "ha_coverage_%":        pct,
            "zappi_charging_slots": n["zappi"],
            "missed_slots":         n["missed"],
            "sensor_only_slots":    n["sensor_only"],
            "status":               status,
        })

    with open(summary_path, "w", newline="", encoding="utf-8-sig") as f:
        w = csv.DictWriter(f, fieldnames=list(summary_rows[0].keys()))
        w.writeheader()
        w.writerows(summary_rows)

    # ── Console summary ───────────────────────────────────────────────────────
    total_slots    = len(detail_rows)
    total_rate     = sum(1 for r in detail_rows if r["rate_cheap"])
    total_dispatch = sum(1 for r in detail_rows if r["dispatched"])
    total_cheap    = sum(1 for r in detail_rows if r["oct_cheap"])
    total_ha       = sum(1 for r in detail_rows if r["ha_offpeak"])
    total_zappi    = sum(1 for r in detail_rows if r["zappi_charging"])
    n_missed       = sum(1 for r in detail_rows if r["flag"] == "MISSED_SLOT")
    n_sensor_only  = sum(1 for r in detail_rows if r["flag"] == "SENSOR_ONLY")
    n_charged_peak = sum(1 for r in detail_rows if r["flag"] == "CHARGED_AT_PEAK")

    print()
    print("=" * 62)
    print(f"  Total half-hour slots examined  : {total_slots:,}")
    print(f"  Cheap by tariff rate            : {total_rate:,}")
    print(f"  Cheap by dispatch               : {total_dispatch:,}")
    print(f"  Total cheap (rate + dispatch)   : {total_cheap:,}")
    print(f"  HA off_peak sensor detected     : {total_ha:,}")
    print(f"  Zappi charging slots            : {total_zappi:,}")
    print(f"  ──────────────────────────────────────────────────")
    print(f"  ✗  MISSED_SLOT                  : {n_missed:,}"
          + ("  ← investigate!" if n_missed else ""))
    print(f"  ⚠  SENSOR_ONLY                 : {n_sensor_only:,}")
    print(f"  ⚡ CHARGED_AT_PEAK              : {n_charged_peak:,}"
          + ("  ← check billing!" if n_charged_peak else ""))
    print()
    print(f"  Detail CSV   → {detail_path}")
    print(f"  Night CSV    → {summary_path}")
    print("=" * 62)
    print()


if __name__ == "__main__":
    main()
