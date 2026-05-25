# Bugs reported on Discord

Source: `arctis-manager-general.20260514.0815.json` (5,924 messages, 2026-01-09 → 2026-05-14, ~80 distinct users).

| # | Title | Device/Component | Description | Frequency | Last discussed |
|---|---|---|---|---|---|
| 1 | ChatMix breaks after resume from sleep / no re-detect | Nova Pro Wireless, Nova 7 Gen 2 — daemon/USB | After suspend, daemon still runs but USB calls fail (Errno 5/110); sinks vanish or chatmix freezes. v2.2.0 wake re-init only fixed it on some boards (S2idle/S3 dependent). Workaround: restart user service. | **Very high** (Aiyahhh, Miles Tormani, Ian the Dragon, Dagron, Wylel, Khorheal, June, Pyromage, vLTD) | 2026-05-14 |
| 2 | USB I/O errors (Errno 5 / Errno 110) on init or after long uptime | Nova Pro Wireless, Nova 7 — CoreEngine | Daemon spams I/O errors until restart. Sometimes tied to USB 2.0 port. v2.3.1 added re-detect on write error but only partially fixes it. | **High** (VinCheezel, Miles Tormani, vLTD, Dagron, Flame, s1lv3r, Wylel) | 2026-05-07 |
| 3 | udev rules write fails / wrong path | `lam-cli udev write-rules` / AUR installer | Errno 13 when run as sudo; immutable distros (Bazzite) need different path; AUR system-wide install writes to /etc and gets clobbered by /usr on pacman upgrade. | **High** (Donut, Skill, Dagron, DrJawB0n3s, June, edaK, patrickRx, Khorheal, TacticalBill, Baztion, Aiyahhh) | 2026-05-08 |
| 4 | Redirect-on-disconnect / BT state not reporting | Nova Pro Wireless — GUI | BT state always shown off; fallback-audio dropdown sometimes empty; fallback doesn't fire on power-off. | Medium (Aiyahhh, Miles Tormani) | 2026-04-14 |
| 5 | Battery % wrong (74% at full, or 1000%) | Nova Pro Wireless, Nova 7 WoW — battery decode | Wylel sees 74% at full (treats 75 as max). WoW edition saw 1000% — needs discrete-vs-percentage variant detection. | Medium | 2026-05-12 |
| 6 | ChatMix on-screen volume display broken (function works) | Nova Pro Wireless GameDAC — status decoding | The GUI bar/value is stale; audio routing itself is correct. | Medium (Aiyahhh) | 2026-04-30 |
| 7 | HW ↔ system volume sync drifts | Nova Pro Wireless | Maintainer-acknowledged known bug; no fix yet. | Medium (Miles Tormani, elegos) | 2026-04-19 |
| 8 | Crackling/pops blamed on LAM | Multiple — virtual sinks vs HW | Restart temporarily clears it. Nova 5 dongle rebinds itself ~20s (HW). Likely sink-creation interaction rather than LAM proper. | Medium (Sandoicchi, ondrej_lakota, Wylel, Aiyahhh) | 2026-05-11 |
| 9 | DAC freeze; sinks not recreated on replug | Nova Pro Wireless GameDAC | After unplug/replug LAM detects device but doesn't recreate Arctis_Media/Arctis_Chat sinks; needs manual `lam-cli setup`. | Medium (Wylel) | 2026-05-07 |
| 10 | Mic mono-only / virtual mic dies after ~10s | Nova Pro Wireless — PipeWire | Andreas: mono only on Arch. Aiyahhh: virtual mic dies ~10s, EasyEffects toggle revives it briefly. | Low (Andreas, Aiyahhh, June) | 2026-04-27 |
| 11 | Daemon exception with bad config key | `nova_5.yaml` | Line 150 uses `values_mapping` instead of `values`; PM-shutdown labels also wrong. | Low (nrwlia) | 2026-03-18 |
| 12 | `lam-cli setup` flags in docs are wrong | CLI / docs | Docs show `--systray-autostart --start-now` but CLI rejects them. | Low (multiple paste-and-fail) | 2026-04-17 |
| 13 | Stale user-systemd unit + udev rules after AUR migration | AUR / installer hygiene | Old `~/.local`-era files conflict with new `/usr`-era install. Aiyahhh wrote a post_upgrade hook. | Low | 2026-05-09 |
