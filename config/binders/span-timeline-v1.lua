-- Shared bounded timed-span adapter for flame, waterfall, Gantt, and event
-- river views. Lua selects semantics once; Rust owns every rendered frame.
local p = event.payload
if type(p) ~= "table" or type(p.spans) ~= "table" then
  error("span-timeline-v1 requires spans")
end
local style = tostring(p.style or "waterfall")
if style ~= "flame" and style ~= "waterfall" and style ~= "gantt" and style ~= "event_river" then
  error("span-timeline-v1 style must be flame, waterfall, gantt, or event_river")
end
return {
  kind = "snapshot",
  widget = {
    kind = "span_timeline",
    title = tostring(p.title or "Timeline"),
    style = style,
    spans = p.spans,
    unit = p.unit and tostring(p.unit) or nil,
  },
  data = {
    state = #p.spans > 0 and (p.state or "ready") or "no_data",
    source = tostring(p.source or event.source or "timeline"),
    detail = p.detail and tostring(p.detail) or nil,
    provenance = "live",
  },
}
