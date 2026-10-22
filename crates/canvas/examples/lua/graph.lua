canvas.group { id = "graph" }

for index, point in ipairs(module.inputs.samples) do
  local bars = math.max(1, math.min(12, math.floor(point.y)))
  canvas.text {
    id = "sample-" .. index,
    parent = "graph",
    text = string.rep("█", bars),
    translation = { 0, (index - 1) * -0.25, 0 }
  }
end
