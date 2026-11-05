-- Logical pixels are the layout vocabulary; scene coordinates stay internal.
function widget.pixel_space(width, height)
  if width <= 0 or height <= 0 then error("pixel space needs positive dimensions") end
  return {
    x = function(px) return -4 + 8 * px / width end,
    y = function(px) return 4 - 8 * px / height end,
    w = function(px) return 8 * px / width end,
    h = function(px) return 8 * px / height end,
  }
end

function widget.icon_button(spec)
  spec.icon = spec.icon or "window"
  spec.icon_size = spec.icon_size or 28
  spec.appearance = spec.appearance or "quiet"
  widget.button(spec)
end

local function required(spec, key)
  local value = spec[key]
  if value == nil then error("widget primitive requires " .. key, 3) end
  return value
end

local function point(spec)
  return { spec.x or 0, spec.y or 0, spec.z or 0.2 }
end

local function text_scale(px)
  local unit = module.theme.body * 1.25
  return { px / unit, px / unit, 1 }
end

-- A rectangle. It speaks scene units unless given a `space`, in which case it
-- speaks the logical pixels every author reasons in -- the same rule the higher
-- components follow. Without this, passing a `space` was silently ignored and a
-- panel authored in pixels landed a screen away.
function widget.panel(spec)
  local width, height = required(spec, "width"), required(spec, "height")
  local x, y = spec.x or 0, spec.y or 0
  if spec.space then
    x, y = spec.space.x(x), spec.space.y(y)
    width, height = spec.space.w(width), spec.space.h(height)
  end
  canvas.rect {
    id = required(spec, "id"), parent = spec.parent, rgb = spec.rgb,
    transition_ms = spec.transition_ms,
    ink = spec.ink or "text", opacity = spec.opacity or 0.055,
    radius = spec.radius or module.theme.radius,
    translation = { x, y, spec.z or 0.2 },
    scale = { width, height, 1 },
  }
end

function widget.label(spec)
  canvas.text {
    id = required(spec, "id"), parent = spec.parent,
    text = tostring(spec.text or "—"), ink = spec.ink or "text",
    translation = point(spec),
    scale = text_scale(spec.size or module.theme.body),
  }
end

function widget.badge(spec)
  local id = required(spec, "id")
  widget.panel {
    id = id .. "-bg", parent = spec.parent,
    x = spec.x, y = spec.y, z = spec.z or 0.1,
    width = required(spec, "width"), height = required(spec, "height"),
    ink = spec.ink or "primary", opacity = spec.opacity or 0.18,
    radius = spec.radius or module.theme.radius,
  }
  widget.label {
    id = id, parent = spec.parent, text = required(spec, "text"),
    x = spec.text_x or spec.x, y = spec.text_y or spec.y,
    z = (spec.z or 0.1) + 0.1, ink = spec.text_ink or spec.ink or "primary",
    size = spec.size or module.theme.small,
  }
end

function widget.metric(spec)
  local id = required(spec, "id")
  widget.label {
    id = id .. "-caption", parent = spec.parent,
    text = required(spec, "label"), x = spec.x, y = spec.y,
    ink = "muted", size = module.theme.micro,
  }
  widget.label {
    id = id .. "-value", parent = spec.parent,
    text = required(spec, "value"), x = spec.x,
    y = (spec.y or 0) - (spec.value_offset or 0.46),
    ink = spec.ink or "text", size = spec.size or module.theme.hero,
  }
  if spec.detail ~= nil and spec.detail ~= "" then
    widget.label {
      id = id .. "-detail", parent = spec.parent,
      text = spec.detail, x = spec.x,
      y = (spec.y or 0) - (spec.detail_offset or 0.92),
      ink = "muted", size = module.theme.small,
    }
  end
end

-- Tokens: the design decisions a component may not make for itself.
--
-- A chrome region that hardcodes a gap will disagree with the next one. These are
-- the only numbers a component reads, and they derive from the theme so retheming
-- moves every surface at once.
function widget.tokens()
  local t = module.theme
  return {
    -- Four steps is enough for a shell; a fifth is how a design system becomes a
    -- paint chart.
    space = { xs = 4, sm = 8, md = 14, lg = 22 },
    radius = { sm = math.max(t.radius * 0.6, 2), md = t.radius, lg = t.radius * 1.6 },
    stroke = { hair = 1, strong = 2 },
    -- Elevation is drawn, not blurred: the scene has no shadow pass, so a lifted
    -- surface is a dark rect offset by a couple of pixels.
    elevation = { flat = 0, raised = 1.5, floating = 3 },
    glass = { tint = 0.72, border = 0.18 },
    density = { row = 38, control = 34, header = 30 },
    motion = { fast = 120, base = 180, slow = 260, spin = 2600 },
    type = {
      display = t.title,
      title = t.title * 0.86,
      body = t.body,
      small = t.small,
      micro = t.micro,
    },
  }
