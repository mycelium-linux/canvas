#!/usr/bin/env lua
-- Dock control plane: the tmux fleet as JSON, plus per-pane actions.
--
-- Phase 1 of docs/CONTROL-PLANE.md. tmux owns panes; `pi-tmux` owns worker name
-- resolution and keystroke semantics. This script is a stateless adapter that
-- enumerates panes and forwards four actions, so nothing about pane identity or
-- input is re-implemented here.
--
-- Routes (loopback only by default):
--
--   GET  /api/panes                       fleet snapshot
--   POST /api/pane/{name}/send            {"message": "...", "submit": true}
--   POST /api/pane/{name}/kill            terminate the pane or window
--   GET  /api/pane/{name}/capture?lines=N visible text, oldest first
--
-- Requires Lua 5.4 and luasocket (the only socket binding Lua has):
--   macOS:      brew install lua
--   Debian/Ubuntu: apt-get install -y lua5.4 lua-socket
--
-- Usage:
--   scripts/cp-server.lua [--port 8791] [--host 127.0.0.1] [--session NAME]
--
-- Environment: CP_PORT, CP_HOST, CP_SESSION, PI_TMUX_SESSION, PI_TMUX_PREFIX,
-- PI_TMUX_FLEET, PI_TMUX_STATE, PI_TMUX_BIN.

local socket = require("socket")

local DEFAULT_PORT = 8791
local DEFAULT_HOST = "127.0.0.1"
local DEFAULT_LINES = 80
local MAX_LINES = 2000
local MAX_BODY = 1024 * 1024

local function die(message, code)
  io.stderr:write("cp-server: " .. message .. "\n")
  os.exit(code or 2)
end

-- ---------------------------------------------------------------------------
-- stdlib-only JSON
-- ---------------------------------------------------------------------------

local json = {}

local ARRAY = {}

--- Mark a table as a JSON array (needed for empty arrays).
function json.array(value)
  return setmetatable(value or {}, ARRAY)
end

local escapes = {
  ['"'] = '\\"',
  ["\\"] = "\\\\",
  ["\b"] = "\\b",
  ["\f"] = "\\f",
  ["\n"] = "\\n",
  ["\r"] = "\\r",
  ["\t"] = "\\t",
}

local function escape_string(value)
  return (value:gsub('[%z\1-\31\\"]', function(char)
    return escapes[char] or string.format("\\u%04x", char:byte())
  end))
end

local function is_array(value)
  if getmetatable(value) == ARRAY then
    return true
  end
  local count = 0
  for key in pairs(value) do
    if type(key) ~= "number" then
      return false
    end
    count = count + 1
  end
  return count > 0, count
end

local encode

