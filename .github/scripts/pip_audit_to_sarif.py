#!/usr/bin/env python3
"""Convert `pip-audit --format json` output to SARIF 2.1.0 for GitHub Code Scanning."""
import json
import sys

SARIF_SCHEMA = (
    "https://raw.githubusercontent.com/oasis-tcs/sarif-spec"
    "/master/Schemata/sarif-schema-2.1.0.json"
)

# pip-audit does not provide severity; treat all findings as warnings.
_LEVEL = "warning"


def _rule(vuln_id, description, aliases):
    cve = next((a for a in aliases if a.startswith("CVE-")), None)
    ghsa = next((a for a in aliases if a.startswith("GHSA-")), None)
    help_uri = (
        f"https://osv.dev/vulnerability/{vuln_id}"
        if not ghsa
        else f"https://github.com/advisories/{ghsa}"
    )
    title = cve or vuln_id
    return {
        "id": vuln_id,
        "shortDescription": {"text": title},
        "fullDescription": {"text": description[:1000]},
        "helpUri": help_uri,
        "properties": {"tags": ["security", "supply-chain"]},
    }


def _location(pkg_name, pkg_version):
    return {
        "physicalLocation": {
            "artifactLocation": {
                "uri": "uv.lock",
                "uriBaseId": "%SRCROOT%",
            },
            "region": {"startLine": 1},
        },
        "logicalLocations": [
            {"name": f"{pkg_name} {pkg_version}".strip(), "kind": "package"}
        ],
    }


def convert(data):
    rules, results = [], []
    seen_rules: set[str] = set()
    seen_results: set[tuple[str, str]] = set()

    for dep in data.get("dependencies", []):
        name = dep.get("name", "?")
        version = dep.get("version", "")
        for vuln in dep.get("vulns", []):
            vid = vuln.get("id", "unknown")
            aliases = vuln.get("aliases", [])
            description = vuln.get("description", "")
            fix = ", ".join(vuln.get("fix_versions", [])) or "no fix available"

            if vid not in seen_rules:
                seen_rules.add(vid)
                rules.append(_rule(vid, description, aliases))

            key = (name, vid)
            if key in seen_results:
                continue
            seen_results.add(key)

            msg = f"{vid} in {name} {version}. Fix: {fix}. {description}"
            results.append(
                {
                    "ruleId": vid,
                    "level": _LEVEL,
                    "message": {"text": msg[:2000]},
                    "locations": [_location(name, version)],
                }
            )

    return {
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [
            {
                "tool": {
                    "driver": {
                        "name": "pip-audit",
                        "informationUri": "https://github.com/pypa/pip-audit",
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
        sys.exit(f"Failed to parse pip-audit JSON: {e}")
    json.dump(convert(data), sys.stdout, indent=2)