end

-- Components speak logical pixels, because that is what an author reasons in, and
-- convert once here. `spec.space` is a `widget.pixel_space`; without one the
-- module viewport is used, and without that the call is a bug worth raising rather
-- than a layout that silently draws at the wrong scale.
local function space_of(spec)
  if spec.space then return spec.space end
  local viewport = module.viewport
  if viewport and (viewport.width or 0) > 0 and (viewport.height or 0) > 0 then
    return widget.pixel_space(viewport.width, viewport.height)
  end
  error("widget component needs spec.space (a pixel_space) or module.viewport", 3)
end

local function num(value, fallback)
  if type(value) == "number" then return value end
  return fallback
end

-- A lifted surface: offset shadow rect behind an opaque panel.
function widget.glass(spec)
  local sp = space_of(spec)
  local k = widget.tokens()
  local id = required(spec, "id")
  local x, y = num(spec.x, 0), num(spec.y, 0)
  local width, height = required(spec, "width"), required(spec, "height")
  local radius = num(spec.radius, k.radius.lg)
  local lift = num(spec.elevation, k.elevation.floating)
  if lift > 0 then
    widget.panel {
      id = id .. "-shadow", parent = spec.parent,
      x = sp.x(x), y = sp.y(y + lift), z = (spec.z or 0) - 0.02,
      width = sp.w(width), height = sp.h(height),
      ink = "background", opacity = num(spec.shadow_opacity, 0.32), radius = radius,
    }
  end
  widget.panel {
    id = id, parent = spec.parent, x = sp.x(x), y = sp.y(y), z = spec.z,
    width = sp.w(width), height = sp.h(height),
    ink = spec.ink or "surface", rgb = spec.rgb,
    opacity = num(spec.opacity, k.glass.tint), radius = radius,
  }
end

-- A short capsule: fill, then a label inset from its left edge.
function widget.pill(spec)
  local sp = space_of(spec)
  local k = widget.tokens()
  local id = required(spec, "id")
  local x, y = num(spec.x, 0), num(spec.y, 0)
  local width, height = required(spec, "width"), num(spec.height, 22)
  widget.panel {
    id = id, parent = spec.parent, x = sp.x(x), y = sp.y(y), z = spec.z,
    width = sp.w(width), height = sp.h(height),
    ink = spec.ink or "muted", opacity = num(spec.opacity, 0.16), radius = height * 0.5,
  }
  widget.label {
    id = id .. "-text", parent = spec.parent, text = required(spec, "text"),
    x = sp.x(x + num(spec.pad, 10)), y = sp.y(y + (height - k.type.small) * 0.5),
    z = (spec.z or 0) + 0.05, ink = spec.text_ink or "text", size = spec.size or k.type.small,
  }
end

-- A square mark: the launcher's row icon, the dock's app tile.
function widget.tile(spec)
  local sp = space_of(spec)
  local k = widget.tokens()
  local id = required(spec, "id")
  local x, y = num(spec.x, 0), num(spec.y, 0)
  local size = num(spec.size, 26)
  widget.panel {
    id = id, parent = spec.parent, x = sp.x(x), y = sp.y(y), z = spec.z,
    width = sp.w(size), height = sp.h(size),
    ink = spec.ink or "muted", rgb = spec.rgb,
    opacity = num(spec.opacity, 0.18), radius = num(spec.radius, k.radius.md),
  }
  if spec.text and spec.text ~= "" then
    local glyph = spec.size_text or k.type.small
    widget.label {
      id = id .. "-glyph", parent = spec.parent, text = spec.text,
      x = sp.x(x + (size - glyph * 0.62) * 0.5),
      y = sp.y(y + (size - glyph) * 0.5),
      z = (spec.z or 0) + 0.05, ink = spec.text_ink or "text", size = glyph,
    }
  end
end