local function encode_table(value, buffer)
  local array, count = is_array(value)
  if not array then
    buffer[#buffer + 1] = "{"
    local first = true
    local keys = {}
    for key in pairs(value) do
      keys[#keys + 1] = key
    end
    table.sort(keys, function(a, b)
      return tostring(a) < tostring(b)
    end)
    for _, key in ipairs(keys) do
      if not first then
        buffer[#buffer + 1] = ","
      end
      first = false
      buffer[#buffer + 1] = '"' .. escape_string(tostring(key)) .. '":'
      encode(value[key], buffer)
    end
    buffer[#buffer + 1] = "}"
    return
  end

  buffer[#buffer + 1] = "["
  for index = 1, count or #value do
    if index > 1 then
      buffer[#buffer + 1] = ","
    end
    encode(value[index], buffer)
  end
  buffer[#buffer + 1] = "]"
end

encode = function(value, buffer)
  local kind = type(value)
  if value == nil then
    buffer[#buffer + 1] = "null"
  elseif kind == "boolean" then
    buffer[#buffer + 1] = value and "true" or "false"
  elseif kind == "number" then
    if value ~= value or value == math.huge or value == -math.huge then
      buffer[#buffer + 1] = "null"
    elseif math.type(value) == "integer" or value == math.floor(value) then
      buffer[#buffer + 1] = string.format("%d", value)
    else
      buffer[#buffer + 1] = string.format("%.14g", value)
    end
  elseif kind == "string" then
    buffer[#buffer + 1] = '"' .. escape_string(value) .. '"'
  elseif kind == "table" then
    encode_table(value, buffer)
  else
    buffer[#buffer + 1] = "null"
  end
end

function json.encode(value)
  local buffer = {}
  encode(value, buffer)
  return table.concat(buffer)
end

local function decode_error(text, position, message)
  error(string.format("invalid JSON at byte %d: %s", position, message), 0)
end

local function skip_space(text, position)
  local _, stop = text:find("^[ \t\r\n]*", position)
  return stop + 1
end

local decode_value

local function decode_string(text, position)
  local out = {}
  position = position + 1 -- opening quote
  while true do
    local char = text:sub(position, position)
    if char == "" then
      decode_error(text, position, "unterminated string")
    elseif char == '"' then
      return table.concat(out), position + 1
    elseif char == "\\" then
      local next_char = text:sub(position + 1, position + 1)
      local simple = { ['"'] = '"', ["\\"] = "\\", ["/"] = "/", b = "\b", f = "\f", n = "\n", r = "\r", t = "\t" }
      if simple[next_char] then
        out[#out + 1] = simple[next_char]
        position = position + 2
      elseif next_char == "u" then
        local hex = text:sub(position + 2, position + 5)
        local code = tonumber(hex, 16)
        if not code then
          decode_error(text, position, "bad \\u escape")
        end
        if code >= 0xD800 and code <= 0xDBFF then
          local low = tonumber(text:sub(position + 8, position + 11), 16)
          if low and low >= 0xDC00 and low <= 0xDFFF then
            code = 0x10000 + (code - 0xD800) * 0x400 + (low - 0xDC00)
            position = position + 6
          end
        end
        out[#out + 1] = utf8.char(code)
        position = position + 6
      else
        decode_error(text, position, "bad escape")
      end
    else
      out[#out + 1] = char
      position = position + 1
    end
  end
end

decode_value = function(text, position)
  position = skip_space(text, position)
  local char = text:sub(position, position)
  if char == "{" then
    local object = {}
    position = skip_space(text, position + 1)
    if text:sub(position, position) == "}" then
      return object, position + 1
    end
    while true do
      position = skip_space(text, position)
      if text:sub(position, position) ~= '"' then
        decode_error(text, position, "expected object key")
      end
      local key
      key, position = decode_string(text, position)
      position = skip_space(text, position)
      if text:sub(position, position) ~= ":" then
        decode_error(text, position, "expected ':'")
      end
      local value
      value, position = decode_value(text, position + 1)
      object[key] = value
      position = skip_space(text, position)
      local separator = text:sub(position, position)
      if separator == "}" then
        return object, position + 1
      elseif separator ~= "," then
        decode_error(text, position, "expected ',' or '}'")
      end
      position = position + 1
    end
  elseif char == "[" then
    local array = json.array()
    position = skip_space(text, position + 1)
    if text:sub(position, position) == "]" then
      return array, position + 1
    end
    while true do
      local value
      value, position = decode_value(text, position)
      array[#array + 1] = value
      position = skip_space(text, position)
      local separator = text:sub(position, position)
      if separator == "]" then
        return array, position + 1
      elseif separator ~= "," then
        decode_error(text, position, "expected ',' or ']'")
      end
      position = position + 1
    end
  elseif char == '"' then
    return decode_string(text, position)
  elseif text:sub(position, position + 3) == "true" then
    return true, position + 4
  elseif text:sub(position, position + 4) == "false" then
    return false, position + 5
  elseif text:sub(position, position + 3) == "null" then
    return nil, position + 4
  elseif char == "" then
    decode_error(text, position, "unexpected end of input")
  end

  local number = text:match("^-?%d+%.?%d*[eE]?[-+]?%d*", position)
  if not number or number == "" then
    decode_error(text, position, "unexpected character")
  end
  local value = tonumber(number)
  if not value then
    decode_error(text, position, "bad number")
  end
  return value, position + #number
end

function json.decode(text)
  local value, position = decode_value(text, 1)
  position = skip_space(text, position)
  if position <= #text then
    decode_error(text, position, "trailing data")
  end
  return value
end

-- ---------------------------------------------------------------------------
-- process execution (no shell interpolation of arguments)
-- ---------------------------------------------------------------------------

local function quote(value)
  return "'" .. tostring(value):gsub("'", "'\\''") .. "'"
end

local function remove(path)
  os.remove(path)
end

--- Run argv without a shell reinterpreting any element.
--- Returns { code = <exit status>, out = <stdout>, err = <stderr> }.
local function run(argv)
  local out_file, err_file, code_file = os.tmpname(), os.tmpname(), os.tmpname()
  local parts = {}
  for index, arg in ipairs(argv) do
    parts[index] = quote(arg)
  end
  local command = table.concat(parts, " ")
    .. " >" .. quote(out_file)
    .. " 2>" .. quote(err_file)
    .. "; printf '%s' $? > " .. quote(code_file)

  os.execute(command)

  local function slurp(path)
    local handle = io.open(path, "rb")
    if not handle then
      return ""
    end
    local content = handle:read("a") or ""
    handle:close()
    return content
  end

  local result = {
    code = tonumber(slurp(code_file)) or 1,
    out = slurp(out_file),
    err = slurp(err_file),
  }
  remove(out_file)
  remove(err_file)
  remove(code_file)
  return result
end

local function trim(text)
  return (text:gsub("^%s+", ""):gsub("%s+$", ""))
end

-- ---------------------------------------------------------------------------
-- fleet
-- ---------------------------------------------------------------------------

local Fleet = {}
Fleet.__index = Fleet

function Fleet.new(options)
  return setmetatable({
    session = options.session,
    prefix = options.prefix,
    fleet_window = options.fleet_window,
    state_file = options.state_file,
    pi_tmux = options.pi_tmux,
    verbose = options.verbose,
  }, Fleet)
end

--- worker name -> { kind, target } from the pi-tmux state file.
function Fleet:state_names()
  local entries = {}
  local handle = io.open(self.state_file, "r")
  if not handle then
    return entries
  end
  for line in handle:lines() do
    local name, kind, target = line:match("^([^\t]+)\t([^\t]+)\t([^\t]+)$")
    if name then
      entries[name] = { kind = kind, target = target }
    end
  end
  handle:close()
  return entries
end

function Fleet:tmux(argv)
  local full = { "tmux" }
  for index, arg in ipairs(argv) do
    full[index + 1] = arg
  end
  return run(full)
end

function Fleet:pi(argv)
  local full = { self.pi_tmux }
  for index, arg in ipairs(argv) do
    full[index + 1] = arg
  end
  return run(full)
end

function Fleet:strip_prefix(label)
  if not label then
    return nil
  end
  local prefix = self.prefix .. "-"
  if label:sub(1, #prefix) == prefix then
    local stripped = label:sub(#prefix + 1)
    if stripped ~= "" then
      return stripped
    end
  end
  return nil
end

local FORMAT = table.concat({
  "#{window_index}",
  "#{window_name}",
  "#{pane_id}",
  "#{pane_title}",
  "#{pane_current_command}",
  "#{pane_current_path}",
  "#{pane_active}",
  "#{pane_width}",
  "#{pane_height}",
}, "\t")

function Fleet:list_panes()
  local result = self:tmux({ "list-panes", "-s", "-t", self.session, "-F", FORMAT })
  if result.code ~= 0 then
    error({ status = 502, message = "tmux list-panes failed: " .. trim(result.err ~= "" and result.err or result.out) })
  end

  local state = self:state_names()
  local by_target, by_window = {}, {}
  for name, entry in pairs(state) do
    by_target[entry.target] = name
    if entry.kind == "window" then
      by_window[entry.target:match("([^:]+)$")] = name
    end
  end

  local raw = {}
  for line in result.out:gmatch("[^\n]+") do
    local window_index, window, pane_id, title, command, path, active, width, height =
      line:match("^(%d+)\t([^\t]*)\t([^\t]+)\t([^\t]*)\t([^\t]*)\t([^\t]*)\t([01])\t(%d+)\t(%d+)$")
    if pane_id then
      raw[#raw + 1] = {
        window_index = tonumber(window_index),
        window = window,
        pane_id = pane_id,
        title = title,
        command = command,
        path = path,
        active = active == "1",
        width = tonumber(width),
        height = tonumber(height),
      }
    end
  end
  table.sort(raw, function(left, right)
    if left.window_index ~= right.window_index then
      return left.window_index < right.window_index
    end
    return left.pane_id < right.pane_id
  end)

  local panes, used = json.array(), {}
  for _, pane in ipairs(raw) do
    -- Dense list on purpose: a leading nil would stop ipairs before a
    -- later, still-valid candidate was ever considered.
    local candidates = {}
    local function consider(value)
      if value and value ~= "" then
        candidates[#candidates + 1] = value
      end
    end
    consider(by_target[pane.pane_id])
    consider(by_window[pane.window])
    consider(self:strip_prefix(pane.title))
    consider(self:strip_prefix(pane.window))
    consider(pane.window)

    local name
    for _, candidate in ipairs(candidates) do
      if not used[candidate] then
        name = candidate
        break
      end
    end
    if not name then
      local base = self:strip_prefix(pane.window) or pane.pane_id
      name = base .. "-" .. (pane.pane_id:gsub("%%", ""))
    end
    used[name] = true

    local lower_command = pane.command:lower()
    local worker = pane.window:sub(1, #self.prefix + 1) == self.prefix .. "-"
      or pane.title:sub(1, #self.prefix + 1) == self.prefix .. "-"
      or lower_command == "node"
      or lower_command == "pi"
      or lower_command:find("pi", 1, true) ~= nil

    panes[#panes + 1] = {
      name = name,
      pane_id = pane.pane_id,
      window = pane.window,
      window_index = pane.window_index,
      title = pane.title,
      command = pane.command,
      path = pane.path,
      active = pane.active,
      width = pane.width,
      height = pane.height,
      worker = worker,
      fleet_window = pane.window == self.fleet_window,
    }
  end
  return panes
end

function Fleet:snapshot()
  local panes = self:list_panes()
  return {
    session = self.session,
    fleet_window = self.fleet_window,
    generated_at = os.date("!%Y-%m-%dT%H:%M:%SZ"),
    count = #panes,
    panes = panes,
  }
end

function Fleet:alive(target)
  local result = self:tmux({ "display-message", "-p", "-t", target, "#{pane_id}" })
  return result.code == 0
end

--- name -> tmux target. Unknown names fall through to pi-tmux's own lookup.
function Fleet:resolve(name)
  if not name:match("^[%w][%w._-]*$") or #name > 64 then
    error({ status = 400, message = "invalid pane name: " .. name })
  end
  local entry = self:state_names()[name]
  if entry then
    if self:alive(entry.target) then
      return entry.target
    end
    error({ status = 404, message = "pane " .. name .. " is stale: " .. entry.target .. " is gone" })
  end
  for _, pane in ipairs(self:list_panes()) do
    if pane.name == name then
      if self:alive(pane.pane_id) then
        return pane.pane_id
      end
      error({ status = 404, message = "pane " .. name .. " is gone" })
    end
  end
  -- Not ours to resolve: let the owner try its own fleet/window lookup.
  local probe = self:pi({ "capture", "--target", name, "--lines", "1" })
  if probe.code == 0 then
    return name
  end
  error({ status = 404, message = "unknown pane: " .. name })
end

function Fleet:send(name, message, submit)
  if type(message) ~= "string" then
    error({ status = 400, message = "message must be a string" })
  end
  if message == "" then
    error({ status = 400, message = "message must not be empty" })
  end
  if #message > MAX_BODY then
    error({ status = 413, message = "message exceeds " .. MAX_BODY .. " characters" })
  end
  local target = self:resolve(name)
  local argv = { "send", "--target", target, "--message", message }
  if not submit then
    argv[#argv + 1] = "--no-submit"
  end
  local result = self:pi(argv)
  if result.code ~= 0 then
    error({ status = 502, message = "pi-tmux send failed: " .. trim(result.err ~= "" and result.err or result.out) })
  end
  return {
    ok = true,
    name = name,
    target = target,
    chars = #message,
    submitted = submit,
  }
end

function Fleet:capture(name, lines)
  local target = self:resolve(name)
  local result = self:pi({ "capture", "--target", target, "--lines", tostring(lines) })
  if result.code ~= 0 then
    error({ status = 502, message = "pi-tmux capture failed: " .. trim(result.err ~= "" and result.err or result.out) })
  end
  return {
    ok = true,
    name = name,
    target = target,
    lines = lines,
    text = result.out,
  }
end

function Fleet:kill(name)
  local target = self:resolve(name)
  -- pi-tmux refuses `kill fleet`; a window target that *is* the fleet window
  -- must be refused here too, because the fleet is a container, not a worker.
  if target:sub(-#self.fleet_window - 1) == ":" .. self.fleet_window then
    error({ status = 409, message = "refusing to kill the fleet window; kill workers by name" })
  end
  local result = self:pi({ "kill", target })
  if result.code ~= 0 then
    error({ status = 502, message = "pi-tmux kill failed: " .. trim(result.err ~= "" and result.err or result.out) })
  end
  return {
    ok = true,
    name = name,
    target = target,
    killed = trim(result.out),
  }
end

-- ---------------------------------------------------------------------------
-- discovery / configuration
-- ---------------------------------------------------------------------------

local function env(name)
  local value = os.getenv(name)
  if value and value ~= "" then
    return value
  end
  return nil
end

local function tmux_sessions()
  local result = run({ "tmux", "list-sessions", "-F", "#{session_name}" })
  local sessions = {}
  if result.code ~= 0 then
    return sessions
  end
  for line in result.out:gmatch("[^\n]+") do
    sessions[#sessions + 1] = line
  end
  return sessions
end

local function detect_session(fleet_window)
  local configured = env("CP_SESSION") or env("PI_TMUX_SESSION")
  if configured then
    return configured, "environment"
  end

  if env("TMUX") then
    local result = run({ "tmux", "display-message", "-p", "#S" })
    if result.code == 0 then
      local session = trim(result.out)
      if session ~= "" then
        return session, "current tmux client"
      end
    end
  end

  local listing = run({ "tmux", "list-windows", "-a", "-F", "#{session_name}:#{window_name}" })
  if listing.code == 0 then
    local owners, seen = {}, {}
    for line in listing.out:gmatch("[^\n]+") do
      local session = line:match("^(.-):" .. fleet_window .. "$")
      if session and not seen[session] then
        seen[session] = true
        owners[#owners + 1] = session
      end
    end
    table.sort(owners)
    if #owners == 1 then
      return owners[1], "session owning " .. fleet_window
    elseif #owners > 1 then
      die("several sessions own " .. fleet_window .. " (" .. table.concat(owners, ", ")
        .. "); pass --session NAME")
    end
    local sessions = tmux_sessions()
    if #sessions == 1 then
      return sessions[1], "only tmux session"
    elseif #sessions > 1 then
      die("cannot choose a session (" .. table.concat(sessions, ", ")
        .. "); pass --session NAME or set CP_SESSION")
    end
  end
  die("no tmux server or no sessions; start a tmux fleet first or pass --session NAME")
end

local function resolve_pi_tmux()
  local explicit = env("PI_TMUX_BIN")
  if explicit then
    if not io.open(explicit, "r") then
      die("PI_TMUX_BIN is not readable: " .. explicit)
    end
    return explicit
  end
  local result = run({ "sh", "-c", "command -v pi-tmux" })
  local found = trim(result.out)
  if result.code == 0 and found ~= "" then
    return found
  end
  local home = env("HOME") or ""
  local fallback = home .. "/.pi/agent/bin/pi-tmux"
  if io.open(fallback, "r") then
    return fallback
  end
  die("pi-tmux not found; install it or set PI_TMUX_BIN")
end

local function usage()
  print([[
usage: cp-server.lua [options]

  --port N            TCP port (default 8791, or CP_PORT)
  --host ADDR         bind address (default 127.0.0.1, or CP_HOST)
  --session NAME      tmux session to expose (default: detected)
  --prefix NAME       worker name prefix (default pi, or PI_TMUX_PREFIX)
  --fleet-window NAME tiled fleet window (default pi-fleet, or PI_TMUX_FLEET)
  --state-file PATH   pi-tmux worker state (default ~/.pi/agent/pi-tmux.state)
  --verbose           log every request
  -h, --help          this text

See docs/CONTROL-PLANE.md.]])
end

local function parse_args(argv)
  local options = {
    port = tonumber(env("CP_PORT") or DEFAULT_PORT),
    host = env("CP_HOST") or DEFAULT_HOST,
    session = env("CP_SESSION") or env("PI_TMUX_SESSION"),
    prefix = env("PI_TMUX_PREFIX") or "pi",
    fleet_window = env("PI_TMUX_FLEET") or "pi-fleet",
    state_file = env("PI_TMUX_STATE")
      or ((env("HOME") or "") .. "/.pi/agent/pi-tmux.state"),
    verbose = false,
  }
  local index = 1
  while index <= #argv do
    local arg = argv[index]
    local function value()
      index = index + 1
      if index > #argv then
        die("missing value for " .. arg)
      end
      return argv[index]
    end
    if arg == "--port" then
      options.port = tonumber(value())
    elseif arg == "--host" then
      options.host = value()
    elseif arg == "--session" then
      options.session = value()
    elseif arg == "--prefix" then
      options.prefix = value()
    elseif arg == "--fleet-window" then
      options.fleet_window = value()
    elseif arg == "--state-file" then
      options.state_file = value()
    elseif arg == "--verbose" then
      options.verbose = true
    elseif arg == "-h" or arg == "--help" then
      usage()
      os.exit(0)
    else
      die("unknown argument: " .. arg)
    end
    index = index + 1
  end
  if not options.port then
    die("port must be a number")
  end
  return options
end

-- ---------------------------------------------------------------------------
-- HTTP
-- ---------------------------------------------------------------------------

local function url_decode(text)
  return (text:gsub("%%(%x%x)", function(hex)
    return string.char(tonumber(hex, 16))
  end):gsub("+", " "))
end

local function url_encode(text)
  return (text:gsub("[^%w%-%._~]", function(char)
    return string.format("%%%02X", char:byte())
  end))
end

local function parse_query(query)
  local values = {}
  for pair in (query or ""):gmatch("[^&]+") do
    local key, value = pair:match("^([^=]*)=(.*)$")
    if not key then
      key, value = pair, ""
    end
    values[url_decode(key)] = url_decode(value)
  end
  return values
end

local function truthy(value)
  if not value then
    return false
  end
  value = value:lower()
  return value == "1" or value == "true" or value == "yes" or value == "on"
end

local function respond(client, status, payload)
  local reasons = {
    [200] = "OK",
    [400] = "Bad Request",
    [404] = "Not Found",
    [405] = "Method Not Allowed",
    [409] = "Conflict",
    [413] = "Payload Too Large",
    [500] = "Internal Server Error",
    [502] = "Bad Gateway",
  }
  local body = json.encode(payload)
  local head = table.concat({
    string.format("HTTP/1.1 %d %s", status, reasons[status] or "Error"),
    "Content-Type: application/json; charset=utf-8",
    "Content-Length: " .. #body,
    "Cache-Control: no-store",
    "Connection: close",
    "", "",
  }, "\r\n")
  client:send(head .. body)
end

local function fail(client, status, message)
  respond(client, status, { ok = false, error = message })
end

local function serve_request(fleet, client, request_line, headers)
  local method, target = request_line:match("^(%u+) (%S+) HTTP/%d%.%d$")
  if not method then
    return fail(client, 400, "malformed request line")
  end
  local path, query = target:match("^([^?]*)%??(.*)$")
  local parts = {}
  for part in path:gmatch("[^/]+") do
    parts[#parts + 1] = url_decode(part)
  end
  local params = parse_query(query)

  -- A path that exists in the route table but was called with another
  -- method is 405; anything else is 404. Keep the two cases distinct so an
  -- unknown path never claims to be a known route.
  local known_actions = { capture = true, send = true, kill = true }
  local function known_route(route_parts)
    if route_parts[1] ~= "api" then
      return false
    end
    if #route_parts == 2 and route_parts[2] == "panes" then
      return true
    end
    if #route_parts == 4 and route_parts[2] == "pane" and known_actions[route_parts[4]] then
      return true
    end
    return false
  end

  local results = table.pack(pcall(function()
    if method == "GET" and #parts == 2 and parts[1] == "api" and parts[2] == "panes" then
      local snapshot = fleet:snapshot()
      if truthy(params.workers) then
        local workers = json.array()
        for _, pane in ipairs(snapshot.panes) do
          if pane.worker then
            workers[#workers + 1] = pane
          end
        end
        snapshot.panes = workers
        snapshot.count = #workers
      end
      return 200, snapshot
    end

    if #parts == 4 and parts[1] == "api" and parts[2] == "pane" then
      local name, action = parts[3], parts[4]
      if method == "GET" and action == "capture" then
        local raw = params.lines or tostring(DEFAULT_LINES)
        local lines = tonumber(raw)
        if not lines or lines ~= math.floor(lines) then
          error({ status = 400, message = "lines must be an integer, got " .. raw })
        end
        if lines < 1 or lines > MAX_LINES then
          error({ status = 400, message = "lines must be between 1 and " .. MAX_LINES })
        end
        return 200, fleet:capture(name, lines)
      elseif method == "POST" and action == "send" then
        local submit = fleet.body.submit
        if submit == nil then
          submit = true
        elseif type(submit) ~= "boolean" then
          error({ status = 400, message = "submit must be a boolean" })
        end
        return 200, fleet:send(name, fleet.body.message, submit)
      elseif method == "POST" and action == "kill" then
        return 200, fleet:kill(name)
      end
    end

    if known_route(parts) then
      error({ status = 405, message = "no such route for " .. method })
    end
    error({ status = 404, message = "not found" })
  end))

  if results[1] then
    return respond(client, results[2], results[3])
  end
  local outcome = results[2]
  if type(outcome) == "table" and outcome.status then
    return fail(client, outcome.status, outcome.message)
  end
  return fail(client, 500, tostring(outcome))
end

local function read_headers(client)
  local headers, line = {}, nil
  repeat
    line = client:receive("*l")
    if not line then
      return headers
    end
    local key, value = line:match("^([%w-]+):%s*(.*)$")
    if key then
      headers[key:lower()] = value
    end
  until line == ""
  return headers
end

local function handle(fleet, client)
  client:settimeout(30)
  local request_line = client:receive("*l")
  if not request_line then
    client:close()
    return
  end
  -- Read headers (always) so the connection stays in sync before we answer.
  local headers = read_headers(client)

  local method, target = request_line:match("^(%u+) (%S+) HTTP/%d%.%d$")
  local length = tonumber(headers["content-length"] or "0") or 0
  local body = ""
  fleet.body = {}
  if length > 0 then
    if length > MAX_BODY then
      fail(client, 413, "body exceeds " .. MAX_BODY .. " bytes")
      client:close()
      return
    end
    body = client:receive(length) or ""
  end

  if method == "POST" then
    if body == "" then
      fleet.body = {}
    else
      local decoded_ok, decoded = pcall(json.decode, body)
      if not decoded_ok then
        fail(client, 400, "invalid JSON body: " .. tostring(decoded))
        client:close()
        return
      end
      if type(decoded) ~= "table" or getmetatable(decoded) == ARRAY then
        fail(client, 400, "body must be a JSON object")
        client:close()
        return
      end
      fleet.body = decoded
    end
  end

  if fleet.verbose then
    io.stderr:write(string.format("cp-server: %s %s\n", method or "?", target or "?"))
  end

  serve_request(fleet, client, request_line, headers)
  client:close()
end

-- ---------------------------------------------------------------------------
-- main
-- ---------------------------------------------------------------------------

local options = parse_args(arg)

if options.host ~= "127.0.0.1" and options.host ~= "::1" and options.host ~= "localhost" then
  io.stderr:write("cp-server: warning: binding " .. options.host
    .. "; this API can type into shells and belongs on loopback or behind an authenticated tunnel\n")
end

if not options.session then
  local session, source = detect_session(options.fleet_window)
  options.session = session
  io.stderr:write("cp-server: session " .. session .. " (" .. source .. ")\n")
end

local fleet = Fleet.new({
  session = options.session,
  prefix = options.prefix,
  fleet_window = options.fleet_window,
  state_file = options.state_file,
  pi_tmux = resolve_pi_tmux(),
  verbose = options.verbose,
})

local snapshot_ok, snapshot = pcall(function()
  return fleet:snapshot()
end)
if not snapshot_ok then
  die(tostring(snapshot.message or snapshot))
end
if snapshot.count == 0 then
  die("session " .. options.session .. " has no panes")
end

local server, bind_error = socket.bind(options.host, options.port)
if not server then
  if bind_error and bind_error:lower():find("in use", 1, true) then
    die("cannot bind " .. options.host .. ":" .. options.port .. ": address in use\n"
      .. "  On the Mac, Alfalfa's local service already owns 8791; run with --port 8795\n"
      .. "  (or CP_PORT) and point clients at that port.")
  end
  die("cannot bind " .. options.host .. ":" .. options.port .. ": " .. tostring(bind_error))
end
server:settimeout(nil)

-- Port 0 asks the OS for a free port; report the one we actually got so a
-- scratch server (and its selftest) can connect without guessing.
local _, bound_port = server:getsockname()
bound_port = tonumber(bound_port) or options.port

io.stderr:write(string.format("cp-server: session=%s panes=%d listening on http://%s:%d\n",
  options.session, snapshot.count, options.host, bound_port))

while true do
  local client = server:accept()
  if client then
    -- One bad request must not take the fleet view down with it.
    local ok, err = pcall(handle, fleet, client)
    if not ok then
      io.stderr:write("cp-server: request failed: " .. tostring(err) .. "\n")
    end
    client:close()
  end
end
