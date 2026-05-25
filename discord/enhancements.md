# Enhancement requests from Discord

Source: `arctis-manager-general.20260514.0815.json` (5,924 messages, 2026-01-09 → 2026-05-14, ~80 distinct users).

| # | Title | Device/Component | Description | Frequency | Last discussed |
|---|---|---|---|---|---|
| E1 | Support: Nova 7+ / 7X / 7P / WoW | New device YAMLs | WoW merged 2026-05-12 (battery still wrong); 7+ added early May; 7X/7P still missing. | **Very high** | 2026-05-12 |
| E2 | Support: Arctis 7 / 7 (2019) / Arctis 1 | New device YAMLs | Supported in legacy v1; not in v2. nano offered RE help. | High | 2026-05-02 |
| E3 | Support: GameDAC Gen 2 wired / Nova Pro Wired | `nova_pro_wired.yaml` | Baztion (1038:12cd) drafting Python integration. | High (active contributor) | 2026-04-24 |
| E4 | Support: Nova 4P / 4X | Tonitch | Maintains AUR but his own 4P isn't supported yet. | Medium | 2026-04-23 |
| E5 | Built-in equalizer with presets | GUI | Most-requested feature. Loteran's fork has it. elegos open as plugin/separate. | High | 2026-03-25 |
| E6 | Mic noise cancellation built-in | Mic pipeline | Users want it without setting up EasyEffects. | Medium (Aiyahhh, Donut) | 2026-04-28 |
| E7 | Autostart toggle + crash auto-restart | systemd / GUI | Currently manual desktop-file copy; KDE+GNOME autostart on table. | Medium | 2026-03-30 |
| E8 | "Restart Service" button in GUI | GUI | Workaround surface for the sleep/USB bug. | Low (recent) | 2026-05-14 |
| E9 | Dongle hot-plug detection | USB monitor | Dongle plugged after boot is not picked up. | Low | 2026-05-14 |
| E10 | First-run setup wizard (udev/autostart) | Onboarding | Would replace the CLI dance many users trip over. | Low (maintainer endorsed) | 2026-03-30 |
| E11 | GameDAC OLED display control | Nova Pro Wireless | ggoled/OmniLED-style temps/stats. elegos prefers plugin. | Low | 2026-04-05 |
| E12 | BT power-up-state toggle | Nova Pro Wireless | Dagron has the bytes for Nova 7; not in LAM GUI yet. | Low | 2026-05-11 |
| E13 | Plugin/extensibility system | Architecture | elegos open to it as the way to land custom UIs/EQ/OLED. | Low | 2026-03-25 |
| E14 | Surface missing deps / no-udev to user | GUI / launcher | Missing `libxcb-cursor0` causes silent failure. | Low | 2026-03-05 |
