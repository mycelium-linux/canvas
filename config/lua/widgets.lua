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

function widget.panel(spec)
  canvas.rect {
    id = required(spec, "id"), parent = spec.parent, rgb = spec.rgb,
    ink = spec.ink or "text", opacity = spec.opacity or 0.055,
    radius = spec.radius or module.theme.radius,
    translation = { spec.x or 0, spec.y or 0, spec.z or 0.2 },
    scale = { required(spec, "width"), required(spec, "height"), 1 },
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
