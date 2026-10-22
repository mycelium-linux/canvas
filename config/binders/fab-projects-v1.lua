local p = event.payload
if type(p) ~= "table" or type(p.projects) ~= "table" then error("fab-projects-v1 requires projects") end
local nodes = canvas.array()
for i = 1, #p.projects do
  local project = p.projects[i]
  local state = tostring(project.status or "unknown")
  local level = state == "failed" and "critical" or (state == "building" and "warning" or (state == "active" and "good" or "neutral"))
  nodes[#nodes + 1] = {
    label = tostring(project.name or project.id or "project"), state = level,
    detail = state .. " · " .. tostring(project.active_deployments or 0) .. " active deploys · " .. tostring(project.artifact_files or 0) .. " artifacts"
  }
end
if #nodes == 0 then nodes[1] = { label = "Projects", state = "neutral", detail = "No retained Fab project activity" } end
return { kind="snapshot", widget={kind="status_list", title="Project lifecycle", nodes=nodes}, data={state="ready",source="fab",provenance="live"} }
