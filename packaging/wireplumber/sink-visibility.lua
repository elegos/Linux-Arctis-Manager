-- Linux Arctis Manager: optional physical-sink hiding (GH #67).
--
-- Always loaded (see 51-lam-sink-visibility.conf) but a no-op unless the
-- user turns on the "hide_physical_sink" General setting AND a device is
-- currently connected. Only hides the sink from the desktop shell's own
-- quick-settings output picker (see QUICK_PANEL_BINARIES below) -- the
-- full audio control panel, pavucontrol, etc. keep normal visibility, so
-- the user can always regain full control there. WirePlumber enforces
-- per-client object permissions
-- via `client:update_permissions{}`, applied once per client as it connects
-- (see WirePlumber's own scripts/client/access-default.lua and
-- scripts/node/software-dsp.lua, whose "hide-parent" feature this mirrors) —
-- there is no external one-shot call that could hide an object from clients
-- that connect *after* the call runs. Hence this always-loaded policy
-- script instead of something the daemon starts/stops directly.
--
-- The daemon signals "keep hiding" by the mere presence of a lease file
-- (see `LeaseGuard` in daemon/engine/src/sink_visibility.rs) -- created
-- while the setting is on and a device is connected, removed as soon as
-- either stops being true. This script watches that file reactively via
-- WirePlumber's own `file-monitor-api` plugin (the same one
-- monitors/alsa-midi.lua uses for /dev/snd), not by polling: an earlier
-- version used a `Core.timeout_add` poll loop that, despite matching the
-- documented GSourceFunc contract (return true to repeat), reliably fired
-- exactly once and then went silent on this WirePlumber build (0.5.14) --
-- confirmed live over several redeploys during the GH #67 report, cause
-- unresolved. file-monitor-api sidesteps the question entirely.
--
-- Crash safety (never leaving the sink hidden forever if the daemon dies)
-- is NOT this script's job: `lam-daemon.service`'s own `ExecStopPost=`
-- removes the lease file unconditionally whenever that unit stops for any
-- reason, including a crash, which this script observes like any other
-- deletion.

local log = Log.open_topic("s-lam-sink-visibility")

local LEASE_DIR = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
local LEASE_NAME = "lam-sink-visibility.lease"
local LEASE_PATH = LEASE_DIR .. "/" .. LEASE_NAME