-- A list row: optional mark, title, optional detail, optional trailing value.
-- Returns its height so a caller can stack rows without arithmetic.
function widget.list_row(spec)
  local sp = space_of(spec)
  local k = widget.tokens()
  local id = required(spec, "id")
  local x, y = num(spec.x, 0), num(spec.y, 0)
  local row = num(spec.height, k.density.row)
  local width = required(spec, "width")
  if spec.selected then
    widget.panel {
      id = id .. "-sel", parent = spec.parent, x = sp.x(x), y = sp.y(y),
      z = (spec.z or 0) - 0.01, width = sp.w(width), height = sp.h(row),
      ink = "primary", opacity = num(spec.selected_opacity, 0.16), radius = k.radius.md,
    }
  end
  local text_left = x + 4
  if spec.mark then
    widget.tile {
      id = id .. "-mark", parent = spec.parent, space = sp,
      x = x + 6, y = y + (row - 26) * 0.5, z = (spec.z or 0) + 0.01,
      size = 26, text = spec.mark, rgb = spec.mark_rgb, ink = spec.mark_ink,
    }
    text_left = x + 42
  end
  local body = spec.size or k.type.body
  widget.label {
    id = id .. "-title", parent = spec.parent, text = required(spec, "title"),
    x = sp.x(text_left), y = sp.y(y + (row - body) * 0.5), z = (spec.z or 0) + 0.02,
    ink = spec.title_ink or "text", size = body,
  }
  if spec.value and spec.value ~= "" then
    widget.label {
      id = id .. "-value", parent = spec.parent, text = spec.value,
      x = sp.x(x + width - 8 - #tostring(spec.value) * (k.type.small * 0.62)),
      y = sp.y(y + (row - k.type.small) * 0.5), z = (spec.z or 0) + 0.02,
      ink = spec.value_ink or "muted", size = k.type.small,
    }
  end
  return row
end

-- A horizontal gauge with a labelled fraction, for battery/volume/storage.
function widget.gauge(spec)
  local sp = space_of(spec)
  local id = required(spec, "id")
  local x, y = num(spec.x, 0), num(spec.y, 0)
  local width = required(spec, "width")
  local height = num(spec.height, 6)
  local fraction = math.max(0, math.min(1, num(spec.fraction, 0)))
  widget.panel {
    id = id .. "-track", parent = spec.parent, x = sp.x(x), y = sp.y(y), z = spec.z,
    width = sp.w(width), height = sp.h(height),
    ink = "muted", opacity = 0.18, radius = height * 0.5,
  }
  if fraction > 0 then
    widget.panel {
      id = id .. "-fill", parent = spec.parent, x = sp.x(x), y = sp.y(y),
      z = (spec.z or 0) + 0.01, width = sp.w(width * fraction), height = sp.h(height),
      ink = spec.ink or "success", opacity = num(spec.opacity, 0.9), radius = height * 0.5,
    }
  end
end

-- A titled section: caption above a hairline. Returns the offset a caller should
-- advance by before drawing content, so sections stack without magic numbers.
function widget.section(spec)
  local sp = space_of(spec)
  local k = widget.tokens()
  local id = required(spec, "id")
  local x, y = num(spec.x, 0), num(spec.y, 0)
  widget.label {
    id = id .. "-caption", parent = spec.parent, text = required(spec, "title"),
    x = sp.x(x), y = sp.y(y), z = spec.z, ink = "muted", size = k.type.micro,
  }
  widget.panel {
    id = id .. "-rule", parent = spec.parent, x = sp.x(x), y = sp.y(y + 18), z = spec.z,
    width = sp.w(required(spec, "width")), height = sp.h(1),
    ink = "muted", opacity = 0.14, radius = 0,
  }
  return k.type.micro + k.space.md
end

-- An indeterminate loading mark: a dotted Lissajous ring, because a rotating arc
-- says "a spinner" and a Lissajous says "this machine knows how to take a
-- breath". No progress is claimed, so no fraction is drawn.
--
-- It is built from round rects rather than a path: the scene has no line
-- primitive, and dots have the useful property that they read as motion even
-- when a frame is dropped.
function widget.spinner(spec)
  local sp = space_of(spec)
  local k = widget.tokens()
  local id = required(spec, "id")
  local x, y = num(spec.x, 0), num(spec.y, 0)
  local radius = num(spec.radius, 13)
  local dots = math.max(6, math.min(num(spec.dots, 24), 64))
  local period = math.max(400, num(spec.period_ms, k.motion.spin or 2600))
  local phase = (module.now_ms or 0) / period
  -- Three and two: the curve only closes after a full turn, so the eye keeps
  -- finding new segments instead of a shape it has already learned.
  local a = num(spec.a, 3)
  local b = num(spec.b, 2)
  local spin = phase * math.pi * 2
  module.animate()
  for i = 1, dots do
    local u = (i - 1) / dots
    local angle = u * math.pi * 2 + spin
    local px = x + radius + radius * 0.92 * math.sin(a * angle)
    local py = y + radius + radius * 0.92 * math.sin(b * angle + math.pi / 2)
    local head = 0.35 + 0.65 * u
    local dot = math.max(1.2, radius * 0.16 * (0.5 + u))
    widget.panel {
      id = id .. "-dot" .. i, parent = spec.parent,
      x = sp.x(px - dot * 0.5), y = sp.y(py + dot * 0.5), z = spec.z,
      width = sp.w(dot), height = sp.h(dot),
      ink = spec.ink or "primary", rgb = spec.rgb,
      opacity = head * num(spec.opacity, 1), radius = dot * 0.5,
    }
  end
  return radius * 2
end
