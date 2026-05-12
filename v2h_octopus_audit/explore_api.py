#!/usr/bin/env python3
"""
explore_api.py — Octopus Kraken GraphQL API explorer
Calls each query of interest and writes query + raw response to api_explore/.
Sample date: 2026-05-08 (one full day).

Usage:  .venv/bin/python explore_api.py
Output: api_explore/NN_queryname.txt
"""

import json
import tomllib
import requests
from pathlib import Path

# ── Config ────────────────────────────────────────────────────────────────────
cfg    = tomllib.load(open(Path(__file__).parent / "config.toml", "rb"))
KEY       = cfg["octopus"]["api_key"]
ACCT      = cfg["octopus"]["account_number"]
MPAN      = cfg["octopus"]["mpan"]
SERIAL    = cfg["octopus"]["serial"]
METER_ID  = cfg["octopus"]["meter_id"]
PROP_ID   = cfg["octopus"]["property_id"]
SM_UUID   = cfg["octopus"]["smart_meter_uuid"]

GQL    = "https://api.octopus.energy/v1/graphql/"
OUT    = Path(__file__).parent / "api_explore"
OUT.mkdir(exist_ok=True)

FROM   = "2026-05-08T00:00:00+00:00"
TO     = "2026-05-09T00:00:00+00:00"

# ── Helpers ───────────────────────────────────────────────────────────────────

def get_jwt():
    r = requests.post(GQL, json={
        "query": "mutation($k:String!){obtainKrakenToken(input:{APIKey:$k}){token}}",
        "variables": {"k": KEY},
    })
    return r.json()["data"]["obtainKrakenToken"]["token"]


def gql(jwt, query, variables=None):
    r = requests.post(GQL,
                      json={"query": query, "variables": variables or {}},
                      headers={"Authorization": f"JWT {jwt}"})
    return r.status_code, r.json()


def save(n, name, query_text, status, result):
    path = OUT / f"{n:02d}_{name}.txt"
    with open(path, "w", encoding="utf-8") as f:
        f.write(f"Query: {name}\n")
        f.write("=" * 70 + "\n\n")
        f.write("GraphQL:\n")
        f.write(query_text.strip())
        f.write(f"\n\nHTTP status: {status}\n\n")
        f.write("Response:\n")
        f.write(json.dumps(result, indent=2, default=str))
    ok = "errors" not in result
    print(f"  {'✓' if ok else '✗'} {path.name}")


jwt = get_jwt()
print(f"JWT obtained  account={ACCT}  MPAN={MPAN}  serial={SERIAL}\n")

# ── 01. completedDispatches ───────────────────────────────────────────────────
q = """
query {
  completedDispatches(accountNumber: "%s") {
    start end delta
    meta { source location }
  }
}
""" % ACCT
save(1, "completedDispatches", q, *gql(jwt, q))

# ── 02. plannedDispatches ─────────────────────────────────────────────────────
q = """
query {
  plannedDispatches(accountNumber: "%s") {
    start end delta
    meta { source location }
  }
}
""" % ACCT
save(2, "plannedDispatches", q, *gql(jwt, q))

# ── 03. account (properties, agreements, balance) ────────────────────────────
q = """
query {
  account(accountNumber: "%s") {
    number brand balance overdueBalance projectedBalance createdAt
    billingName billingEmail billingAddressPostcode
    properties {
      id postcode address splitAddress
      electricityMeterPoints {
        mpan
        meters { serialNumber id meterType activeFrom activeTo }
        agreements { id validFrom validTo agreedFrom agreedTo isRevoked }
      }
    }
    electricityAgreements { id validFrom validTo agreedFrom agreedTo isRevoked }
    ledgers { name ledgerType balance }
    paymentAdequacy { suggestedDirectDebitAmount minimumDirectDebitAmount }
  }
}
""" % ACCT
save(3, "account", q, *gql(jwt, q))

# ── 04. electricityMeterReadings (half-hourly consumption) ───────────────────
q = """
query {
  electricityMeterReadings(
    accountNumber: "%s"
    meterId: "%s"
    readFrom: "%s"
    readTo: "%s"
    first: 100
  ) {
    totalCount
    edges {
      node {
        id readAt readingSource source readingType
        registers { identifier name value digits isQuarantined }
      }
    }
    pageInfo { hasNextPage endCursor }
  }
}
""" % (ACCT, METER_ID, FROM, TO)
save(4, "electricityMeterReadings", q, *gql(jwt, q))

# ── 05. applicableRates (tariff rates for the meter point) ───────────────────
q = """
query {
  applicableRates(
    accountNumber: "%s"
    mpxn: "%s"
    startAt: "%s"
    endAt: "%s"
    first: 100
  ) {
    totalCount
    edges {
      node { value validFrom validTo }
    }
  }
}
""" % (ACCT, MPAN, FROM, TO)
save(5, "applicableRates", q, *gql(jwt, q))

# ── 06. devices (registered smart / flex devices) ────────────────────────────
q = """
query {
  devices(accountNumber: "%s") {
    id name integrationDeviceId propertyId
    onboardingWizard { id displayName }
  }
}
""" % ACCT
save(6, "devices", q, *gql(jwt, q))

# ── 07. productEnrolments ─────────────────────────────────────────────────────
q = """
query {
  productEnrolments(accountNumber: "%s") {
    id
    product {
      code displayName description
      isVariable isGreen isBusiness isChargedHalfHourly isPrepay term
    }
    stages { name }
  }
}
""" % ACCT
save(7, "productEnrolments", q, *gql(jwt, q))

# ── 08. accountIoEligibility ──────────────────────────────────────────────────
q = """
query {
  accountIoEligibility(accountNumber: "%s") {
    isEligibleForIo
  }
}
""" % ACCT
save(8, "accountIoEligibility", q, *gql(jwt, q))

# ── 09. smartMeterDataPreferences + consumption via meter point ──────────────
q = """
query {
  smartMeterDataPreferences(accountNumber: "%s") {
    readingFrequency readingsAnalysisConsentProvided readingsAnalysisConsentUpdatedDatetime
  }
}
""" % ACCT
save(9, "smartMeterDataPreferences", q, *gql(jwt, q))

# ── 10. weeklyUsageInsights ───────────────────────────────────────────────────
q = """
query {
  weeklyUsageInsights(accountNumber: "%s") {
    weekStart periodStart periodEnd numberPeriods hasFullReadings
    consumptionKwh carbonGrams achievedCarbonRate achievedCarbonRank
    mpan gspGroupId isLatestWeek
  }
}
""" % ACCT
save(10, "weeklyUsageInsights", q, *gql(jwt, q))

# ── 11. fanClubStatus ─────────────────────────────────────────────────────────
q = """
query {
  fanClubStatus(accountNumber: "%s") {
    name location windFarm discountSource accountNumbers
    current { startAt discount metaData { power windSpeed windDirection windPowerOnGrid windPowerProportion } }
  }
}
""" % ACCT
save(11, "fanClubStatus", q, *gql(jwt, q))

print("\nDone — see api_explore/")
