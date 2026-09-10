-- The machine-wide build queue, formatted for a wall.
--
-- The producer sends `lockout status --json` verbatim; everything a reader
-- needs done to it happens here, so changing the formatting is editing a file
-- rather than rebuilding Canvas.
--
-- Three things this does that `watch lockout status` cannot:
--   * shows WHICH REPO each job belongs to, from cwd - the field the text
--     output never prints, and the reason half the queue looks like yours
--   * unescapes and trims the command, so a shell-quoted cargo invocation stops
--     eating the width it needs to wrap
--   * says how many jobs are waiting on a build dir, which is the usual reason
--     a queue looks stuck

local status = event.payload
if type(status) ~= "table" or type(status.jobs) ~= "table" then
  error("lockout binder requires a status payload with jobs")
end

local MAX_ROWS = 10
local COMMAND_WIDTH = 44

-- Readable across a room. "51:00" is precise and unreadable at a glance; "51m"
-- is neither less true nor less useful.
local function duration(seconds)
  if type(seconds) ~= "number" or seconds < 0 then return "-" end
  if seconds < 90 then return string.format("%ds", seconds) end
  local minutes = math.floor(seconds / 60)
  if minutes < 90 then return string.format("%dm", minutes) end
  return string.format("%dh", math.floor(minutes / 60))
end

-- Derived from cwd rather than configured, so a new checkout needs no change.
local function repo(cwd)
  if type(cwd) ~= "string" then return "?" end
  local name = cwd:match("([^/]+)/?$")
  return name or "?"
end

local function command(raw)
  if type(raw) ~= "string" then return "" end
  -- Undo shell escaping, collapse whitespace.
  local text = raw:gsub("\\ ", " "):gsub("\\", " "):gsub("%s+", " ")
  text = text:gsub("^%s+", ""):gsub("%s+$", "")
  -- `sh -c` and `cargo` are on almost every row, so they carry no information.
  text = text:gsub("^sh %-c ", ""):gsub("^cargo ", "")
  text = text:gsub("^/[%w%p]*/cargo ", "")
  if #text > COMMAND_WIDTH then
    return text:sub(1, COMMAND_WIDTH - 1) .. "…"
  end
  return text
end

-- Running first, then longest waiting: what holds a slot matters more than what
-- is waiting for one, and the queue is usually longer than the frame.
local jobs = {}
for index = 1, #status.jobs do jobs[index] = status.jobs[index] end
table.sort(jobs, function(a, b)
  local a_running = a.state == "running"
  local b_running = b.state == "running"
  if a_running ~= b_running then return a_running end
  return (a.waited or 0) > (b.waited or 0)
end)

local rows = {}
local blocked = 0
for index = 1, #jobs do
  local job = jobs[index]
  if type(job.note) == "string" and job.note:find("waiting on build dir", 1, true) then
    blocked = blocked + 1
  end
  if #rows < MAX_ROWS then
    rows[#rows + 1] = {
      job.state or "?",
      repo(job.cwd),
      command(job.command),
      duration(job.waited),
      duration(job.ran),
    }
  end
end

-- The producer caps how many jobs fit one datagram, so trust its `total` over
-- what arrived. Otherwise a queue trimmed for the wire would report itself as
-- shorter than it is, which is the one thing this title exists to prevent.
local total = status.total or #jobs
local title
if total > #rows then
  -- Never let the visible row count read as the whole queue.
  title = string.format("Build queue (%d of %d, limit %d)", #rows, total, status.limit or 0)
else
  title = string.format("Build queue (%d running, %d queued)", status.running or 0, status.queued or 0)
end

local detail = nil
local state = "ready"
if total == 0 then
  state = "no_data"
  detail = "Nothing building"
elseif blocked > 0 then
  detail = string.format("%d waiting on a build dir", blocked)
end

return {
  kind = "snapshot",
  -- Only the KIND and its own fields. The snapshot replaces the widget kind
  -- inside the surface's existing spec, so identity - schema_version, id, theme,
  -- chrome - belongs to the surface and must not be resent here.
  widget = {
    kind = "table",
    title = title,
    columns = { "state", "repo", "command", "wait", "run" },
    rows = rows,
  },
  data = {
    state = state,
    source = "lockout status --json",
    detail = detail,
    provenance = "live",
  },
}