-- Only hide the sink from the desktop shell's own quick-settings output
-- picker, not from every client -- confirmed live (GH #67 report) that
-- `plasmashell` (KDE's panel/quick-settings host) is a distinct PipeWire
-- client from the full "Audio Volume" control panel (a separate process,
-- e.g. systemsettings/kcmshell6), so targeting only these binaries by name
-- leaves the full control panel, pavucontrol, etc. with normal visibility
-- -- the user can always regain full control there if something looks
-- wrong. `gnome-shell` is GNOME's equivalent (its quick-settings sound
-- menu lives in the shell process; the full Sound settings panel is the
-- separate `gnome-control-center` process) -- not verified live, only by
-- the same "shell process hosts quick settings" pattern as KDE.
local QUICK_PANEL_BINARIES = {
  ["plasmashell"] = true,
  ["gnome-shell"] = true,
}

local function is_quick_panel_client(client_props)
  local binary = client_props["application.process.binary"]
  return binary ~= nil and QUICK_PANEL_BINARIES[binary] == true
end

-- SteelSeries USB vendor ID -- matches STEELSERIES_VID in
-- daemon/engine/src/audio.rs. Unlike `pactl -f json list sinks` (which
-- audio.rs's own find_physical_sink() greps), WirePlumber's native Node
-- properties do NOT carry `device.vendor.id` -- that lives on the separate
-- parent Device object, and pactl only sees it merged in because the
-- PulseAudio-compat layer does that merge itself. The one Node-level
-- property that does carry it is `alsa.components`, e.g. "USB1038:12e0"
-- (verified against a live WirePlumber 0.5.14 session, GH #67 report).
local VENDOR_ID_PATTERN = "USB1038:"

-- True for the physical Arctis sink, false for LAM's own virtual sinks or
-- any other device -- mirrors `find_physical_sink()` in audio.rs.
local function is_physical_arctis_sink(props)
  local name = props["node.name"] or ""
  if name == "Arctis_Media" or name == "Arctis_Chat" then
    return false
  end
  local components = (props["alsa.components"] or ""):upper()
  return components:find(VENDOR_ID_PATTERN, 1, true) ~= nil
end

-- Same Interest/Constraint shape as WirePlumber's own fallback-sink.lua for
-- watching Audio/Sink nodes.
local sink_om = ObjectManager {
  Interest {
    type = "node",
    Constraint { "media.class", "matches", "Audio/Sink", type = "pw-global" },
  },
}

local clients_om = ObjectManager {
  Interest { type = "client" },
}

local function find_arctis_bound_id()
  for node in sink_om:iterate { type = "node" } do
    if is_physical_arctis_sink(node.properties) then
      return node["bound-id"]
    end
  end
  return nil
end

-- The physical sink's current PipeWire global (bound) id, or nil while no
-- Arctis device is connected/detected yet. Whether it's currently hidden.
local arctis_bound_id = nil
local hidden = false

local function set_permission_on_all_clients(perm)
  if arctis_bound_id == nil then
    return
  end
  local count = 0
  for client in clients_om:iterate { type = "client" } do
    if is_quick_panel_client(client["properties"]) then
      client:update_permissions { [arctis_bound_id] = perm }
      count = count + 1
    end
  end
  log:info("set permission '" .. perm .. "' on node " .. arctis_bound_id .. " for " .. count .. " quick-panel client(s)")
end

-- Re-evaluates and applies visibility. Called from the file-monitor
-- callback below (lease created/deleted) and once at script load (in case
-- WirePlumber itself was restarted while a lease already existed).
local function apply_visibility()
  arctis_bound_id = find_arctis_bound_id()
  local should_hide = arctis_bound_id ~= nil and GLib.access(LEASE_PATH, "r")
  if should_hide == hidden then
    return
  end
  log:info("visibility transition: hidden=" .. tostring(hidden) .. " -> " .. tostring(should_hide)
    .. " (arctis_bound_id=" .. tostring(arctis_bound_id) .. ")")
  set_permission_on_all_clients(should_hide and "-" or "all")
  hidden = should_hide
end

-- Safety net alongside the lease-file watch below: re-evaluate whenever any
-- Audio/Sink node appears or disappears. Needed because `alsa.components`
-- (which is_physical_arctis_sink keys off) is populated by the ALSA
-- monitor asynchronously -- confirmed live (GH #67 report) that the very
-- first "object-added" for the physical Arctis node can fire before that
-- property is set, and without this, nothing else would ever re-check
-- find_arctis_bound_id() if that first look missed it (no polling here to
-- self-correct). A later, unrelated Audio/Sink event (e.g. the daemon's
-- own Arctis_Media/Arctis_Chat sinks appearing moments afterwards) gives
-- find_arctis_bound_id() another chance, by which point the property
-- should be populated.
sink_om:connect("object-added", apply_visibility)
sink_om:connect("object-removed", apply_visibility)

-- A client that connects while hiding is already in effect needs the same
-- override applied to it directly -- mirrors software-dsp.lua's own
-- clients_om:connect("object-added", ...) for its `hidden_nodes` table.
clients_om:connect("object-added", function(_, client)
  if hidden and arctis_bound_id ~= nil and is_quick_panel_client(client["properties"]) then
    client:update_permissions { [arctis_bound_id] = "-" }
  end
end)

sink_om:activate()
clients_om:activate()

local fm_plugin = Plugin.find("file-monitor-api")
if fm_plugin == nil then
  log:warning("file-monitor-api plugin not available, hide_physical_sink will not react to lease changes")
else
  fm_plugin:connect("changed", function(_, file, _, evtype)
    if file == LEASE_PATH then
      apply_visibility()
    end
  end)
  -- "m" matches the one confirmed-working real usage of this API
  -- (monitors/alsa-midi.lua watching /dev/snd) -- the exact meaning of the
  -- flag isn't documented anywhere accessible, so this mirrors known-good
  -- usage rather than guessing at an unverified alternative.
  fm_plugin:call("add-watch", LEASE_DIR, "m")
end

-- Cover the case where the lease already existed when this script started
-- (e.g. WirePlumber restarting while hiding was already active).
apply_visibility()

-- Bounded settle-time retries on top of the "safety net" above: confirmed
-- live (GH #67 report) that even the retroactive "object-added" events
-- sink_om:activate() fires for pre-existing nodes can all occur before the
-- ALSA monitor finishes attaching `alsa.components` to the physical Arctis
-- node -- there's no further node event to hang a retry on at that point.
-- Each call below is an independent one-shot Core.timeout_add issued from
-- top-level script scope, not chained by having a firing callback
-- reschedule itself -- that specific pattern (used for the steady-state
-- poll in an earlier version of this script) reliably fired exactly once
-- and never again on this WirePlumber build (0.5.14), cause unresolved;
-- these plain one-shots did not show that problem in the same testing.
for _, delay_ms in ipairs({ 1000, 3000, 6000, 10000 }) do
  Core.timeout_add(delay_ms, apply_visibility)
end
