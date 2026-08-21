#!/usr/bin/env python3
"""Convert `cargo audit --json` output to SARIF 2.1.0 for GitHub Code Scanning."""
import json
import sys

_SEVERITY = {"critical": "error", "high": "error", "medium": "warning", "low": "note"}

SARIF_SCHEMA = (
    "https://raw.githubusercontent.com/oasis-tcs/sarif-spec"
    "/master/Schemata/sarif-schema-2.1.0.json"
)


def _location(pkg):
    name = pkg.get("name", "?") if isinstance(pkg, dict) else str(pkg)
    version = pkg.get("version", "") if isinstance(pkg, dict) else ""
    return {
        "physicalLocation": {
            "artifactLocation": {
                "uri": "daemon/Cargo.lock",
                "uriBaseId": "%SRCROOT%",
            },
            "region": {"startLine": 1},
        },
        "logicalLocations": [
            {"name": f"{name} {version}".strip(), "kind": "package"}
        ],
    }


def _rule(rule_id, title, url):
    return {
        "id": rule_id,
        "shortDescription": {"text": title},
        "helpUri": url or "https://rustsec.org",
        "properties": {"tags": ["security", "supply-chain"]},
    }


def convert(data):
    rules, results, seen = [], [], set()

    for vuln in data.get("vulnerabilities", {}).get("list", []):
        adv = vuln["advisory"]
        pkg = vuln["package"]
        rid = adv["id"]
        if rid not in seen:
            seen.add(rid)
            rules.append(_rule(rid, adv.get("title", rid), adv.get("url")))
        patched = (
            ", ".join(vuln.get("versions", {}).get("patched", []))
            or "no fix available"
        )
        msg = (
            f"{adv.get('title', rid)} in {pkg.get('name', '?')} "
            f"{pkg.get('version', '')}. Patched: {patched}."
        )
        if adv.get("url"):
            msg += f" {adv['url']}"
        results.append(
            {
                "ruleId": rid,
                "level": _SEVERITY.get(adv.get("severity", ""), "warning"),
                "message": {"text": msg},
                "locations": [_location(pkg)],
            }
        )

    for kind, warnings in data.get("warnings", {}).items():
        for w in warnings:
            adv = w.get("advisory") or {}
            pkg = w.get("package") or {}
            rid = adv.get("id") or f"rustsec-{kind}-{pkg.get('name', 'unknown')}"
            if rid not in seen:
                seen.add(rid)
                rules.append(_rule(rid, adv.get("title", kind), adv.get("url")))
            msg = (
                f"{adv.get('title', kind)}: "
                f"{pkg.get('name', '?')} {pkg.get('version', '')}."
            )
            if adv.get("url"):
                msg += f" {adv['url']}"
            results.append(
                {
                    "ruleId": rid,
                    "level": "note",
                    "message": {"text": msg},
                    "locations": [_location(pkg)],
                }
            )

    return {
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [
            {
                "tool": {
                    "driver": {
                        "name": "cargo-audit",
                        "informationUri": "https://github.com/rustsec/rustsec",
                        "rules": rules,
                    }
                },
                "results": results,
            }
        ],
    }


if __name__ == "__main__":
    try:
        data = json.load(sys.stdin)
    except json.JSONDecodeError as e:
        sys.exit(f"Failed to parse cargo audit JSON: {e}")
    json.dump(convert(data), sys.stdout, indent=2)
