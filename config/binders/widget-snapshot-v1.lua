-- Transport owners emit a bounded, already-normalized widget snapshot.
-- Canvas retains the view identity/layout/theme; this binder may replace only
-- the matching widget kind and its shared source-health metadata.
if type(event.payload) ~= "table" or type(event.payload.widget) ~= "table" then
  error("widget snapshot payload requires widget")
end

return {
  kind = "snapshot",
  widget = event.payload.widget,
  data = event.payload.data or {},
}
