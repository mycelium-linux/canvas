-- Hello World: a minimal JARVIS-directed, Canvas-rendered briefing.
-- Edit this file in the runtime briefings directory; Canvas reloads it for
-- every beat, so visual iteration requires no Canvas rebuild or restart.

local beats = {
  [0] = {
    narrate = "Hello, Avery. This is a live briefing, presented by JARVIS and rendered by Canvas.",
    wait_seconds = 5.0,
    message = "Hello World — arrival",
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "tidepool", speed = 0.25 } },
      { op = "set_chrome", chrome = { title = "hidden", canvas_padding = 48, surface_gap = 24 } },
      { op = "set_focus", surface_id = "clock" },
    },
  },
  [1] = {
    narrate = "The visuals are controlled by this Lua artifact. The background, layout chrome, and focus can change without rebuilding Canvas.",
    wait_seconds = 7.0,
    message = "Hello World — Lua controls the scene",
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "aurora-drift", speed = 0.45 } },
      { op = "set_chrome", chrome = { title = "focused", canvas_padding = 32, surface_gap = 18 } },
      { op = "set_focus", surface_id = "status" },
    },
  },
  [2] = {
    narrate = "Narration was rendered ahead of time through Bifrost, cached here on the presenting edge, and synchronized with these visual beats.",
    wait_seconds = 7.0,
    message = "Hello World — edge-cached narration",
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "dusk-ember", speed = 0.35 } },
      { op = "set_focus", surface_id = "meter" },
    },
  },
  [3] = {
    narrate = "Hello, world. This is the smallest complete briefing. Next, we can make it beautiful.",
    wait_seconds = 6.0,
    message = "Hello World — complete",
    patches = {
      { op = "set_background", background = { kind = "shader", shader_id = "smoke-rings", speed = 0.6 } },
      { op = "set_chrome", chrome = { title = "focused", canvas_padding = 24, surface_gap = 16 } },
      { op = "set_focus", surface_id = nil },
    },
    done = true,
  },
}

local payload = event.payload
if type(payload) ~= "table" or type(payload.beat) ~= "number" then
  error("hello-world briefing requires payload.beat")
end
local index = math.floor(payload.beat)
local beat = beats[index]
if beat == nil then
  return {
    beat = index, narrate = nil, wait_seconds = 0,
    patches = {}, pre_actions = {}, post_actions = {}, done = true,
    message = "Hello World is complete.",
  }
end
return {
  beat = index,
  narrate = beat.narrate,
  wait_seconds = beat.wait_seconds,
  patches = beat.patches,
  pre_actions = {},
  post_actions = {},
  done = beat.done or false,
  message = beat.message,
}
