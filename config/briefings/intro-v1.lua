-- Intro briefing: first-launch narrative as paced beats with per-zone effects.
--
-- Pre-actions hide distracting desktop widgets (Conky, etc.) for the duration
-- of the briefing, scoped to the presenting zone. Post-actions restore them.
-- This is dock-4i5: per-zone pre/post briefing effects.
--
-- Contract: canvas.briefing.beat.v1
-- The producer (scripts/canvas-briefing-run.py) emits one envelope per beat.
-- This binder returns a full briefing definition (beats + pre/post actions),
-- not a single beat. The first event returns the whole briefing; subsequent
-- events advance the beat index.

local beats = {
  [0] = {
    narrate = "FPL login boots Canvas. The spinning logo and TV-open animation cover real startup latency, not fake polish.",
    wait_seconds = 3.0,
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "tidepool", speed = 0.3 } },
      { op = "set_chrome", chrome = { title = "hidden", canvas_padding = 0, surface_gap = 0 } },
    },
  },
  [1] = {
    narrate = "JARVIS boot animation establishes presence and identity.",
    wait_seconds = 4.0,
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "aurora-drift", speed = 0.4 } },
    },
  },
  [2] = {
    narrate = "Good afternoon, Avery. What would you like to work on today?",
    wait_seconds = 2.0,
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "dusk-ember", speed = 0.5 } },
      { op = "set_focus", surface_id = nil },
    },
  },
  [3] = {
    narrate = "Canvas is the shared visualization plane. F2 chooses backgrounds, F4 chooses themes.",
    wait_seconds = 5.0,
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "smoke-rings", speed = 0.6 } },
      { op = "set_chrome", chrome = { title = "focused", canvas_padding = 24, surface_gap = 16 } },
    },
  },
  [4] = {
    narrate = "JARVIS handles voice and presence. Toggle the overlay with Control-Option-J.",
    wait_seconds = 4.0,
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "slow-ripple", speed = 0.5 } },
    },
  },
  [5] = {
    narrate = "Alfalfa owns project identity and workflows. The demo is the real path.",
    wait_seconds = 3.0,
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "spirales", speed = 0.7 } },
    },
  },
}

local payload = event.payload
if type(payload) ~= "table" or type(payload.beat) ~= "number" then
  error("briefing beat event requires payload.beat as a number")
end

local index = math.floor(payload.beat)
local beat = beats[index]

if beat == nil then
  -- Final beat: apply post-actions (restore widgets)
  if index >= 6 then
    return {
      beat = index,
      narrate = nil,
      wait_seconds = 0.0,
      patches = {},
      pre_actions = {},
      post_actions = {
        { op = "set_mesh_node", node_id = "conky", hidden = false },
        { op = "set_mesh_node", node_id = "lockout-table", hidden = false },
        { op = "set_background", background = { kind = "theme" } },
      },
      done = true,
      message = "Briefing complete — widgets restored.",
    }
  end
  return {
    beat = index,
    narrate = nil,
    wait_seconds = 0.0,
    patches = {},
    pre_actions = {
      { op = "set_mesh_node", node_id = "conky", hidden = true },
      { op = "set_mesh_node", node_id = "lockout-table", hidden = true },
    },
    post_actions = {},
    done = false,
    message = string.format("beat %d is past the end", index),
  }
end

-- Beat 0 triggers pre-actions (hide widgets); beat 5 triggers post-actions
local pre = {}
local post = {}
if index == 0 then
  pre = {
    { op = "set_mesh_node", node_id = "conky", hidden = true },
    { op = "set_mesh_node", node_id = "lockout-table", hidden = true },
  }
end
if index >= 5 then
  post = {
    { op = "set_mesh_node", node_id = "conky", hidden = false },
    { op = "set_mesh_node", node_id = "lockout-table", hidden = false },
    { op = "set_background", background = { kind = "theme" } },
  }
  return {
    beat = index,
    narrate = beat.narrate,
    wait_seconds = beat.wait_seconds,
    patches = beat.patches,
    pre_actions = pre,
    post_actions = post,
    done = true,
  }
end

return {
  beat = index,
  narrate = beat.narrate,
  wait_seconds = beat.wait_seconds,
  patches = beat.patches,
  pre_actions = pre,
  post_actions = post,
  done = false,
}
