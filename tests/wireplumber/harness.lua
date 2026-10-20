-- Loads sink-visibility.lua against a stubbed WirePlumber Lua API.
-- usage: lua harness.lua <script> <variant>   (variant: topic | legacy)
-- Prints every log line the script emitted as "<level>|<message>".
local script, variant = arg[1], arg[2]

local function emit(level, msg) print(level .. "|" .. msg) end

local Log
if variant == "topic" then
  -- WirePlumber >= 0.4.81 / 0.5: topic loggers.
  Log = {
    open_topic = function()
      return {
        info = function(_, m) emit("info", m) end,
        warning = function(_, m) emit("warning", m) end,
      }
    end,
  }
else
  -- WirePlumber 0.4.x (e.g. 0.4.17): no open_topic; Log.info(message).
  -- Strict like the real binding: a non-string first argument is an error.
  local function strict(level)
    return function(m)
      assert(type(m) == "string", "bad argument #1 (string expected)")
      emit(level, m)
    end
  end
  Log = { info = strict("info"), warning = strict("warning") }
end

local function om()
  return {
    connect = function() end,
    activate = function() end,
    iterate = function() return function() return nil end end,
  }
end

local env = {
  Log = Log,
  ObjectManager = om,
  Interest = function(t) return t end,
  Constraint = function(t) return t end,
  GLib = { access = function() return false end },
  Plugin = { find = function() return nil end },
  Core = { timeout_add = function() end },
}
setmetatable(env, { __index = _G })

local chunk = assert(loadfile(script, "t", env))
chunk()
